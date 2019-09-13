use crate::ci::exec::{Environment, Executable};
use evscode::{error::ResultExt, R};
use futures::future::BoxFuture;
use std::{fmt, io::Write};
use tempfile::NamedTempFile;

pub trait Checker: fmt::Debug {
	fn judge<'a>(&'a self, input: &'a str, desired: &'a str, out: &'a str) -> BoxFuture<'a, R<bool>>;
}

#[derive(Debug)]
pub struct Task {
	pub checker: Box<dyn Checker+Send+Sync>,
	pub environment: Environment,
}

#[derive(Debug)]
pub struct FreeWhitespaceChecker;
impl Checker for FreeWhitespaceChecker {
	fn judge<'a>(&'a self, _input: &'a str, desired: &'a str, out: &'a str) -> BoxFuture<'a, R<bool>> {
		Box::pin(async move { Ok(self.equal_bew(desired, out)) })
	}
}
impl FreeWhitespaceChecker {
	fn equal_bew(&self, a: &str, b: &str) -> bool {
		let mut i = a.chars().peekable();
		let mut j = b.chars().peekable();
		while i.peek().is_some() && j.peek().is_some() {
			if i.peek().unwrap().is_whitespace() && j.peek().unwrap().is_whitespace() {
				while i.peek().map(|c| c.is_whitespace()).unwrap_or(false) {
					i.next();
				}
				while j.peek().map(|c| c.is_whitespace()).unwrap_or(false) {
					j.next();
				}
			} else {
				if i.peek() != j.peek() {
					return false;
				}
				i.next();
				j.next();
			}
		}
		for c in i {
			if !c.is_whitespace() {
				return false;
			}
		}
		for c in j {
			if !c.is_whitespace() {
				return false;
			}
		}
		true
	}
}

#[derive(Debug)]
pub struct ExecChecker {
	pub executable: Executable,
	pub environment: Environment,
}

impl Checker for ExecChecker {
	fn judge<'a>(&'a self, input: &'a str, desired: &'a str, out: &'a str) -> BoxFuture<'a, R<bool>> {
		Box::pin(async move {
			let mut input_file = NamedTempFile::new().wrap("failed to create temporary input file")?;
			let mut desired_file = NamedTempFile::new().wrap("failed to create temporary correct-output file")?;
			let mut out_file = NamedTempFile::new().wrap("failed to create temporary output file")?;
			input_file.write_all(input.as_bytes()).wrap("failed to fill temporary input file")?;
			desired_file.write_all(desired.as_bytes()).wrap("failed to fill temporary correct-output file")?;
			out_file.write_all(out.as_bytes()).wrap("failed to fill temporary output file")?;
			let args = [input_file.path().to_str().unwrap(), out_file.path().to_str().unwrap(), desired_file.path().to_str().unwrap()];
			let run = self.executable.run("", &args, &self.environment).await?;
			Ok(run.success())
		})
	}
}
