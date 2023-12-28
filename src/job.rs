//! Provider for job handlers.

use std::sync::{
	atomic::{AtomicUsize, Ordering},
	Arc,
};

use serde::{de::DeserializeOwned, Serialize};
use tokio::task::JoinHandle;
use tokio_retry::{strategy::FixedInterval, RetryIf};
use tracing_futures::Instrument;

use crate::{queue::Id, runner::JobRunnerHandle, AbortOnDropHandle, Error, JobFunctionType};

/// Handle to the `JobRunner`.
type JobRunnerHandler = Arc<dyn JobRunnerHandle + Send + Sync>;

/// Handle for the job handlers. Allows retrieving input data, setting
/// checkpoints and completing the job. The job is kept alive as long as this
/// object lives.
#[derive(Debug)]
pub struct CurrentJob {
	/// ID of this job (message ID).
	pub(crate) id: Id,
	/// Name of job (the message name/type).
	pub(crate) name: &'static str,
	/// Database handle.
	pub(crate) db: JobRunnerHandler,
	/// The job's input JSON payload.
	pub(crate) payload_json: Option<serde_json::Value>,
	/// The job's input bytes payload.
	pub(crate) payload_bytes: Option<Vec<u8>>,
	/// Keep alive job Joinhandle.
	pub(crate) keep_alive: Option<AbortOnDropHandle<Result<(), Error>>>,
}

impl CurrentJob {
	/// Get the job's ID.
	#[must_use]
	pub fn id(&self) -> Id {
		self.id
	}

	/// Get the job's name.
	#[must_use]
	pub fn name(&self) -> &'static str {
		self.name
	}

	/// Get the job's JSON input (payload).
	#[must_use]
	pub fn payload_json<D: DeserializeOwned>(&self) -> Option<Result<D, serde_json::Error>> {
		self.payload_json.as_ref().map(|payload| serde_json::from_value(payload.clone()))
	}

	/// Get the job's byte input (payload).
	#[must_use]
	pub fn payload_bytes(&self) -> Option<&Vec<u8>> {
		self.payload_bytes.as_ref()
	}

	/// Retrieve the context of the specified type.
	#[must_use]
	pub fn context<C: Clone + Send + Sync + 'static>(&self) -> Option<C> {
		self.db.context().get::<C>().cloned()
	}

	/// Complete the job. Mark it as completed. Without doing this, it will
	/// be retried!
	///
	/// This method retries, but still can fail and should possibly be retried
	/// in that case. You can use [`Error::should_retry`] to find out.
	pub async fn complete(&mut self) -> Result<(), Error> {
		RetryIf::spawn(
			FixedInterval::from_millis(10).take(2),
			|| self.db.complete(self.id),
			Error::should_retry,
		)
		.await?;
		if let Some(keep_alive) = self.keep_alive.take() {
			keep_alive.abort();
		};
		Ok(())
	}

	/// Start setting a checkpoint (with the checkpoint builder), which means
	/// simply setting the input payload for the job. The next job execution
	/// will then start with the new input data. It is recommended to keep
	/// running the job and use the data without restarting the job as well.
	/// Otherwise, keep in mind that this checkpoint does not extend the job's
	/// number of executions, so there might be less maximum executions than
	/// needed for executing all checkpoint stages.
	#[must_use]
	pub fn checkpoint(&mut self) -> Checkpoint<'_> {
		Checkpoint::new(self)
	}

	/// Keep alive this job by pushing forward the `attempt_at` field in the
	/// database.
	pub(crate) fn keep_alive(db: JobRunnerHandler, id: Id) -> JoinHandle<Result<(), Error>> {
		let span = tracing::debug_span!("job-keep-alive");
		tokio::task::spawn(
			async move {
				loop {
					let duration = RetryIf::spawn(
						FixedInterval::from_millis(10).take(2),
						|| db.keep_alive(id),
						Error::should_retry,
					)
					.await?;
					tokio::time::sleep(duration.div_f32(2.0)).await;
				}
			}
			.instrument(span),
		)
	}

	/// Job running function that handles retries as well etc.
	pub(crate) fn run(
		mut self,
		mut function: JobFunctionType,
		currently_running: Arc<AtomicUsize>,
	) -> JoinHandle<Result<(), Error>> {
		self.keep_alive = Some(Self::keep_alive(self.db.clone(), self.id).into());

		let span = tracing::debug_span!("job-run");
		currently_running.fetch_add(1, Ordering::Relaxed);
		tokio::task::spawn(
			async move {
				let id = self.id;
				let db = self.db.clone();

				tracing::trace!("Starting job with ID {id}.");
				let res = function(self).await;
				currently_running.fetch_sub(1, Ordering::Relaxed);

				// Handle the job's error
				if let Err(err) = res {
					db.handle_job_error(err);
				}
				db.notify().await?;

				tracing::trace!("Job with ID {id} finished execution.");
				Ok(())
			}
			.instrument(span),
		)
	}
}

/// Checkpoint builder and setter.
#[derive(Debug)]
pub struct Checkpoint<'a> {
	/// The handle to the current job
	job: &'a mut CurrentJob,
	/// JSON payload.
	payload_json: Option<serde_json::Value>,
	/// Byte payload.
	payload_bytes: Option<Vec<u8>>,
}

impl<'a> Checkpoint<'a> {
	/// Create new checkpoint builder / setter.
	fn new(job: &'a mut CurrentJob) -> Self {
		let payload_json = job.payload_json.clone();
		let payload_bytes = job.payload_bytes.clone();
		Self { job, payload_json, payload_bytes }
	}

	/// Set the JSON payload to a new value.
	pub fn payload_json<S: Serialize>(
		mut self,
		payload: impl Into<Option<S>>,
	) -> Result<Self, Error> {
		let payload_json = payload.into().map(|s| serde_json::to_value(s)).transpose()?;
		self.payload_json = payload_json;
		Ok(self)
	}

	/// Set the byte payload to a new value.
	#[must_use]
	pub fn payload_bytes(mut self, payload: impl Into<Option<Vec<u8>>>) -> Self {
		self.payload_bytes = payload.into();
		self
	}

	/// Set the checkpoint and write it to the database.
	///
	/// This method retries, but still can fail and should possibly be retried
	/// in that case. You can use [`Error::should_retry`] to find out.
	pub async fn set(self) -> Result<(), Error> {
		RetryIf::spawn(
			FixedInterval::from_millis(10).take(2),
			|| {
				self.job.db.checkpoint(
					self.job.id,
					self.payload_json.clone(),
					self.payload_bytes.clone(),
				)
			},
			Error::should_retry,
		)
		.await?;

		self.job.payload_json = self.payload_json;
		self.job.payload_bytes = self.payload_bytes;

		Ok(())
	}
}
