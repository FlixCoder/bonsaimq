//! Connector to the database which runs code based on the messages and their
//! type.

use std::{fmt::Debug, sync::Arc, time::Duration};

use bonsaidb::core::{
	async_trait::async_trait,
	connection::AsyncConnection,
	document::CollectionDocument,
	pubsub::{AsyncPubSub, AsyncSubscriber},
	schema::{view::map::MappedDocuments, Collection, SerializedCollection},
	transaction::{Operation, Transaction},
	Error as BonsaiError,
};
use time::OffsetDateTime;

use crate::{
	queue::{DueMessages, Id, Message, MessagePayload, Timestamp, MQ_NOTIFY},
	AbortOnDropHandle, CurrentJob, Error, JobRegister,
};

/// Job Runner. This is the job execution system to be run in the background. It
/// runs on the specified database and using a specific job registry.
#[derive(Debug)]
pub struct JobRunner<DB> {
	/// The database handle.
	db: Arc<DB>,
}

impl<DB> Clone for JobRunner<DB> {
	fn clone(&self) -> Self {
		Self { db: self.db.clone() }
	}
}

impl<DB> JobRunner<DB>
where
	DB: AsyncConnection + AsyncPubSub + Debug + 'static,
{
	/// Create a new job runner on this database.
	pub fn new(db: DB) -> Self {
		Self { db: Arc::new(db) }
	}

	/// Get messages that are due at the specified time.
	async fn due_messages(
		&self,
		due_at: Timestamp,
	) -> Result<MappedDocuments<CollectionDocument<Message>, DueMessages>, BonsaiError> {
		self.db.view::<DueMessages>().with_key_range(..due_at).query_with_collection_docs().await
	}

	/// Get the duration until the next message is due.
	async fn next_message_due_in(&self, from: Timestamp) -> Result<Duration, BonsaiError> {
		let nanos = self
			.db
			.view::<DueMessages>()
			.with_key_range(from..)
			.reduce()
			.await?
			.map_or(10_000_000_000, |target| target - from);
		let duration = Duration::from_nanos(nanos.clamp(0, u64::MAX.into()) as u64);
		Ok(duration)
	}

	/// Get the message payloads for the specified message (ID).
	async fn message_payloads(
		&self,
		id: Id,
	) -> Result<(Option<serde_json::Value>, Option<Vec<u8>>), BonsaiError> {
		Ok(MessagePayload::get_async(id, self.db.as_ref()).await?.map_or((None, None), |payload| {
			(payload.contents.payload_json, payload.contents.payload_bytes)
		}))
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
		tokio::task::spawn(self.job_queue::<REG>()).into()
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

		loop {
			// Retrieve due messages
			let now = OffsetDateTime::now_utc().unix_timestamp_nanos();
			let messages = self.due_messages(now).await?;
			tracing::trace!("Found {} due messages.", messages.len());

			// Execute jobs for the messages
			for msg in &messages {
				if let Some(job) = REG::from_name(&msg.document.contents.name) {
					// Filter out messages with active dependencies
					if let Some(dependency) = msg.document.contents.execute_after {
						if Message::get_async(dependency, self.db.as_ref()).await?.is_some() {
							continue;
						}
					}

					// Keep alive, load payload annd start the job
					let keep_alive =
						CurrentJob::keep_alive(Arc::new(self.clone()), msg.document.contents.id)
							.await?;

					let payloads = self.message_payloads(msg.document.contents.id).await?;
					let current_job = CurrentJob {
						id: msg.document.contents.id,
						name: job.name(),
						db: Arc::new(self.clone()),
						payload_json: payloads.0,
						payload_bytes: payloads.1,
						keep_alive: Some(keep_alive.into()),
					};

					// TODO: Do something with the job handle?
					let _jh = current_job.run(job.function());
				} else {
					// TODO: Just silently ignore?
					tracing::trace!(
						"Job {} is not registered and will be ignored.",
						msg.document.contents.name
					);
				}
			}

			// Sleep until the next message is due or a notification comes in.
			let next_due_in = self.next_message_due_in(now).await?;
			tokio::time::timeout(next_due_in, subscriber.receiver().receive_async())
				.await
				.ok() // Timeout is not a failure
				.transpose()?;
		}
	}
}

/// JobRunner handle for the jobs. Workaround for putting the database into
/// CurrentJob, which requires generics.. Performs all the necessary database
/// access for the jobs.
#[async_trait]
pub(crate) trait JobRunnerHandle: Debug {
	/// Complete the job with the specified ID.
	async fn complete(&self, id: Id) -> Result<(), Error>;
	/// Keep the job alive. Updates the job's database message to avoid multiple
	/// concurrent executions.
	async fn keep_alive(&self, id: Id) -> Result<Duration, Error>;
	/// Job update function, that updates the job's database message for the
	/// next retry after job execution.
	async fn job_update(&self, id: Id) -> Result<(), Error>;
}

#[async_trait]
impl<DB: Debug> JobRunnerHandle for JobRunner<DB>
where
	DB: AsyncConnection + AsyncPubSub + 'static,
{
	#[tracing::instrument(level = "debug", skip(self))]
	async fn complete(&self, id: Id) -> Result<(), Error> {
		tracing::trace!("Completing job {id}.");

		let del_message = Message::get_async(id, self.db.as_ref()).await?.map(|msg| msg.header);
		let del_payload =
			MessagePayload::get_async(id, self.db.as_ref()).await?.map(|payload| payload.header);

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
		if let Some(mut message) = Message::get_async(id, self.db.as_ref()).await? {
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
	async fn job_update(&self, id: Id) -> Result<(), Error> {
		if let Some(mut message) = Message::get_async(id, self.db.as_ref()).await? {
			tracing::trace!("Updating job {id} for retry.");

			if message.contents.max_retries.map_or(false, |max| message.contents.executions >= max)
			{
				return self.complete(id).await;
			}

			let duration = message.contents.retry_timing.next_duration(message.contents.executions);
			let now = OffsetDateTime::now_utc().unix_timestamp_nanos();
			message.contents.attempt_at = now + Timestamp::try_from(duration.as_nanos())?;
			message.contents.executions += 1;

			message.update_async(self.db.as_ref()).await?;
		}
		Ok(())
	}

	// TODO: Checkpoint capability.
}
