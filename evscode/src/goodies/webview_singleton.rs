//! Container for a singleton webview used by many threads.

use crate::{goodies::WebviewHandle, Webview, R};
use futures::lock::Mutex;
use std::{future::Future, pin::Pin, sync::Arc};

/// Function that creates the webview.
pub type Creator = fn() -> R<Webview>;
/// Worker function that handles the messages received from the webview.
pub type Manager = fn(WebviewHandle) -> Pin<Box<dyn Future<Output=R<()>>+Send>>;

/// State of the webview.
pub struct WebviewSingleton {
	create: Creator,
	manage: Manager,
	container: Mutex<Option<Arc<Mutex<Webview>>>>,
}

impl WebviewSingleton {
	/// Create a new instance of the webview
	pub fn new(create: Creator, manage: Manager) -> WebviewSingleton {
		WebviewSingleton { create, manage, container: Mutex::new(None) }
	}

	/// Get a webview handle, creating it if it does not exist or was closed.
	pub async fn handle(&'static self) -> R<Arc<Mutex<Webview>>> {
		let mut container_lock = self.container.lock().await;
		let view = if let Some(view) = &*container_lock {
			view.clone()
		} else {
			let view = (self.create)()?;
			let handle = Arc::new(Mutex::new(view));
			let handle2 = handle.clone();
			crate::runtime::spawn(async move {
				(self.manage)(handle2).await?;
				let mut container_lock = self.container.lock().await;
				*container_lock = None;
				Ok(())
			});
			*container_lock = Some(handle.clone());
			handle
		};
		Ok(view)
	}
}
