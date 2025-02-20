//! Job registry for finding the code to execute based on incoming messages of
//! specified types. Allows to spawn new jobs / messages using
//! `JobRegistry::Handle.builder().spawn().await?`.

use std::{error::Error, future::Future, pin::Pin};

use crate::{CurrentJob, spawn::JobBuilder};

/// Function type of the jobs returned by the job registry.
pub type JobFunctionType = Box<
	dyn FnMut(
			CurrentJob,
		) -> Pin<Box<dyn Future<Output = Result<(), Box<dyn Error + Send + Sync>>> + Send>>
		+ Send,
>;

/// Functions the registry exposes.
pub trait JobRegister: Sized {
	/// Spawn a new job/message using this builder.
	fn builder(self) -> JobBuilder;
	/// Return the message name of this message/job.
	fn name(&self) -> &'static str;
	/// Get the registry entry based on message name. Returns None if not found.
	fn from_name(name: &str) -> Option<Self>;
	/// Return the handler for this message/job.
	fn function(&self) -> JobFunctionType;
}

/// Creates a job registry with the given name as first parameter. The second
/// parameter is a named map of message names to functions, which are executed
/// for the message.
///
/// Example:
/// ```
/// # use bonsaimq::{job_registry, CurrentJob};
/// async fn async_message_handler_fn(_job: CurrentJob) -> color_eyre::Result<()> {
///     Ok(())
/// }
///
/// job_registry!(JobRegistry, {
///     Ident: "message_name" => async_message_handler_fn,
/// });
/// ```
#[macro_export]
macro_rules! job_registry {
	(
		$reg_name:ident,
		{$($msg_fn_name:ident: $msg_name:literal => $msg_fn:path),*$(,)?}
	) => {
		#[doc = "Job Registry"]
		#[derive(Debug, Clone, Copy, PartialEq, Eq)]
		pub enum $reg_name {
			$(
				#[doc = concat!("`", $msg_name, "` leading to `", stringify!($msg_fn), "`.")]
				#[allow(non_camel_case_types, reason = "User provided & specific task identifiers")]
				$msg_fn_name
			),*
		}

		impl $crate::JobRegister for $reg_name {
			#[inline]
			fn builder(self) -> $crate::JobBuilder {
				match self {
					$(Self::$msg_fn_name => $crate::JobBuilder::new($msg_name)),*
				}
			}

			/// Return the message name of this message/job
			#[inline]
			fn name(&self) -> &'static str {
				match *self {
					$(Self::$msg_fn_name => $msg_name),*
				}
			}

			/// Get the registry entry based on message name. Returns None if not found.
			#[inline]
			fn from_name(name: &str) -> Option<Self> {
				match name {
					$($msg_name => Some(Self::$msg_fn_name)),*,
					_ => None,
				}
			}

			/// Return the function of this message/job
			#[inline]
			fn function(&self) -> $crate::JobFunctionType {
				match *self {
					$(Self::$msg_fn_name => Box::new(|job| Box::pin(async move {
						$msg_fn(job).await.map_err(Into::into)
					}))),*
				}
			}
		}
	};
}

/// Test correct macro type referencing and implementation. See if it compiles.
#[cfg(test)]
mod tests {
	#![allow(clippy::expect_used, unused_qualifications, clippy::unused_async, reason = "Tests")]

	use color_eyre::Result;

	use super::*;
	use crate::job_registry;

	job_registry!(JobRegistry, {
		some_fn: "cats" => some_fn,
		OtherFn: "foxes" => self::some_other_fn,
	});

	async fn some_fn(_job: CurrentJob) -> Result<()> {
		Ok(())
	}
	async fn some_other_fn(_job: CurrentJob) -> Result<(), Box<dyn Error + Send + Sync>> {
		Ok(())
	}

	#[test]
	fn test_job_registry() {
		let name = JobRegistry::some_fn.name();
		assert_eq!(name, "cats");

		let _function = JobRegistry::from_name("foxes").expect("name was set").function();

		let _builder = JobRegistry::some_fn.builder();
	}
}
