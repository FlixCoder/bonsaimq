# Bonsaimq

Simple database message queue based on [bonsaidb](https://github.com/khonsulabs/bonsaidb).

The project is highly influenced by [sqlxmq](https://github.com/Diggsey/sqlxmq).

Warning: This project is in early alpha and should not be used in production!

## Usage

Import the project using:

```toml
# adjust the version to the latest version:
bonsaimq = "0.2.0"
# or
bonsaimq = { git = "https://github.com/FlixCoder/bonsaimq" }
```

Then you can use the message/job queue as follows:

- You need job handlers, which are async functions that receive one argument of type `CurrentJob` and return nothing. `CurrentJob` allows interfacing the job to retrieve job input or complete the job etc.
- The macro `job_regristy!` needs to be use to create a job registry, which maps message names/types to the job handlers and allows spawning new jobs.
- A job runner needs to be created and run on a bonsai database. It runs in the background as long as the handle is in scope and executes the jobs according to the incoming messages. It acts on the job registry.

## Example

Besides the following simple example, see the examples in the [examples folder](https://github.com/FlixCoder/bonsaimq/tree/main/examples/) and take a look at the tests.

```rust
use bonsaidb::local::{
	config::{Builder, StorageConfiguration},
	AsyncDatabase,
};
use bonsaimq::{job_registry, CurrentJob, JobRegister, JobRunner, MessageQueueSchema};
use color_eyre::Result;

/// Example job function. It receives a handle to the current job, which gives
/// the ability to get the input payload, complete the job and more.
async fn greet(mut job: CurrentJob) -> color_eyre::Result<()> {
	// Load the JSON payload and make sure it is there.
	let name: String = job.payload_json().expect("input should be given")?;
	println!("Hello {name}!");
	job.complete().await?;
	Ok(())
}

// The JobRegistry provides a way to spawn new jobs and provides the interface
// for the JobRunner to find the functions to execute for the jobs.
job_registry!(JobRegistry, {
	Greetings: "greet" => greet,
});

#[tokio::main]
async fn main() -> Result<()> {
	// Open a local database for this example.
	let db_path = "simple-doc-example.bonsaidb";
	let db = AsyncDatabase::open::<MessageQueueSchema>(StorageConfiguration::new(db_path)).await?;

	// Start the job runner to execute jobs from the messages in the queue in the
	// database.
	let job_runner = JobRunner::new(db.clone()).run::<JobRegistry>();

	// Spawn new jobs via a message on the database queue.
	let job_id = JobRegistry::Greetings.builder().payload_json("cats")?.spawn(&db).await?;

	// Wait for job to finish execution, polling every 100 ms.
	bonsaimq::await_job(job_id, 100, &db).await?;

	// Clean up.
	job_runner.abort(); // Is done automatically on drop.
	tokio::fs::remove_dir_all(db_path).await?;
	Ok(())
}
```

## Lints

This projects uses a bunch of clippy lints for higher code quality and style.

Install [`cargo-lints`](https://github.com/soramitsu/iroha2-cargo_lints) using `cargo install --git https://github.com/FlixCoder/cargo-lints`. The lints are defined in `lints.toml` and can be checked by running `cargo lints clippy --all-targets --workspace`.
