use crate::internal::executor::{ASYNC_ID_FACTORY, ASYNC_OPS2};
use json::JsonValue;
use std::{
	collections::hash_map::Entry, future::Future, pin::Pin, task::{Context, Poll}
};

pub struct Pong {
	id: u64,
}

impl Pong {
	pub fn new() -> Pong {
		let id = ASYNC_ID_FACTORY.generate();
		ASYNC_OPS2.lock().unwrap().insert(id, (None, None));
		Pong { id }
	}

	pub fn aid(&self) -> u64 {
		self.id
	}
}

impl Future for Pong {
	type Output = JsonValue;

	fn poll(self: Pin<&mut Self>, cx: &mut Context) -> Poll<Self::Output> {
		match ASYNC_OPS2.lock().unwrap().entry(self.id) {
			Entry::Occupied(mut e) => match e.get_mut() {
				(Some(_), _) => Poll::Ready(e.remove().0.unwrap()),
				(None, Some(_)) => Poll::Pending,
				(None, waker @ None) => {
					*waker = Some(cx.waker().clone());
					Poll::Pending
				},
			},
			Entry::Vacant(_) => unreachable!(),
		}
	}
}

impl Drop for Pong {
	fn drop(&mut self) {
		ASYNC_OPS2.lock().unwrap().remove(&self.id);
	}
}
