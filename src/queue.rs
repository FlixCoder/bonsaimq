//! Database message queue implementation

use std::time::Duration;

use bonsaidb::core::{
	document::{CollectionDocument, Emit},
	schema::{
		view::map::Mappings, Collection, CollectionViewSchema, ReduceResult, Schema, View,
		ViewMapResult, ViewMappedValue,
	},
};
use serde::{Deserialize, Serialize};

/// Key/ID datatype for messages. Chosen to be compatible with UUIDs.
pub type Id = u128;
/// Timestamp datatype (UNIX timestamp nanos).
pub type Timestamp = i128;

/// Database schema for the message queue.
#[derive(Debug, Schema)]
#[schema(name = "message_queue", collections = [Message, MessagePayload])]
pub struct MessageQueueSchema;

/// The message queue's message metadata.
#[derive(Debug, Serialize, Deserialize, Collection)]
#[collection(name = "messages", primary_key = Id, views = [DueMessages, LatestMessage])]
pub struct Message {
	/// Name of the message, i.e. a text message type identifier.
	name: String,
	/// Commit timestamp.
	created_at: Timestamp,
	/// Next execution timestamp.
	attempt_at: Timestamp,
	/// Number of executions tried.
	executions: u32,
	/// Number of retries to do. None = infinite.
	max_retries: Option<u32>,
	/// Strategy to determine time between retries.
	retry_timing: RetryTiming,
	/// Whether or not the message is to be executed in ordered mode. Ordered
	/// messages are executed after other ordered messages, but unordered
	/// messages are executed immediately.
	ordered: bool,
	/// Dependency, which needs to be finished before. References another
	/// message by ID.
	execute_after: Option<Id>,
}

/// Retry timing strategy.
#[derive(Debug, Serialize, Deserialize)]
pub enum RetryTiming {
	/// Fixed timing between retries.
	Fixed(Duration),
	/// Exponential back-off with maximum time
	Backoff {
		/// Starting time in between retries
		initial: Duration,
		/// Maximum time between retries
		maximum: Option<Duration>,
	},
}

/// The message queue's message payloads.
#[derive(Debug, Serialize, Deserialize, Collection)]
#[collection(name = "message_payloads", primary_key = Id)]
pub struct MessagePayload {
	/// Message JSON payload.
	payload_json: Option<serde_json::Value>,
	/// Message byte payload.
	payload_bytes: Option<Vec<u8>>,
}

/// Messages by their due time.
#[derive(Debug, Clone, View)]
#[view(collection = Message, key = Timestamp, value = u32, name = "due_messages")]
pub struct DueMessages;

impl CollectionViewSchema for DueMessages {
	type View = Self;

	fn map(&self, document: CollectionDocument<Message>) -> ViewMapResult<Self::View> {
		document.header.emit_key_and_value(document.contents.attempt_at, 1)
	}

	fn reduce(
		&self,
		mappings: &[ViewMappedValue<Self::View>],
		_rereduce: bool,
	) -> ReduceResult<Self::View> {
		Ok(mappings.iter().map(|view| view.value).sum())
	}

	fn version(&self) -> u64 {
		0
	}
}

/// Latest Message that is in ordered mode and should be executed before a new
/// one.
#[derive(Debug, Clone, View)]
#[view(collection = Message, key = Timestamp, value = Option<Id>, name = "latest_message")]
pub struct LatestMessage;

impl CollectionViewSchema for LatestMessage {
	type View = Self;

	fn map(&self, document: CollectionDocument<Message>) -> ViewMapResult<Self::View> {
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

	fn unique(&self) -> bool {
		true
	}

	fn version(&self) -> u64 {
		0
	}
}
