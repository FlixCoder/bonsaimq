//! Stress-test, making sure everything still works as expected
#![allow(clippy::tests_outside_test_module, clippy::expect_used, reason = "Tests")]

mod common;

use std::{
	sync::atomic::{AtomicUsize, Ordering},
	time::Duration,
};

use bonsaidb::local::{
	AsyncDatabase,
	config::{Builder, StorageConfiguration},
};
use bonsaimq::{CurrentJob, JobRegister, JobRunner, MessageQueueSchema, job_registry};
use color_eyre::Result;

/// Counter
static COUNT: AtomicUsize = AtomicUsize::new(0);

/// Counting handler.
async fn counter(mut job: CurrentJob) -> Result<()> {
	COUNT.fetch_add(1, Ordering::SeqCst);
	job.complete().await?;
	Ok(())
}

job_registry!(JobRegistry, {
	Counter: "counter" => counter,
});

#[tokio::test]
#[ntest::timeout(60000)]
async fn stress_test() -> Result<()> {
	common::init();

	let db_path = "stress-test.bonsaidb";
	tokio::fs::remove_dir_all(db_path).await.ok();
	let db = AsyncDatabase::open::<MessageQueueSchema>(StorageConfiguration::new(db_path)).await?;
	let job_runner = JobRunner::new(db.clone()).run::<JobRegistry>();

	let n = 100;
	let mut jobs = Vec::with_capacity(n);
	for _ in 0 .. n {
		let job_id =
			JobRegistry::Counter.builder().delay(Duration::from_millis(500)).spawn(&db).await?;
		jobs.push(job_id);
	}

	// Wait for jobs to finish
	for job_id in jobs {
		bonsaimq::await_job(job_id, 100, &db).await?;
	}

	let value = COUNT.load(Ordering::SeqCst);
	assert_eq!(value, n);

	job_runner.abort();
	tokio::fs::remove_dir_all(db_path).await?;
	Ok(())
}
