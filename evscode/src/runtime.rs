//! Runtime used by Evscode to manage communicating with VS Code

use crate::{internal::executor::CONFIG_ENTRIES, meta::ConfigEntry, R};
use futures::executor::block_on;
use std::{future::Future, sync::Arc};

/// Spawn a thread. If the function fails, the error returned from the function will be displayed to the user.
pub fn spawn(f: impl FnOnce() -> R<()>+Send+'static) {
	crate::internal::executor::spawn(f)
}

/// Spawn an async operation in a new thread.
///
/// See [`spawn`] for details.
pub fn spawn_async(f: impl Future<Output=R<()>>+Send+'static) {
	spawn(move || block_on(f))
}

/// Returns a vector with metadata on all configuration entries in the plugin.
pub fn config_entries() -> Arc<&'static [ConfigEntry]> {
	CONFIG_ENTRIES.load().as_ref().unwrap().clone()
}
