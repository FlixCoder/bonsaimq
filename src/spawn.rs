//! Job spawning capabilities.

use std::time::Duration;

use bonsaidb::core::{
	connection::AsyncConnection,
	pubsub::AsyncPubSub,
	transaction::{Operation, Transaction},
};
use serde::Serialize;
use time::OffsetDateTime;

use crate::{
	queue::{
		generate_id, Id, LatestMessage, Message, MessagePayload, RetryTiming, Timestamp, MQ_NOTIFY,
	},
	Error,
};

/// Builder for spawning a job.  By default, `ordered` mode is off and infinite
/// retries with capped exponential backoff is used (1 second initially,
/// maximum 1 hour between tries).
#[derive(Debug, Clone)]
pub struct JobBuilder {
	/// Message name/type.
	name: &'static str,
	/// Message ID.
	id: Option<Id>,
	/// Whether the job should be executed in ordered mode.
	ordered: bool,
	/// Initial execution delay.
	delay: Option<Duration>,
	/// Maximum amount of retries.
	max_retries: Option<u32>,
	/// Retry timing, i.e. how much time should be in between job retries.
	retry_timing: RetryTiming,
	/// JSON payload.
	payload_json: Option<serde_json::Value>,
	/// Byte payload.
	payload_bytes: Option<Vec<u8>>,
}

impl JobBuilder {
	/// Create new [`JobBuilder`]. By default, `ordered` mode is off and
	/// infinite retries with capped exponential backoff is used (1 second
	/// initially, maximum 1 hour between tries).
	#[must_use]
	pub fn new(name: &'static str) -> Self {
		Self {
			name,
			id: None,
			ordered: false,
			delay: None,
			max_retries: None,
			retry_timing: RetryTiming::Backoff {
				initial: Duration::from_secs(1),
				maximum: Some(Duration::from_secs(60 * 60)),
			},
			payload_json: None,
			payload_bytes: None,
		}
	}

	/// Set the message's ID. If not set, a new random one will be generated.
	#[must_use]
	#[inline]
	pub fn id(mut self, id: Id) -> Self {
		self.id = Some(id);
		self
	}

	/// Set whether ordered mode should be used. Ordered messages can only be
	/// executed after the previous ordered message, but unordered messages caan
	/// always be executed independently.
	#[must_use]
	#[inline]
	pub fn ordered(mut self, ordered: bool) -> Self {
		self.ordered = ordered;
		self
	}

	/// Set initial execution delay.
	#[must_use]
	#[inline]
	pub fn delay(mut self, delay: impl Into<Option<Duration>>) -> Self {
		self.delay = delay.into();
		self
	}

	/// Set the maximum number of retries. None = infinite retrying.
	#[must_use]
	#[inline]
	pub fn max_retries(mut self, max_retries: impl Into<Option<u32>>) -> Self {
		self.max_retries = max_retries.into();
		self
	}

	/// Set the retry timing strategy. See [`RetryTiming`] for the possible
	/// values.
	#[must_use]
	#[inline]
	pub fn retry_timing(mut self, timing: RetryTiming) -> Self {
		self.retry_timing = timing;
		self
	}

	/// Set JSON payload. If not set, there will be no JSON input to the job,
	/// but there can still be byte data. The payloads are independent.
	#[inline]
	pub fn payload_json<S: Serialize>(mut self, payload: S) -> Result<Self, serde_json::Error> {
		let value = serde_json::to_value(payload)?;
		self.payload_json = Some(value);
		Ok(self)
	}

	/// Set byte payload. If not set, there will be no byte input to the job,
	/// but there can still be JSON data. The payloads are independent.
	#[must_use]
	#[inline]
	pub fn payload_bytes(mut self, payload: Vec<u8>) -> Self {
		self.payload_bytes = Some(payload);
		self
	}

	/// Prepare the database entries.
	async fn prepare_db_entries<DB>(self, db: &DB) -> Result<(Message, MessagePayload), Error>
	where
		DB: AsyncConnection,
	{
		let execute_after =
			if self.ordered { db.view::<LatestMessage>().reduce().await? } else { None };

		let id = self.id.map_or_else(generate_id, Ok)?;

		let now = OffsetDateTime::now_utc().unix_timestamp_nanos();
		let attempt_at = self
			.delay
			.map(|delay| Timestamp::try_from(delay.as_nanos()))
			.transpose()?
			.map_or(now, |delay| now + delay);

		let message = Message {
			id,
			name: self.name.to_owned(),
			created_at: now,
			attempt_at,
			executions: 0,
			max_retries: self.max_retries,
			retry_timing: self.retry_timing,
			ordered: self.ordered,
			execute_after,
		};
		let payload = MessagePayload {
			message_id: id,
			payload_json: self.payload_json,
			payload_bytes: self.payload_bytes,
		};

		Ok((message, payload))
	}

	/// Spawn the job into the message queue on the database.
	#[tracing::instrument(level = "debug", skip_all)]
	pub async fn spawn<DB>(self, db: &DB) -> Result<Id, Error>
	where
		DB: AsyncConnection + AsyncPubSub,
	{
		let (message, payload) = self.prepare_db_entries(db).await?;
		Transaction::new()
			.with(Operation::push_serialized::<Message>(&message)?)
			.with(Operation::push_serialized::<MessagePayload>(&payload)?)
			.apply_async(db)
			.await?;

		db.publish(&MQ_NOTIFY, &()).await?;

		Ok(message.id)
	}
}
