//! Crate errors.

use std::num::TryFromIntError;

use bonsaidb::core::{pubsub, Error as BonsaiError};
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
}

impl Error {
	/// Whether the action that lead to this error should likely be retried.
	#[must_use]
	pub fn should_retry(&self) -> bool {
		#[allow(clippy::match_like_matches_macro)]
		match self {
			Self::BonsaiDb(BonsaiError::DocumentConflict(_, _)) => true,
			Self::BonsaiDb(BonsaiError::Networking(_)) => true,
			_ => false,
		}
	}
}
