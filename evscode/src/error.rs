//! Rich error typee, supporting cancellation, backtraces, automatic logging and followup actions.
//!
//! It should also be used by extensions instead of custom error types, because it supports follow-up actions, cancellations, hiding error details
//! from the user, backtraces and carrying extended logs. Properly connecting these features to VS Code API is a little bit code-heavy, and keeping
//! this logic inside Evscode allows to improve error message format across all extensions.

use backtrace::Backtrace;
use std::ops::Try;

/// Result type used for errors in Evscode. See [`E`] for details.
pub type R<T> = Result<T, E>;

/// A button on an error message that the user can press.
#[derive(Clone, Debug)]
pub struct Action {
	/// Title displayed to the user.
	/// Preferably one-word, because wider buttons look weird.
	pub title: String,
	/// The function that will be launched upon clicking the button.
	/// It will be called in a separate thread.
	pub trigger: fn() -> R<()>,
}

/// Indication of how serious the error is.
#[derive(Clone, Debug)]
pub enum Severity {
	/// Abort the operation, display an error message, provide a link to GitHub issues.
	Error,
	/// Abort the operation, do not display an error message or provide a link to GitHub issues.
	Cancel,
	/// Do not abort the operation, display a warning message, do not provide a link to GitHub issues.
	/// Do not use the `?` operator to avoid aborting the operation.
	Warning,
	/// Abort the operation, display an error message, do not provide a link to GitHub issues.
	Workflow,
}

/// Error type used by Evscode.
///
/// See [module documentation](index.html) for details.
#[derive(Clone, Debug)]
pub struct E {
	/// Marks whose fault this error is and how serious is it.
	pub severity: Severity,
	/// List of human-facing error messages, ordered from low-level to high-level.
	pub reasons: Vec<String>,
	/// List of error messages not meant for the end user, ordered from low-level to high level.
	/// These messages will not be displayed in the UI.
	pub details: Vec<String>,
	/// List of actions available as buttons in the message, if displayed.
	pub actions: Vec<Action>,
	/// Stack trace from either where the error was converted to [`E`] or from a lower-level error.
	/// The backtrace will only be converted from foreign errors if it is done manually.
	pub backtrace: Backtrace,
	/// List of extended error logs, presumably too long to be displayed to the end user.
	pub extended: Vec<String>,
}

impl E {
	/// Create an error from a user-facing string, capturing a backtrace.
	pub fn error(s: impl AsRef<str>) -> E {
		E {
			severity: Severity::Error,
			reasons: vec![s.as_ref().to_owned()],
			details: Vec::new(),
			actions: Vec::new(),
			backtrace: Backtrace::new(),
			extended: Vec::new(),
		}
	}

	/// Create an error representing an operation cancelled by user. This error will be logged, but not displayed to the user.
	pub fn cancel() -> E {
		E::from(Cancellation)
	}

	/// Convert an error implementing [`std::error::Error`] to an Evscode error. Error messages will be collected from [`std::fmt::Display`]
	/// implementations on each error in the [`std::error::Error::source`] chain.
	pub fn from_std(native: impl std::error::Error) -> E {
		let mut e = E {
			severity: Severity::Error,
			reasons: Vec::new(),
			details: Vec::new(),
			actions: Vec::new(),
			backtrace: Backtrace::new(),
			extended: Vec::new(),
		};
		e.reasons.push(format!("{}", native));
		let mut v: Option<&(dyn std::error::Error)> = native.source();
		while let Some(native) = v {
			e.reasons.push(format!("{}", native));
			v = native.source();
		}
		e.reasons.reverse();
		e
	}

	/// A short human-facing representation of the error.
	pub fn human(&self) -> String {
		let mut buf = String::new();
		for (i, reason) in self.reasons.iter().enumerate().rev() {
			buf += reason;
			if i != 0 {
				buf += "; ";
			}
		}
		buf
	}

	/// Add an additional message describing the error, which will be displayed in front of the previous ones.
	/// ```
	/// # use evscode::E;
	/// let e = E::error("DNS timed out").context("network failure").context("failed to fetch Bitcoin prices");
	/// assert_eq!(e.human(), "failed to fetch Bitcoin prices; network failure; DNS timed out");
	/// ```
	pub fn context(mut self, msg: impl AsRef<str>) -> Self {
		self.reasons.push(msg.as_ref().to_owned());
		self
	}

	/// Add an additional message describing the error and mark all previous message as not meant for the end user.
	/// This does not remove the lower-level messages, they will still be present in developer tools' logs.
	/// ```
	/// # use evscode::E;
	/// let e = E::error("entity not found").reform("file kitty.txt not found");
	/// assert_eq!(e.human(), "file kitty.txt not found");
	/// ```
	pub fn reform(mut self, msg: impl AsRef<str>) -> Self {
		self.details.extend_from_slice(&self.reasons);
		self.reasons.clear();
		self.reasons.push(msg.as_ref().to_owned());
		self
	}

	/// Add a follow-up action that can be taken by the user, who will see the action as a button on the error message.
	pub fn action(mut self, title: impl AsRef<str>, trigger: fn() -> R<()>) -> Self {
		self.actions.push(Action { title: title.as_ref().to_owned(), trigger });
		self
	}

	/// A convenience function to add a follow-up action if the condition is true. See [`E::action`] for details.
	pub fn action_if(self, cond: bool, title: impl AsRef<str>, trigger: fn() -> R<()>) -> Self {
		if cond { self.action(title, trigger) } else { self }
	}

	/// Add an extended error log, which typically is a multiline string, like a compilation log or a subprocess output.
	/// The log will be displayed as a seperate message in developer tools.
	pub fn extended(mut self, extended: impl AsRef<str>) -> Self {
		self.extended.push(extended.as_ref().to_owned());
		self
	}

	/// Mark the error as something common in extension's workflow, that does not need to be put on project's issue tracker.
	/// This will remove the "report issue?" suffix, which may irritate users in when the error is common.
	pub fn workflow_error(mut self) -> Self {
		self.severity = Severity::Workflow;
		self
	}
}

/// Error type representing a operation intentionally cancelled by the user.
pub struct Cancellation;

/// Result-like type for operations that could be intentionally cancelled by the user.
///
/// It implements [`std::ops::Try`], which makes it possible to use ? operator in functions returning [`R`].
pub struct Cancellable<T>(pub Option<T>);
impl<T> Try for Cancellable<T> {
	type Error = Cancellation;
	type Ok = T;

	fn into_result(self) -> Result<Self::Ok, Self::Error> {
		self.0.ok_or(Cancellation)
	}

	fn from_error(_: Self::Error) -> Self {
		Cancellable(None)
	}

	fn from_ok(v: Self::Ok) -> Self {
		Cancellable(Some(v))
	}
}

impl From<Cancellation> for E {
	fn from(_: Cancellation) -> Self {
		E {
			severity: Severity::Cancel,
			reasons: Vec::new(),
			details: Vec::new(),
			actions: Vec::new(),
			backtrace: Backtrace::new(),
			extended: Vec::new(),
		}
	}
}