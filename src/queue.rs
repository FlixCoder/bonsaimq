//! Database storage for the message queue.

use std::time::Duration;

use bonsaidb::core::{
	document::{CollectionDocument, Emit},
	schema::{
		view::{map::Mappings, ViewUpdatePolicy},
		Collection, CollectionMapReduce, ReduceResult, Schema, View, ViewMapResult,
		ViewMappedValue, ViewSchema,
	},
};
use serde::{Deserialize, Serialize};

/// Key/ID datatype for messages. Chosen to be compatible with UUIDs.
pub type Id = u128;
/// Timestamp datatype (UNIX timestamp nanos).
pub type Timestamp = i128;

/// PubSub channel for notifying of new messages to check.
pub(crate) const MQ_NOTIFY: &str = "message_queue_notify";

/// Database schema for the message queue.
#[derive(Debug, Schema)]
#[schema(name = "message_queue", collections = [Message, MessagePayload])]
pub struct MessageQueueSchema;

/// The message queue's message metadata.
#[derive(Debug, Clone, Serialize, Deserialize, Collection)]
#[collection(
	name = "messages",
	primary_key = Id,
	views = [DueMessages, LatestMessage]
)]
pub struct Message {
	/// The message ID.#
	#[natural_id]
	pub id: Id,
	/// Name of the message, i.e. a text message type identifier.
	pub name: String,
	/// Commit timestamp.
	pub created_at: Timestamp,
	/// Next execution timestamp.
	pub attempt_at: Timestamp,
	/// Number of executions tried.
	pub executions: u32,
	/// Number of executions to do. None = infinite. Zero means never being
	/// executed!
	pub max_executions: Option<u32>,
	/// Strategy to determine time between retries.
	pub retry_timing: RetryTiming,
	/// Whether or not the message is to be executed in ordered mode. Ordered
	/// messages are executed after other ordered messages, but unordered
	/// messages are executed immediately.
	pub ordered: bool,
	/// Dependency, which needs to be finished before. References another
	/// message by ID.
	pub execute_after: Option<Id>,
}

/// Retry timing strategy.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum RetryTiming {
	/// Fixed timing between retries.
	Fixed(Duration),
	/// Exponential back-off with maximum time.
	Backoff {
		/// Initial time in between retries.
		initial: Duration,
		/// Maximum time between retries.
		maximum: Option<Duration>,
	},
}

/// The message queue's message payloads.
#[derive(Debug, Clone, Serialize, Deserialize, Collection)]
#[collection(
	name = "message_payloads",
	primary_key = Id,
)]
pub struct MessagePayload {
	/// The message ID.
	#[natural_id]
	pub message_id: Id,
	/// Message JSON payload.
	pub payload_json: Option<serde_json::Value>,
	/// Message byte payload.
	pub payload_bytes: Option<Vec<u8>>,
}

/// Messages by their due time. Reduces to the first message to be executed in
/// the key range. Use by key ranges ro receive all due messages. Reduce to
/// receive the first message to be executed, which can be used to sleep until
/// then.
#[derive(Debug, Clone, View)]
#[view(collection = Message, key = Timestamp, value = Option<Timestamp>, name = "due_messages")]
pub struct DueMessages;

impl ViewSchema for DueMessages {
	type View = Self;
	type MappedKey<'doc> = <Self as View>::Key;

	fn update_policy(&self) -> ViewUpdatePolicy {
		ViewUpdatePolicy::default()
	}

	fn version(&self) -> u64 {
		0
	}
}

impl CollectionMapReduce for DueMessages {
	fn map<'doc>(&self, document: CollectionDocument<Message>) -> ViewMapResult<'doc, Self::View> {
		document
			.header
			.emit_key_and_value(document.contents.attempt_at, Some(document.contents.attempt_at))
	}

	fn reduce(
		&self,
		mappings: &[ViewMappedValue<Self::View>],
		_rereduce: bool,
	) -> ReduceResult<Self::View> {
		Ok(mappings.iter().filter_map(|view| view.value).min())
	}
}

/// Latest Message view that reduces to messages in ordered mode that should be
/// executed before a new one. This should be used to find the dependency for
/// new ordered messages.
#[derive(Debug, Clone, View)]
#[view(collection = Message, key = Timestamp, value = Option<Id>, name = "latest_message")]
pub struct LatestMessage;

impl ViewSchema for LatestMessage {
	type View = Self;
	type MappedKey<'doc> = <Self as View>::Key;

	fn update_policy(&self) -> ViewUpdatePolicy {
		ViewUpdatePolicy::Unique
	}

	fn version(&self) -> u64 {
		0
	}
}

impl CollectionMapReduce for LatestMessage {
	fn map<'doc>(&self, document: CollectionDocument<Message>) -> ViewMapResult<'doc, Self::View> {
		if document.contents.ordered {
			document
				.header
				.emit_key_and_value(document.contents.created_at, Some(document.header.id))
		} else {
			Ok(Mappings::Simple(None))
		}
	}

	fn reduce(
		&self,
		mappings: &[ViewMappedValue<Self::View>],
		_rereduce: bool,
	) -> ReduceResult<Self::View> {
		let max_val = mappings.iter().max_by_key(|view| view.key);
		Ok(max_val.and_then(|view| view.value))
	}
}

impl RetryTiming {
	/// Compute the next retry duration based on the number of executions
	/// already done. So for calculating the time after the first execution,
	/// executions is supposed to be one.
	#[must_use]
	pub fn next_duration(&self, executions: u32) -> Duration {
		match *self {
			RetryTiming::Fixed(fixed) => fixed,
			RetryTiming::Backoff { initial, maximum } => {
				let duration =
					initial.saturating_mul(2_u32.saturating_pow(executions.saturating_sub(1)));

				if let Some(max) = maximum {
					duration.min(max)
				} else {
					duration
				}
			}
		}
	}
}

/// Generate a new ID.
pub(crate) fn generate_id() -> Result<Id, getrandom::Error> {
	let mut buf = [0_u8; std::mem::size_of::<Id>()];
	getrandom::getrandom(&mut buf)?;
	// SAFETY: Safe because we made sure it has the correct size using size_of.
	let id = unsafe { std::mem::transmute(buf) };
	Ok(id)
}
