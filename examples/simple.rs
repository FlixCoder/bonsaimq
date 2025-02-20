//! Simple example.
#![allow(
	clippy::expect_used,
	unused_qualifications,
	clippy::unused_async,
	clippy::print_stdout,
	reason = "Example"
)]

mod common;

use bonsaidb::local::{
	config::{Builder, StorageConfiguration},
	AsyncDatabase,
};
use bonsaimq::{job_registry, CurrentJob, JobRegister, JobRunner, MessageQueueSchema};
use color_eyre::Result;

/// Example job function. It receives a handle to the current job, which gives
/// the ability to get the input payload, complete the job and more. The
/// function returns an error that can be turned into a `Box<dyn Error + Send +
/// Sync>`.
async fn greet(mut job: CurrentJob) -> Result<()> {
	// Load the JSON payload and make sure it is there.
	let name: String = job.payload_json().expect("input should be given").expect("deserializing");
	println!("Hello {name}!");
	job.complete().await.expect("access to DB");
	Ok(())
}

/// Example job function 2, using a general error type.
async fn greet_german(_job: CurrentJob) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
	println!("This one fails and would be retried after a second.");
	// No job.complete()
	Ok(())
}

// The JobRegistry provides a way to spawn new jobs and provides the interface
// for the JobRunner to find the functions to execute for the jobs.
job_registry!(JobRegistry, {
	cats: "cats" => greet,
	Foxes: "foxes" => self::greet_german,
});

#[tokio::main]
async fn main() -> Result<()> {
	common::init();

	// Open a local database for this example.
	let db_path = "simple-example.bonsaidb";
	let db = AsyncDatabase::open::<MessageQueueSchema>(StorageConfiguration::new(db_path)).await?;

	// Start the job runner to execute jobs from the messages in the queue in the
	// database.
	let job_runner = JobRunner::new(db.clone()).run::<JobRegistry>();

	// Spawn new jobs via a message on the database queue.
	let job_id = JobRegistry::cats.builder().payload_json("cats")?.spawn(&db).await?;
	JobRegistry::Foxes.builder().spawn(&db).await?;

	// Wait for job to finish execution.
	bonsaimq::await_job(job_id, 100, &db).await?;

	job_runner.abort(); // Is done automatically on drop.
	tokio::fs::remove_dir_all(db_path).await?;
	Ok(())
}

#[test]
#[ntest::timeout(10000)]
fn example_simple() {
	main().expect("running main");
}
