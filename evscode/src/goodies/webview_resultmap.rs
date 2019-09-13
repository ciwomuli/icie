//! Associative webview collection that represents a set of computation results.

use crate::{future::BoxedFuture, goodies::WebviewHandle, Webview, R};
use futures::{future::BoxFuture, lock::Mutex};
use std::{collections::HashMap, future::Future, hash::Hash, sync::Arc};

/// Trait controlling the webview collection behaviour.
pub trait Computation: Send+Sync {
	/// Key type provided to the computation.
	type K: Eq+Hash+Clone+Send+Sync;
	/// Computation results intended to be displayed.
	type V: Send+Sync;
	/// Run the computation.
	fn compute<'a>(&'a self, source: &'a Self::K) -> BoxFuture<'a, R<Self::V>>;
	/// Create an empty webview for the results of a computation with the given key.
	fn create_empty_webview(&self, key: &Self::K) -> R<Webview>;
	/// Update a webview with given computation results.
	fn update<'a>(&'a self, _: &'a Self::K, report: &'a Self::V, webview: &'a Webview) -> BoxFuture<'a, R<()>>;
	/// Return a worker function which will handle the messages received from the webview.
	fn manage(&self, key: &Self::K, value: &Self::V, handle: WebviewHandle) -> R<BoxedFuture<'static, R<()>>>;
}

/// State of the webview collection.
pub struct WebviewResultmap<T: Computation> {
	computation: T,
	collection: Mutex<HashMap<T::K, WebviewHandle>>,
}

impl<T: Computation> WebviewResultmap<T> {
	/// Create a new instance of the webview collection.
	pub fn new(computation: T) -> WebviewResultmap<T> {
		WebviewResultmap { computation, collection: Mutex::new(HashMap::new()) }
	}

	/// Run the computation, update the view and return both the webview and the computed values.
	pub async fn get_force(&'static self, key: T::K) -> R<(WebviewHandle, T::V)> {
		let (handle, value) = self.raw_get(key, true).await?;
		Ok((handle, value.unwrap()))
	}

	/// Run the computation and create the view if it does not already exist.
	/// Return the associated webview.
	pub async fn get_lazy(&'static self, key: T::K) -> R<WebviewHandle> {
		Ok(self.raw_get(key, false).await?.0)
	}

	/// Select the webview that is currently active.
	pub async fn find_active(&self) -> Option<WebviewHandle> {
		let lck = self.collection.lock().await;
		for webview in lck.values() {
			let lock = webview.lock().await;
			if lock.is_active().await {
				return Some(webview.clone());
			}
		}
		None
	}

	/// Rerun the computation on all existing webviews and update them.
	pub async fn update_all(&'static self) {
		let lck = self.collection.lock().await;
		for k in lck.keys() {
			let k = k.clone();
			crate::runtime::spawn(async move {
				self.get_force(k).await?;
				Ok(())
			});
		}
	}

	async fn raw_get(&'static self, key: T::K, force: bool) -> R<(WebviewHandle, Option<T::V>)> {
		let mut collection = self.collection.lock().await;
		let (webview, value) = match collection.entry(key.clone()) {
			std::collections::hash_map::Entry::Vacant(e) => {
				let (handle, value) = self.make_new(&key).await?;
				e.insert(handle.clone());
				(handle, Some(value))
			},
			std::collections::hash_map::Entry::Occupied(e) => {
				if force {
					let handle = e.get().lock().await;
					let value = self.update_old(&key, &*handle).await?;
					(e.get().clone(), Some(value))
				} else {
					(e.get().clone(), None)
				}
			},
		};
		let webview_lock = webview.lock().await;
		drop(collection);
		drop(webview_lock);
		Ok((webview, value))
	}

	async fn make_new(&'static self, key: &T::K) -> R<(WebviewHandle, T::V)> {
		let value = self.computation.compute(key).await?;
		let webview = self.computation.create_empty_webview(&key)?;
		self.computation.update(key, &value, &webview).await?;
		let webview = Arc::new(Mutex::new(webview));
		let worker = self.computation.manage(key, &value, webview.clone())?;
		let key = key.clone();
		let handle = webview.clone();
		crate::runtime::spawn(async move {
			let resultmap: &'static WebviewResultmap<T> = self;
			let delayed_error = worker.await;
			let mut collection = resultmap.collection.lock().await;
			if let std::collections::hash_map::Entry::Occupied(e) = collection.entry(key) {
				if Arc::ptr_eq(e.get(), &handle) {
					e.remove_entry();
				}
			}
			delayed_error
		});
		Ok((webview, value))
	}

	fn update_old<'a>(&'static self, key: &'a T::K, webview: &'a Webview) -> impl Future<Output=R<T::V>>+'a {
		async move {
			let value = self.computation.compute(key).await?;
			self.computation.update(key, &value, webview).await?;
			Ok(value)
		}
	}
}
