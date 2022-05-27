//! Utility and helper functionality for the library.

use std::ops::{Deref, DerefMut};

use tokio::task::JoinHandle;

/// [`JoinHandle`] that is aborted on [`Drop`].
#[derive(Debug)]
pub struct AbortOnDropHandle<R>(pub JoinHandle<R>);

impl<R> Drop for AbortOnDropHandle<R> {
	fn drop(&mut self) {
		self.0.abort();
	}
}

impl<R> From<JoinHandle<R>> for AbortOnDropHandle<R> {
	fn from(jh: JoinHandle<R>) -> Self {
		Self(jh)
	}
}

impl<R> Deref for AbortOnDropHandle<R> {
	type Target = JoinHandle<R>;

	fn deref(&self) -> &Self::Target {
		&self.0
	}
}

impl<R> DerefMut for AbortOnDropHandle<R> {
	fn deref_mut(&mut self) -> &mut Self::Target {
		&mut self.0
	}
}
