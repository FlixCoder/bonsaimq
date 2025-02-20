//! Functions for the library users.

use std::time::Duration;

use bonsaidb::core::{connection::AsyncConnection, schema::SerializedCollection};

use crate::{
	Error,
	queue::{Id, Message},
};

/// Check whether the job with the given ID exists. Can also be used to check if
/// a job has finished already.
pub async fn job_exists<DB: AsyncConnection>(id: Id, db: &DB) -> Result<bool, Error> {
	Ok(Message::get_async(&id, db).await?.is_some())
}

/// Wait for a job to finish using a fixed interval to check if it exists.
pub async fn await_job<DB: AsyncConnection>(
	id: Id,
	interval_ms: u64,
	db: &DB,
) -> Result<(), Error> {
	let mut int = tokio::time::interval(Duration::from_millis(interval_ms));
	loop {
		if !job_exists(id, db).await? {
			return Ok(());
		}
		int.tick().await;
	}
}
