//! Simple example using context.
#![allow(
	clippy::expect_used,
	unused_qualifications,
	clippy::unused_async,
	clippy::print_stdout,
	reason = "Example"
)]

mod common;

use bonsaidb::local::{
	AsyncDatabase,
	config::{Builder, StorageConfiguration},
};
use bonsaimq::{CurrentJob, JobRegister, JobRunner, MessageQueueSchema, job_registry};
use color_eyre::{Result, eyre::eyre};

/// Example job function using context. It receives a handle to the current job,
/// which gives the ability to get the context.
async fn greet(mut job: CurrentJob) -> Result<()> {
	let name: &'static str = job.context().ok_or_else(|| eyre!("context should be given"))?;
	println!("Hello {name}!");
	job.complete().await?;
	Ok(())
}

// The JobRegistry provides a way to spawn new jobs and provides the interface
// for the JobRunner to find the functions to execute for the jobs.
job_registry!(JobRegistry, {
	Greet: "greet" => greet,
});

#[tokio::main]
async fn main() -> Result<()> {
	common::init();

	// Open a local database for this example.
	let db_path = "context-example.bonsaidb";
	let db = AsyncDatabase::open::<MessageQueueSchema>(StorageConfiguration::new(db_path)).await?;

	// Start the job runner to execute jobs from the messages in the queue in the
	// database.
	let job_runner = JobRunner::new(db.clone()).with_context("cats").run::<JobRegistry>();

	// Spawn new jobs via a message on the database queue.
	let job_id = JobRegistry::Greet.builder().spawn(&db).await?;

	// Wait for job to finish execution.
	bonsaimq::await_job(job_id, 100, &db).await?;

	job_runner.abort(); // Is done automatically on drop.
	tokio::fs::remove_dir_all(db_path).await?;
	Ok(())
}

#[test]
#[ntest::timeout(10000)]
fn example_context() {
	main().expect("running main");
}
