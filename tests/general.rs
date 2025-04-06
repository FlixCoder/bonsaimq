//! General tests and simple cases.
#![allow(
	clippy::tests_outside_test_module,
	clippy::expect_used,
	clippy::unused_async,
	reason = "Tests"
)]

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

/// Counting handler that does not complete the job, so should be retried.
async fn counter(_job: CurrentJob) -> Result<()> {
	let val = COUNT.fetch_add(1, Ordering::SeqCst);
	tracing::info!("Counting execution nr. {}", val + 1);
	panic!("I am panicking even!");
}

static KEPT_ALIVE: AtomicUsize = AtomicUsize::new(0);

/// Waiting handler that tests keep alive mechanics.
async fn keep_alive(_job: CurrentJob) -> Result<()> {
	KEPT_ALIVE.fetch_add(1, Ordering::Relaxed);
	tokio::time::sleep(Duration::from_secs(9999)).await;
	Ok(())
}

job_registry!(JobRegistry, {
	Counter: "counter" => counter,
	KeepAlive: "keep-alive" => keep_alive,
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
#[ntest::timeout(60000)]
async fn retrying() -> Result<()> {
	common::init();

	let db_path = "retrying-test.bonsaidb";
	tokio::fs::remove_dir_all(db_path).await.ok();
	let db = AsyncDatabase::open::<MessageQueueSchema>(StorageConfiguration::new(db_path)).await?;
	let job_runner = JobRunner::new(db.clone()).run::<JobRegistry>();

	let n = 5;
	let id = JobRegistry::Counter
		.builder()
		.id(123_456_789)
		.max_executions(n)
		.retry_timing(bonsaimq::RetryTiming::Fixed(Duration::from_millis(10)))
		.spawn(&db)
		.await?;

	bonsaimq::await_job(id, 100, &db).await?;

	assert_eq!(COUNT.load(Ordering::SeqCst), n as usize);

	job_runner.abort();
	tokio::fs::remove_dir_all(db_path).await?;
	Ok(())
}

#[tokio::test]
async fn kept_alive() -> Result<()> {
	common::init();

	let db_path = "keep-alive-test.bonsaidb";
	tokio::fs::remove_dir_all(db_path).await.ok();
	let db = AsyncDatabase::open::<MessageQueueSchema>(StorageConfiguration::new(db_path)).await?;
	let job_runner = JobRunner::new(db.clone()).run::<JobRegistry>();

	JobRegistry::KeepAlive
		.builder()
		.retry_timing(bonsaimq::RetryTiming::Fixed(Duration::from_millis(500)))
		.spawn(&db)
		.await?;

	tokio::time::sleep(Duration::from_secs(10)).await;

	assert_eq!(KEPT_ALIVE.load(Ordering::Relaxed), 1);

	job_runner.abort();
	tokio::fs::remove_dir_all(db_path).await?;
	Ok(())
}
