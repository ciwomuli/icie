use crate::{
	auth, telemetry::{self, TELEMETRY}
};
use evscode::{error::ResultExt, E, R};
use reqwest::header::HeaderValue;
use std::{fmt, future::Future, pin::Pin, thread::sleep, time::Duration};
use unijudge::{
	boxed::{BoxedSession, BoxedURL, DynamicBackend}, Backend, Resource, URL
};

const USER_AGENT: &str = concat!("ICIE/", env!("CARGO_PKG_VERSION"), " (+https://github.com/pustaczek/icie)");
const NETWORK_ERROR_RETRY_LIMIT: usize = 4;
const NETWORK_ERROR_RETRY_DELAY: Duration = Duration::from_secs(5);

pub static BACKENDS: [BackendMeta; 5] = [
	BackendMeta::new(&unijudge_atcoder::AtCoder, "C++14 (GCC 5.4.1)", "unijudge_atcoder"),
	BackendMeta::new(&unijudge_codechef::CodeChef, "C++14(gcc 6.3)", "unijudge_codechef"),
	BackendMeta::new(&unijudge_codeforces::Codeforces, "GNU G++17 7.3.0", "unijudge_codeforces"),
	BackendMeta::new(&unijudge_sio2::Sio2, "C++", "unijudge_sio2"),
	BackendMeta::new(&unijudge_spoj::SPOJ, "C++14 (clang 8.0)", "unijudge_spoj"),
];

pub struct Session {
	pub backend: &'static dyn DynamicBackend,
	pub session: BoxedSession,
	site: String,
	domain: String,
}

pub struct BackendMeta {
	pub backend: &'static dyn DynamicBackend,
	pub cpp: &'static str,
	pub telemetry_id: &'static str,
	pub counter: telemetry::Counter,
}

impl BackendMeta {
	const fn new(backend: &'static dyn DynamicBackend, cpp: &'static str, telemetry_id: &'static str) -> BackendMeta {
		BackendMeta { backend, cpp, telemetry_id, counter: telemetry::Counter::new() }
	}
}

pub fn interpret_url(url: &str) -> R<(BoxedURL, &'static BackendMeta)> {
	Ok(BACKENDS
		.iter()
		.filter_map(|backend| match backend.backend.deconstruct_url(url) {
			Ok(Some(url)) => {
				backend.counter.spark();
				Some(Ok((url, backend)))
			},
			Ok(None) => None,
			Err(e) => Some(Err(e)),
		})
		.next()
		.wrap(format!("not yet supporting contests/tasks on site {}", url))?
		.map_err(from_unijudge_error)?)
}

impl Session {
	pub fn connect(domain: &str, backend: &'static dyn DynamicBackend) -> R<Session> {
		TELEMETRY.net_connect.spark();
		let client = reqwest::ClientBuilder::new()
			.cookie_store(true)
			.default_headers(vec![(reqwest::header::USER_AGENT, HeaderValue::from_str(USER_AGENT).unwrap())].into_iter().collect())
			.build()
			.map_err(|e| from_unijudge_error(unijudge::Error::TLSFailure(e)))?;
		let session = backend.connect(client, domain);
		let site = format!("https://{}", domain);
		if let Some(auth) = auth::get_if_cached(&site) {
			if let Ok(auth) = backend.auth_deserialize(&auth) {
				log::debug!("cached auth found for {}, {:?}", domain, auth);
				match backend.auth_restore(&session, &auth) {
					Err(unijudge::Error::WrongData) | Err(unijudge::Error::WrongCredentials) | Err(unijudge::Error::AccessDenied) => Ok(()),
					Err(e) => Err(from_unijudge_error(e)),
					Ok(()) => Ok(()),
				}?;
			} else {
				log::warn!("cached auth is malformed for {}, {:?}", domain, auth);
			}
		} else {
			log::debug!("cached auth not available for {}", domain);
		}
		Ok(Session { backend, session, site, domain: domain.to_owned() })
	}

	pub async fn run<Y>(&self, mut f: impl FnMut(&'static dyn DynamicBackend, &BoxedSession) -> unijudge::Result<Y>) -> R<Y> {
		let mut retries_left = NETWORK_ERROR_RETRY_LIMIT;
		loop {
			match f(self.backend, &self.session) {
				Ok(y) => break Ok(y),
				Err(e @ unijudge::Error::WrongCredentials) | Err(e @ unijudge::Error::AccessDenied) => {
					log::debug!("access denied for {}, trying to log in {:?}", self.domain, e);
					self.maybe_error_show(e);
					let (username, password) = auth::get_cached_or_ask(&self.site).await?;
					self.login(&username, &password).await?
				},
				Err(unijudge::Error::NetworkFailure(e)) if retries_left > 0 => self.wait_for_retry(&mut retries_left, e),
				Err(e) => break Err(from_unijudge_error(e)),
			}
		}
	}

	pub async fn login(&self, username: &str, password: &str) -> R<()> {
		let mut retries_left = NETWORK_ERROR_RETRY_LIMIT;
		match self.backend.auth_login(&self.session, &username, &password) {
			Ok(()) => {
				log::debug!("login successful for {}, trying to cache session", self.domain);
				if let Some(cache) = self.backend.auth_cache(&self.session).map_err(from_unijudge_error)? {
					log::debug!("caching session for {}, {:?}", self.domain, cache);
					auth::save_cache(&self.site, &self.backend.auth_serialize(&cache).map_err(from_unijudge_error)?);
				} else {
					log::warn!("could not cache session for {} even though login succeded", self.domain);
				}
			},
			Err(e @ unijudge::Error::WrongData) | Err(e @ unijudge::Error::WrongCredentials) | Err(e @ unijudge::Error::AccessDenied) => {
				log::warn!("login failure for {}, {:?}", self.domain, e);
				self.maybe_error_show(e);
				self.force_login_boxed().await?;
			},
			Err(unijudge::Error::NetworkFailure(e)) if retries_left > 0 => self.wait_for_retry(&mut retries_left, e),
			Err(e) => return Err(from_unijudge_error(e)),
		}
		Ok(())
	}

	pub async fn force_login(&self) -> R<()> {
		let (username, password) = auth::get_force_ask(&self.site).await?;
		self.login(&username, &password).await
	}

	fn force_login_boxed<'a>(&'a self) -> Pin<Box<dyn Future<Output=R<()>>+Send+Sync+'a>> {
		Box::pin(self.force_login())
	}

	fn maybe_error_show(&self, e: unijudge::Error) {
		if let unijudge::Error::WrongCredentials = e {
			evscode::Message::new("Wrong username or password").error().show_detach();
		}
	}

	fn wait_for_retry(&self, retries_left: &mut usize, e: reqwest::Error) {
		assert!(*retries_left > 0);
		let _status = crate::STATUS.push("Waiting to retry");
		if *retries_left == NETWORK_ERROR_RETRY_LIMIT {
			from_unijudge_error(unijudge::Error::NetworkFailure(e))
				.context(format!("retrying in {} seconds", NETWORK_ERROR_RETRY_DELAY.as_secs_f64()))
				.warning()
				.emit();
		}
		*retries_left -= 1;
		sleep(NETWORK_ERROR_RETRY_DELAY);
	}
}

pub fn require_task<C: fmt::Debug, T: fmt::Debug>(url: URL<C, T>) -> R<URL<!, T>> {
	match url.resource {
		Resource::Task(t) => Ok(URL { domain: url.domain, site: url.site, resource: Resource::Task(t) }),
		_ => Err(E::error(format!("expected task url, found {:?}", url.resource))),
	}
}
pub fn require_contest<C: fmt::Debug, T: fmt::Debug>(url: URL<C, T>) -> R<URL<C, !>> {
	match url.resource {
		Resource::Contest(c) => Ok(URL { domain: url.domain, site: url.site, resource: Resource::Contest(c) }),
		_ => Err(E::error(format!("expected contest url, found {:?}", url.resource))),
	}
}

fn from_unijudge_error(e: unijudge::Error) -> evscode::E {
	match e {
		unijudge::Error::WrongCredentials => E::from_std(e).reform("wrong username or password"),
		unijudge::Error::WrongData => E::from_std(e).reform("wrong data passed to API"),
		unijudge::Error::WrongTaskUrl => E::from_std(e).reform("wrong task URL format"),
		unijudge::Error::AccessDenied => E::from_std(e).reform("access denied"),
		unijudge::Error::NotYetStarted => E::from_std(e).reform("contest not yet started"),
		unijudge::Error::RateLimit => E::from_std(e).reform("too frequent requests to site"),
		unijudge::Error::NetworkFailure(e) => E::from_std(e).context("network error"),
		unijudge::Error::TLSFailure(e) => E::from_std(e).context("TLS encryption error"),
		unijudge::Error::URLParseFailure(e) => E::from_std(e).context("URL parse error"),
		unijudge::Error::StateCorruption => E::from_std(e).context("broken state"),
		unijudge::Error::UnexpectedHTML(e) => {
			TELEMETRY.error_unijudge.spark();
			let mut extended = Vec::new();
			if !e.snapshots.is_empty() {
				extended.push(e.snapshots.last().unwrap().clone());
			}
			evscode::E {
				severity: evscode::error::Severity::Error,
				reasons: vec![format!("unexpected HTML structure ({:?} at {:?})", e.reason, e.operations)],
				details: Vec::new(),
				actions: Vec::new(),
				backtrace: e.backtrace,
				extended,
			}
		},
		unijudge::Error::UnexpectedJSON { endpoint, backtrace, resp_raw, inner } => {
			TELEMETRY.error_unijudge.spark();
			let message = format!("unexpected JSON response at {}", endpoint);
			let mut e = match inner {
				Some(inner) => E::from_std_ref(inner.as_ref()).context(message),
				None => E::error(message),
			};
			e.backtrace = backtrace;
			e.extended.push(resp_raw);
			e
		},
		unijudge::Error::UnexpectedResponse { endpoint, message, backtrace, resp_raw, inner } => {
			TELEMETRY.error_unijudge.spark();
			let mut e = match inner {
				Some(inner) => E::from_std_ref(inner.as_ref()).context(message),
				None => E::error(message),
			}
			.context(format!("unexpected site response at {}", endpoint));
			e.backtrace = backtrace;
			e.extended.push(resp_raw);
			e
		},
	}
}
