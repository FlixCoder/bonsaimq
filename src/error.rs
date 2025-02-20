//! Crate errors.

use std::num::TryFromIntError;

use bonsaidb::core::{Error as BonsaiError, pubsub};
use thiserror::Error;

/// This crate's main error type.
#[derive(Debug, Error)]
pub enum Error {
	/// BonsaiDB error.
	#[error("Error interacting with BonsaiDB: {0}")]
	BonsaiDb(#[from] BonsaiError),
	/// PubSub disconnected error.
	#[error("PubSub was disconnected: {0}")]
	PubSubDisconnected(#[from] pubsub::Disconnected),
	/// Getrandom error.
	#[error("Getrandom error: {0}")]
	Getrandom(#[from] getrandom::Error),
	/// Integer conversion error.
	#[error("Integer conversion error: {0}")]
	TryFromInt(#[from] TryFromIntError),
	/// Serde JSON (de-)serialization error.
	#[error("Serde JSON (de-)serialization failed: {0}")]
	SerdeJson(#[from] serde_json::Error),
}

impl Error {
	/// Whether the action that lead to this error should likely be retried.
	#[must_use]
	pub const fn should_retry(&self) -> bool {
		#[allow(clippy::match_like_matches_macro, reason = "Readability & extensibility")]
		match self {
			Self::BonsaiDb(BonsaiError::DocumentConflict(_, _)) => true,
			Self::BonsaiDb(BonsaiError::Networking(_)) => true,
			_ => false,
		}
	}
}
