//! Provider for job handlers.

use std::sync::Arc;

use serde::de::DeserializeOwned;
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
	pub(crate) fn run(mut self, mut function: JobFunctionType) -> JoinHandle<Result<(), Error>> {
		self.keep_alive = Some(Self::keep_alive(self.db.clone(), self.id).into());

		let span = tracing::debug_span!("job-run");
		tokio::task::spawn(
			async move {
				let id = self.id;
				let db = self.db.clone();

				tracing::trace!("Starting job with ID {id}.");
				let res = function(self).await;

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

	/// Complete the job. Mark it as completed. Without doing this, it will
	/// be retried!
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

	// TODO: Checkpoint capability.
}
