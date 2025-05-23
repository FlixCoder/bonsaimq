//! Connector to the database which runs code based on the messages and their
//! type.

use std::{
	fmt::Debug,
	ops::Range,
	sync::{
		Arc,
		atomic::{AtomicU32, Ordering},
	},
	thread::available_parallelism,
	time::Duration,
};

use bonsaidb::core::{
	Error as BonsaiError,
	async_trait::async_trait,
	connection::AsyncConnection,
	document::CollectionDocument,
	pubsub::{AsyncPubSub, AsyncSubscriber},
	schema::{Collection, SerializedCollection, view::map::MappedDocuments},
	transaction::{Operation, Transaction},
};
use time::OffsetDateTime;

use crate::{
	AbortOnDropHandle, CurrentJob, Error, JobRegister,
	queue::{DueMessages, Id, MQ_NOTIFY, Message, MessagePayload, Timestamp},
};

/// Error handler dynamic function type.
type ErrorHandler = Arc<dyn Fn(Box<dyn std::error::Error + Send + Sync>) + Send + Sync>;
/// Type map for saving the runner-context.
type Context = erased_set::ErasedSyncSet;

/// Job Runner. This is the job execution system to be run in the background. It
/// runs on the specified database and using a specific job registry. It also
/// allows to set a callback for errors that appear in jobs.
pub struct JobRunner<DB> {
	/// The database handle.
	db: DB,
	/// The error handling function for the jobs.
	error_handler: Option<ErrorHandler>,
	/// Outside context type-map to provide resources to the jobs.
	context: Context,
	/// Concurrency limits, a range from minimum to maximum concurrent jobs to
	/// be targeted in the execution queue.
	concurrency: Range<u32>,
}

impl<DB> JobRunner<DB>
where
	DB: AsyncConnection + AsyncPubSub + Debug + 'static,
{
	/// Create a new job runner on this database.
	pub fn new(db: DB) -> Self {
		#[expect(clippy::cast_possible_truncation, reason = "Ok to truncate for number of CPUs")]
		let concurrency = available_parallelism()
			.map(|num_cpus| usize::from(num_cpus) as u32 / 2 .. usize::from(num_cpus) as u32 * 2)
			.unwrap_or(3_u32 .. 8_u32);
		Self { db, error_handler: None, context: Context::new(), concurrency }
	}

	/// Set the error handler callback to be called when jobs return an error.
	#[must_use]
	pub fn with_error_handler<F>(mut self, handler: F) -> Self
	where
		F: Fn(Box<dyn std::error::Error + Send + Sync>) + Send + Sync + 'static,
	{
		self.error_handler = Some(Arc::new(handler));
		self
	}

	/// Add context to the runner. Only one instance per type can be inserted!
	#[must_use]
	pub fn with_context<C: Clone + Send + Sync + 'static>(mut self, context: C) -> Self {
		self.context.insert(context);
		self
	}

	/// Set the concurrency limits.
	#[must_use]
	pub const fn with_concurrency_limits(
		mut self,
		min_concurrent: u32,
		max_concurrent: u32,
	) -> Self {
		self.concurrency = min_concurrent .. max_concurrent;
		self
	}

	/// Spawn and run the daemon for processing messages/jobs in the background.
	/// Keep this handle as long as you want jobs to be executed in the
	/// background! You can also use and await the handle like normal
	/// [`JoinHandle`](tokio::task::JoinHandle)s.
	#[must_use]
	pub fn run<REG>(self) -> AbortOnDropHandle<Result<(), Error>>
	where
		REG: JobRegister + Send + Sync + 'static,
	{
		let internal_runner = InternalJobRunner {
			db: Arc::new(self.db),
			error_handler: self.error_handler,
			context: Arc::new(self.context),
			concurrency: self.concurrency,
		};
		tokio::task::spawn(internal_runner.job_queue::<REG>()).into()
	}
}

impl<DB: Debug> Debug for JobRunner<DB> {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		f.debug_struct("JobRunner")
			.field("db", &self.db)
			.field("error_handler", &"<err handler fn>")
			.field("context", &self.context)
			.field("concurrency", &self.concurrency)
			.finish()
	}
}

/// The internal job runner. Created using the public interface [`JobRunner`].
struct InternalJobRunner<DB> {
	/// The database handle.
	db: Arc<DB>,
	/// The error handling function for the jobs.
	error_handler: Option<ErrorHandler>,
	/// Outside context type-map to provide resources to the jobs.
	context: Arc<Context>,
	/// Concurrency limits, a range from minimum to maximum concurrent jobs to
	/// be targeted in the execution queue.
	concurrency: Range<u32>,
}

impl<DB> Clone for InternalJobRunner<DB> {
	fn clone(&self) -> Self {
		Self {
			db: self.db.clone(),
			error_handler: self.error_handler.clone(),
			context: self.context.clone(),
			concurrency: self.concurrency.clone(),
		}
	}
}

impl<DB> InternalJobRunner<DB>
where
	DB: AsyncConnection + AsyncPubSub + Debug + 'static,
{
	/// Get messages that are due at the specified time.
	async fn due_messages(
		&self,
		due_at: Timestamp,
		limit: u32,
	) -> Result<MappedDocuments<CollectionDocument<Message>, DueMessages>, BonsaiError> {
		self.db
			.view::<DueMessages>()
			.with_key_range(.. due_at)
			.limit(limit)
			.query_with_collection_docs()
			.await
	}

	/// Get the duration until the next message is due.
	async fn next_message_due_in(&self, from: Timestamp) -> Result<Duration, BonsaiError> {
		let nanos = self
			.db
			.view::<DueMessages>()
			.with_key_range(from ..)
			.reduce()
			.await?
			.map_or(10_000_000_000, |target| target - from);
		#[expect(clippy::cast_sign_loss, reason = "Timestamps are monotonically increasing")]
		#[expect(
			clippy::cast_possible_truncation,
			reason = "Difference between now and then would be huge if this fails"
		)]
		let duration = Duration::from_nanos(nanos.clamp(0, u64::MAX.into()) as u64);
		Ok(duration)
	}

	/// Get the message payloads for the specified message (ID).
	async fn message_payloads(
		&self,
		id: Id,
	) -> Result<(Option<serde_json::Value>, Option<Vec<u8>>), BonsaiError> {
		Ok(MessagePayload::get_async(&id, self.db.as_ref())
			.await?
			.map_or((None, None), |payload| {
				(payload.contents.payload_json, payload.contents.payload_bytes)
			}))
	}

	/// Internal job queue runner.
	#[tracing::instrument(level = "debug", skip_all, err)]
	async fn job_queue<REG>(self) -> Result<(), Error>
	where
		REG: JobRegister + Send + Sync,
		DB::Subscriber: AsyncSubscriber,
	{
		tracing::debug!("Running JobRunner..");
		let subscriber = self.db.create_subscriber().await?;
		subscriber.subscribe_to(&MQ_NOTIFY).await?;

		let currently_running = Arc::new(AtomicU32::new(0));
		loop {
			let now = OffsetDateTime::now_utc().unix_timestamp_nanos();

			// Retrieve due messages if there is not enough running already
			let running = currently_running.load(Ordering::Relaxed);
			#[expect(clippy::if_then_some_else_none, reason = "It is async")]
			let messages = if running < self.concurrency.start {
				Some(self.due_messages(now, self.concurrency.end.saturating_sub(running)).await?)
			} else {
				None
			};
			tracing::trace!(
				"Handling {} due messages.",
				messages.as_ref().map_or(0, MappedDocuments::len)
			);

			// Execute jobs for the messages
			for msg in messages.iter().flatten() {
				if let Some(job) = REG::from_name(&msg.document.contents.name) {
					// Filter out messages with active dependencies
					if let Some(dependency) = msg.document.contents.execute_after {
						if Message::get_async(&dependency, self.db.as_ref()).await?.is_some() {
							continue;
						}
					}

					// Update the job and start it with the payloads if max executions haven't been
					// reached.
					if self.job_update(msg.document.contents.id).await? {
						let payloads = self.message_payloads(msg.document.contents.id).await?;
						let current_job = CurrentJob {
							id: msg.document.contents.id,
							name: job.name(),
							db: Arc::new(self.clone()),
							payload_json: payloads.0,
							payload_bytes: payloads.1,
							keep_alive: None,
						};

						// Dropping the handle to the running job.. Panics will not cause anything.
						let _jh = current_job.run(job.function(), currently_running.clone());
					}
				} else {
					tracing::trace!(
						"Job {} is not registered and will be ignored.",
						msg.document.contents.name
					);
				}
			}

			// Sleep until the next message is due or a notification comes in.
			let next_due_in = self.next_message_due_in(now).await?;
			tokio::time::timeout(
				next_due_in.max(Duration::from_millis(100)), // Wait at least 100 ms.
				subscriber.receiver().receive_async(),
			)
			.await
			.ok() // Timeout is not a failure
			.transpose()?;
		}
	}
}

impl<DB: Debug> Debug for InternalJobRunner<DB> {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		f.debug_struct("JobRunner")
			.field("db", &self.db)
			.field("error_handler", &"<err handler fn>")
			.field("context", &self.context)
			.field("concurrency", &self.concurrency)
			.finish()
	}
}

/// JobRunner handle for the jobs. Workaround for putting the database into
/// CurrentJob, which requires generics.. Performs all the necessary database
/// access for the jobs.
#[async_trait]
pub(crate) trait JobRunnerHandle: Debug {
	/// Get access to the context map.
	fn context(&self) -> &Context;
	/// Handle an error of a job.
	fn handle_job_error(&self, err: Box<dyn std::error::Error + Send + Sync>);
	/// Complete the job with the specified ID.
	async fn complete(&self, id: Id) -> Result<(), Error>;
	/// Keep the job alive. Updates the job's database message to avoid multiple
	/// concurrent executions.
	async fn keep_alive(&self, id: Id) -> Result<Duration, Error>;
	/// Job update function, that updates the job's database message for the
	/// next retry before job execution. Returns whether the job should really
	/// be executed (true) or if it has already reached the maximum retries
	/// (false).
	async fn job_update(&self, id: Id) -> Result<bool, Error>;
	/// Notify the runner to re-check for jobs to execute now.
	async fn notify(&self) -> Result<(), Error>;
	/// Set a checkpoint by setting the job's input payloads to something new.
	async fn checkpoint(
		&self,
		id: Id,
		payload_json: Option<serde_json::Value>,
		payload_bytes: Option<Vec<u8>>,
	) -> Result<(), Error>;
}

#[async_trait]
impl<DB> JobRunnerHandle for InternalJobRunner<DB>
where
	DB: AsyncConnection + AsyncPubSub + Debug + 'static,
{
	fn context(&self) -> &Context {
		&self.context
	}

	fn handle_job_error(&self, err: Box<dyn std::error::Error + Send + Sync>) {
		if let Some(err_handler) = &self.error_handler {
			err_handler(err);
		}
	}

	#[tracing::instrument(level = "debug", skip(self))]
	async fn complete(&self, id: Id) -> Result<(), Error> {
		tracing::trace!("Completing job {id}.");

		let del_message = Message::get_async(&id, self.db.as_ref()).await?.map(|msg| msg.header);
		let del_payload =
			MessagePayload::get_async(&id, self.db.as_ref()).await?.map(|payload| payload.header);

		let mut tx = Transaction::new();
		if let Some(header) = del_message {
			tx.push(Operation::delete(Message::collection_name(), header.try_into()?));
		}
		if let Some(header) = del_payload {
			tx.push(Operation::delete(MessagePayload::collection_name(), header.try_into()?));
		}
		match tx.apply_async(self.db.as_ref()).await {
			Err(BonsaiError::DocumentNotFound(_, _)) => {}
			Err(err) => return Err(err.into()),
			Ok(_) => {}
		};

		self.db.publish(&MQ_NOTIFY, &()).await?;

		Ok(())
	}

	#[tracing::instrument(level = "debug", skip(self))]
	async fn keep_alive(&self, id: Id) -> Result<Duration, Error> {
		if let Some(mut message) = Message::get_async(&id, self.db.as_ref()).await? {
			tracing::trace!("Keeping job {id} alive.");

			let duration = message.contents.retry_timing.next_duration(message.contents.executions);
			let now = OffsetDateTime::now_utc().unix_timestamp_nanos();

			message.contents.attempt_at = now + Timestamp::try_from(duration.as_nanos())?;
			message.update_async(self.db.as_ref()).await?;

			Ok(duration)
		} else {
			Ok(Duration::default())
		}
	}

	#[tracing::instrument(level = "debug", skip(self))]
	async fn job_update(&self, id: Id) -> Result<bool, Error> {
		if let Some(mut message) = Message::get_async(&id, self.db.as_ref()).await? {
			tracing::trace!("Updating job {id} for execution/retry.");

			message.contents.executions += 1;
			if message.contents.max_executions.is_some_and(|max| message.contents.executions > max)
			{
				self.complete(id).await?;
				return Ok(false);
			}

			let duration = message.contents.retry_timing.next_duration(message.contents.executions);
			let now = OffsetDateTime::now_utc().unix_timestamp_nanos();
			message.contents.attempt_at = now + Timestamp::try_from(duration.as_nanos())?;

			message.update_async(self.db.as_ref()).await?;
			Ok(true)
		} else {
			Ok(false)
		}
	}

	async fn notify(&self) -> Result<(), Error> {
		self.db.publish(&MQ_NOTIFY, &()).await?;
		Ok(())
	}

	async fn checkpoint(
		&self,
		id: Id,
		payload_json: Option<serde_json::Value>,
		payload_bytes: Option<Vec<u8>>,
	) -> Result<(), Error> {
		if let Some(mut payloads) = MessagePayload::get_async(&id, self.db.as_ref()).await? {
			payloads.contents.payload_json = payload_json;
			payloads.contents.payload_bytes = payload_bytes;
			payloads.update_async(self.db.as_ref()).await?;
		}

		Ok(())
	}
}
