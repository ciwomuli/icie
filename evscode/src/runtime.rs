//! Runtime used by Evscode to manage communicating with VS Code

use crate::{
	internal::executor::{error_show, runtime_handle, CONFIG_ENTRIES}, meta::ConfigEntry, R
};
use std::{future::Future, sync::Arc};

/// Spawn an async operation in a new thread.
///
/// See [`spawn`] for details.
pub fn spawn_async(f: impl Future<Output=R<()>>+Send+'static) {
	runtime_handle()
		.spawn(async move {
			match f.await {
				Ok(()) => (),
				Err(e) => error_show(e),
			}
		})
		.expect("internal error not being able to spawn task");
}

/// Returns a vector with metadata on all configuration entries in the plugin.
pub fn config_entries() -> Arc<&'static [ConfigEntry]> {
	CONFIG_ENTRIES.load().as_ref().unwrap().clone()
}
