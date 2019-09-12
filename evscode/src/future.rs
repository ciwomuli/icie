use crate::internal::executor::{ASYNC_ID_FACTORY, ASYNC_OPS2, ASYNC_STREAMS};
use futures::Stream;
use json::JsonValue;
use std::{
	collections::{hash_map::Entry, VecDeque}, future::Future, pin::Pin, task::{Context, Poll}
};

pub type BoxedFuture<'a, T> = Pin<Box<dyn Future<Output=T>+Send+'a>>;

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

pub struct PongStream {
	id: u64,
}

impl PongStream {
	pub fn new() -> PongStream {
		let id = ASYNC_ID_FACTORY.generate();
		ASYNC_STREAMS.lock().unwrap().insert(id, (VecDeque::new(), None));
		PongStream { id }
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

impl Stream for PongStream {
	type Item = JsonValue;

	fn poll_next(self: Pin<&mut Self>, cx: &mut Context) -> Poll<Option<Self::Item>> {
		match ASYNC_STREAMS.lock().unwrap().entry(self.id) {
			Entry::Occupied(mut e) => match e.get_mut() {
				(que, _) if !que.is_empty() => Poll::Ready(Some(que.pop_front().unwrap())),
				(_, Some(_)) => Poll::Pending,
				(_, waker @ None) => {
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

impl Drop for PongStream {
	fn drop(&mut self) {
		ASYNC_STREAMS.lock().unwrap().remove(&self.id);
	}
}
