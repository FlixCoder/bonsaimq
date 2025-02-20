//! Example showing the error handling in jobs.
#![allow(
	clippy::expect_used,
	unused_qualifications,
	clippy::unused_async,
	clippy::print_stdout,
	reason = "Example"
)]

mod common;

use std::sync::{
	atomic::{AtomicBool, Ordering},
	Arc,
};

use bonsaidb::local::{
	config::{Builder, StorageConfiguration},
	AsyncDatabase,
};
use bonsaimq::{job_registry, CurrentJob, JobRegister, JobRunner, MessageQueueSchema};
use color_eyre::{eyre::bail, Result};

/// Example job function that returns an error.
async fn greet(mut job: CurrentJob) -> Result<()> {
	job.complete().await?;
	bail!("This is an error!");
}

// The JobRegistry provides a way to spawn new jobs and provides the interface
// for the JobRunner to find the functions to execute for the jobs.
job_registry!(JobRegistry, {
	cats: "cats" => greet,
});

#[tokio::main]
async fn main() -> Result<()> {
	common::init();

	// Open a local database for this example.
	let db_path = "error-handling-example.bonsaidb";
	let db = AsyncDatabase::open::<MessageQueueSchema>(StorageConfiguration::new(db_path)).await?;

	// Start the job runner to execute jobs from the messages in the queue in the
	// database. Add an error handler for handling job errors.
	let error_received = Arc::new(AtomicBool::new(false));
	let err_received = error_received.clone();
	let job_runner = JobRunner::new(db.clone())
		.with_error_handler(move |_err| {
			err_received.store(true, Ordering::SeqCst);
		})
		.run::<JobRegistry>();

	// Spawn new jobs via a message on the database queue and wait for its
	// execution.
	let job_id = JobRegistry::cats.builder().spawn(&db).await?;
	bonsaimq::await_job(job_id, 100, &db).await?;

	// Check that the error was received.
	assert!(error_received.load(Ordering::SeqCst));

	job_runner.abort(); // Is done automatically on drop.
	tokio::fs::remove_dir_all(db_path).await?;
	Ok(())
}

#[test]
#[ntest::timeout(10000)]
fn example_error_handling() {
	main().expect("running main");
}
