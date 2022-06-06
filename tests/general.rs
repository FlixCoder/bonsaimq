//! General tests and simple cases.
#![allow(clippy::expect_used, clippy::unused_async)]

mod common;

use std::{
	sync::atomic::{AtomicUsize, Ordering},
	time::Duration,
};

use bonsaidb::local::{
	config::{Builder, StorageConfiguration},
	AsyncDatabase,
};
use bonsaimq::{job_registry, CurrentJob, JobRegister, JobRunner, MessageQueueSchema};
use color_eyre::Result;

/// Counter
static COUNT: AtomicUsize = AtomicUsize::new(0);

/// Counting handler that does not complete the job, so should be retried.
async fn counter(_job: CurrentJob) -> Result<()> {
	COUNT.fetch_add(1, Ordering::SeqCst);
	Ok(())
}

job_registry!(JobRegistry, {
	Counter: "counter" => counter,
});

#[tokio::test]
#[ntest::timeout(10000)]
async fn same_id() -> Result<()> {
	common::init();

	let db_path = "same-id-test.bonsaidb";
	tokio::fs::remove_dir_all(db_path).await.ok();
	let db = AsyncDatabase::open::<MessageQueueSchema>(StorageConfiguration::new(db_path)).await?;

	let id = JobRegistry::Counter.builder().delay(Duration::from_secs(999)).spawn(&db).await?;
	let res = JobRegistry::Counter.builder().id(id).spawn(&db).await;

	assert!(res.is_err());

	tokio::fs::remove_dir_all(db_path).await?;
	Ok(())
}

#[tokio::test]
#[ntest::timeout(30000)]
async fn retrying() -> Result<()> {
	common::init();

	let db_path = "retrying-test.bonsaidb";
	tokio::fs::remove_dir_all(db_path).await.ok();
	let db = AsyncDatabase::open::<MessageQueueSchema>(StorageConfiguration::new(db_path)).await?;
	let job_runner = JobRunner::new(db.clone()).run::<JobRegistry>();

	let n = 4;
	let id = JobRegistry::Counter
		.builder()
		.id(123_456_789)
		.max_retries(n)
		.retry_timing(bonsaimq::RetryTiming::Fixed(Duration::from_millis(10)))
		.spawn(&db)
		.await?;

	bonsaimq::await_job(id, 100, &db).await?;

	assert_eq!(COUNT.load(Ordering::SeqCst), n as usize + 1);

	job_runner.abort();
	tokio::fs::remove_dir_all(db_path).await?;
	Ok(())
}
