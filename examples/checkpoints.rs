//! Checkpoint example.
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

/// Example job function using checkpoints.
async fn checkpoints(mut job: CurrentJob) -> Result<()> {
	loop {
		match job.payload_json::<usize>().transpose()? {
			None => {
				println!("Beginning new job without input, without checkpoint.");
				job.checkpoint().payload_json(1_usize)?.set().await?;
			}
			Some(mut count) => {
				println!("Continuing job from checkpoint with count {count}");
				count += 1;
				job.checkpoint().payload_json(count)?.set().await?;
				if count >= 5 {
					job.complete().await?;
					println!("Job completed!");
					return Ok(());
				}
			}
		}
	}
}

// The JobRegistry provides a way to spawn new jobs and provides the interface
// for the JobRunner to find the functions to execute for the jobs.
job_registry!(JobRegistry, {
	Checkpoints: "checkpoints" => checkpoints,
});

#[tokio::main]
async fn main() -> Result<()> {
	common::init();

	// Open a local database for this example.
	let db_path = "checkpoints-example.bonsaidb";
	let db = AsyncDatabase::open::<MessageQueueSchema>(StorageConfiguration::new(db_path)).await?;

	// Start the job runner to execute jobs from the messages in the queue in the
	// database.
	let job_runner = JobRunner::new(db.clone()).run::<JobRegistry>();

	// Spawn new jobs via a message on the database queue.
	let job_id = JobRegistry::Checkpoints.builder().spawn(&db).await?;

	// Wait for job to finish execution.
	bonsaimq::await_job(job_id, 100, &db).await?;

	job_runner.abort(); // Is done automatically on drop.
	tokio::fs::remove_dir_all(db_path).await?;
	Ok(())
}

#[test]
#[ntest::timeout(10000)]
fn example_checkpoints() {
	main().expect("running main");
}
