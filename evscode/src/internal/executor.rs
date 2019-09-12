use crate::{error::Severity, future::BoxedFuture, meta::ConfigEntry, runtime::spawn_async, stdlib::message::Action, E, R};
use arc_swap::ArcSwapOption;
use futures::{stream::StreamExt, FutureExt};
use json::{object, JsonValue};
use lazy_static::lazy_static;
use log::LevelFilter;
use std::{
	collections::{HashMap, VecDeque}, path::PathBuf, ptr::null_mut, sync::{
		atomic::{AtomicPtr, AtomicU64, Ordering}, Arc, Mutex
	}, task::Waker
};
use tokio::io::{stdin, AsyncBufReadExt, BufReader};
use tokio_executor::current_thread::{CurrentThread, Handle};

pub fn execute(pkg: &'static mut crate::meta::Package) {
	set_panic_hook();
	let logger = crate::internal::logger::VSCodeLoger { blacklist: pkg.log_filters.iter().map(|(id, fil)| (*id, *fil)).collect() };
	log::set_boxed_logger(Box::new(logger)).expect("evscode::execute failed to set logger");
	log::set_max_level(LevelFilter::Trace);
	CONFIG_ENTRIES.store(Some(Arc::new(&pkg.configuration)));
	let mut runtime = CurrentThread::new();
	RUNTIME_HANDLE.store(Box::leak(Box::new(runtime.handle())) as *mut Handle, Ordering::SeqCst);
	let on_activate = pkg.on_activate.take();
	let on_deactivate = pkg.on_deactivate.take();
	runtime.spawn(comms_loop(pkg, on_activate, on_deactivate));
	runtime.run().expect("executor run error");
}

async fn comms_loop(
	pkg: &'static crate::meta::Package,
	mut on_activate: Option<BoxedFuture<'static, R<()>>>,
	mut on_deactivate: Option<BoxedFuture<'static, R<()>>>,
) {
	let mut lines = BufReader::new(stdin()).lines();
	while let Some(line) = lines.next().await {
		let line = line.expect("evscode::execute line read errored");
		let mut impulse = json::parse(&line).expect("evscode::execute malformed json");
		if impulse["tag"] == "async" {
			let aid = impulse["aid"].as_u64().expect("evscode::execute impulse .tag['async'] has no .aid[u64]");
			let value = impulse["value"].take();
			let mut lck2 = ASYNC_OPS2.lock().expect("evscode::execute ASYNC_OPS2 PoisonError");
			if let Some(entry) = lck2.get_mut(&aid) {
				entry.0 = Some(value);
				if let Some(waker) = entry.1.take() {
					waker.wake();
				}
			} else {
				drop(lck2);
				let mut lck3 = ASYNC_STREAMS.lock().expect("evscode::execute ASYNC_STREAMS PoisonError");
				if let Some(entry) = lck3.get_mut(&aid) {
					entry.0.push_front(value);
					if let Some(waker) = entry.1.as_ref() {
						waker.wake_by_ref();
					}
				}
			}
		} else if impulse["tag"] == "trigger" {
			let id = impulse["command_id"].as_str().expect("evscode::execute .tag['trigger'] has no .command_id[str]");
			let command = match pkg.commands.iter().find(|command| command.id.to_string() == id) {
				Some(command) => command,
				None => panic!("evscode::execute unknown command {:?}, known: {:?}", id, pkg.commands.iter().map(|cmd| cmd.id).collect::<Vec<_>>()),
			};
			spawn_async((command.trigger)());
		} else if impulse["tag"] == "config" {
			let tree = &impulse["tree"];
			let mut errors = Vec::new();
			for config in &pkg.configuration {
				let mut v = tree;
				for part in config.id.to_string().split('.').skip(1) {
					v = &v[part];
				}
				if let Err(e) = config.reference.update(v.clone()) {
					errors.push(format!("{}.{} ({})", pkg.identifier, config.id, e));
				}
			}
			if !errors.is_empty() {
				crate::E::error(errors.join(", ")).context("some configuration entries are invalid, falling back to defaults").emit();
			}
		} else if impulse["tag"] == "meta" {
			*WORKSPACE_ROOT.lock().unwrap() = impulse["workspace"].as_str().map(PathBuf::from);
			*EXTENSION_ROOT.lock().unwrap() = Some(PathBuf::from(impulse["extension"].as_str().unwrap()));
			if let Some(on_activate) = on_activate.take() {
				spawn_async(on_activate);
			}
		} else if impulse["tag"] == "dispose" {
			if let Some(on_deactivate) = on_deactivate.take() {
				spawn_async(on_deactivate.map(|r| {
					if let Err(e) = r {
						error_show(e);
					}
					kill();
					Ok(())
				}));
			} else {
				kill();
			}
		} else {
			send_object(object! {
				"tag" => "console_error",
				"message" => json::stringify(impulse),
			});
		}
	}
}

static RUNTIME_HANDLE: AtomicPtr<Handle> = AtomicPtr::new(null_mut());

pub(crate) fn runtime_handle() -> &'static Handle {
	// Safe, because we only write to RUNTIME_HANDLE once and the pointer comes from Box::leak.
	unsafe { &*(RUNTIME_HANDLE.load(Ordering::SeqCst) as *const Handle) }
}

pub fn send_object(obj: json::JsonValue) {
	let fmt = json::stringify(obj);
	println!("{}", fmt);
}

pub fn error_show(e: crate::E) {
	let should_show = match e.severity {
		Severity::Error => true,
		Severity::Cancel => false,
		Severity::Warning => true,
		Severity::Workflow => true,
	};
	if should_show {
		let mut log_msg = String::new();
		for reason in &e.reasons {
			log_msg += &format!("{}\n", reason);
		}
		for detail in &e.details {
			log_msg += &format!("{}\n", detail);
		}
		log_msg += &format!("\nContains {} extended log entries\n\n{:?}", e.extended.len(), e.backtrace);
		log::error!("{}", log_msg);
		for extended in &e.extended {
			log::info!("{}", extended);
		}
		let should_suggest_report = match e.severity {
			Severity::Error => true,
			Severity::Cancel => false,
			Severity::Warning => true,
			Severity::Workflow => false,
		};
		spawn_async(async move {
			let message =
				format!("{}{}", e.human(), if should_suggest_report { "; [report issue?](https://github.com/pustaczek/icie/issues)" } else { "" });
			let items = e
				.actions
				.iter()
				.enumerate()
				.map(|(i, action)| Action { id: i.to_string(), title: &action.title, is_close_affordance: false })
				.collect::<Vec<_>>();
			let mut msg = crate::Message::new(&message).error().items(items);
			if let Severity::Warning = e.severity {
				msg = msg.warning();
			}
			let choice = msg.show().await;
			if let Some(choice) = choice {
				let i: usize = choice.parse().unwrap();
				let action = e.actions.into_iter().nth(i).unwrap();
				action.trigger.await?;
			}
			Ok(())
		});
	}
}

fn kill() {
	send_object(object! {
		"tag" => "kill",
	});
}

pub(crate) static ASYNC_ID_FACTORY: IDFactory = IDFactory::new();
pub(crate) static HANDLE_FACTORY: IDFactory = IDFactory::new();

type PacketChannelAwait = (Option<JsonValue>, Option<Waker>);
pub(crate) type PacketChannelStream = (VecDeque<JsonValue>, Option<Waker>);

lazy_static! {
	pub(crate) static ref CONFIG_ENTRIES: ArcSwapOption<&'static [ConfigEntry]> = ArcSwapOption::new(None);
}

lazy_static! {
	pub(crate) static ref ASYNC_OPS2: Mutex<HashMap<u64, PacketChannelAwait>> = Mutex::new(HashMap::new());
	pub(crate) static ref ASYNC_STREAMS: Mutex<HashMap<u64, PacketChannelStream>> = Mutex::new(HashMap::new());
}

lazy_static! {
	pub(crate) static ref WORKSPACE_ROOT: Mutex<Option<PathBuf>> = Mutex::new(None);
	pub(crate) static ref EXTENSION_ROOT: Mutex<Option<PathBuf>> = Mutex::new(None);
}

fn set_panic_hook() {
	std::panic::set_hook(Box::new(move |info| {
		let payload = if let Some(payload) = info.payload().downcast_ref::<&str>() {
			(*payload).to_owned()
		} else if let Some(payload) = info.payload().downcast_ref::<String>() {
			payload.clone()
		} else {
			"...".to_owned()
		};
		let location = if let Some(location) = info.location() { format!("{}:{}", location.file(), location.line()) } else { "--:--".to_owned() };
		error_show(E::error(format!("ICIE panicked, {} at {}", payload, location)));
	}));
}

pub struct IDFactory {
	counter: AtomicU64,
}
impl IDFactory {
	pub const fn new() -> IDFactory {
		IDFactory { counter: AtomicU64::new(0) }
	}

	pub fn generate(&self) -> u64 {
		self.counter.fetch_add(1, Ordering::Relaxed)
	}
}
