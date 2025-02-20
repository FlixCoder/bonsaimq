//! Testing that job order works as expected.
#![allow(clippy::expect_used, clippy::indexing_slicing, reason = "Tests")]

mod common;

use std::sync::atomic::{AtomicUsize, Ordering};

use bonsaidb::local::{
	AsyncDatabase,
	config::{Builder, StorageConfiguration},
};
use bonsaimq::{CurrentJob, JobRegister, JobRunner, MessageQueueSchema, job_registry};
use color_eyre::Result;

/// Counter
static COUNT: AtomicUsize = AtomicUsize::new(0);

/// Ordered counting handler.
async fn counter(mut job: CurrentJob) -> Result<()> {
	let expected = job.payload_bytes().expect("byte payload")[0];
	let nr = COUNT.fetch_add(1, Ordering::SeqCst);
	assert_eq!(nr, expected as usize);
	job.complete().await?;
	Ok(())
}

job_registry!(JobRegistry, {
	Counter: "counter" => counter,
});

#[tokio::test]
#[ntest::timeout(60000)]
async fn job_order() -> Result<()> {
	common::init();

	let db_path = "ordering-test.bonsaidb";
	tokio::fs::remove_dir_all(db_path).await.ok();
	let db = AsyncDatabase::open::<MessageQueueSchema>(StorageConfiguration::new(db_path)).await?;
	let job_runner = JobRunner::new(db.clone()).run::<JobRegistry>();

	let n = 100_u8;
	let mut jobs = Vec::with_capacity(n as usize);
	for i in 0 .. n {
		let job_id = JobRegistry::Counter.builder().payload_bytes(vec![i]).spawn(&db).await?;
		jobs.push(job_id);
	}

	// Wait for jobs to finish
	for job_id in jobs {
		bonsaimq::await_job(job_id, 100, &db).await?;
	}

	let value = COUNT.load(Ordering::SeqCst);
	assert_eq!(value, n as usize);

	job_runner.abort();
	tokio::fs::remove_dir_all(db_path).await?;
	Ok(())
}
