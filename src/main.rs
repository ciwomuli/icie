#![feature(const_fn, exhaustive_patterns, never_type, proc_macro_hygiene, specialization, todo_macro, try_blocks)]
#![allow(clippy::extra_unused_lifetimes, clippy::unit_arg)]

mod auth;
mod build;
mod checker;
mod ci;
mod debug;
mod dir;
mod discover;
mod init;
mod interpolation;
mod launch;
mod manifest;
mod net;
mod newsletter;
mod paste;
mod submit;
mod telemetry;
mod template;
mod term;
mod test;
mod tutorial;
mod util;

lazy_static::lazy_static! {
	pub static ref STATUS: evscode::StackedStatus = evscode::StackedStatus::new("❄️ ");
}

evscode::plugin! {
	name: "ICIE",
	publisher: "pustaczek",
	description: "Competitive programming IDE-as-a-VS-Code-plugin",
	keywords: &["competitive", "contest", "codeforces", "atcoder", "codechef"],
	categories: &["Other"],
	license: "GPL-3.0-only",
	repository: "https://github.com/pustaczek/icie",
	on_activate: Some(Box::pin(launch::activate())),
	on_deactivate: Some(Box::pin(launch::deactivate())),
	extra_activations: &[
		evscode::meta::Activation::WorkspaceContains { selector: ".icie" },
		evscode::meta::Activation::WorkspaceContains { selector: ".icie-contest" },
	],
	telemetry_key: "b05c4c82-d1e6-44f5-aa16-321230ad2475",
	log_filters: &[
		("cookie_store", log::LevelFilter::Info),
		("html5ever", log::LevelFilter::Error),
		("hyper", log::LevelFilter::Info),
		("mio", log::LevelFilter::Info),
		("reqwest", log::LevelFilter::Info),
		("rustls", log::LevelFilter::Info),
		("selectors", log::LevelFilter::Info),
		("tokio_reactor", log::LevelFilter::Info),
		("want", log::LevelFilter::Info),
	],
}
