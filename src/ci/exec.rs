use evscode::{error::ResultExt, E, R};
use futures::channel::oneshot;
use std::{
	io::{self, Read, Write}, path::PathBuf, process::{Command, ExitStatus, Stdio}, time::{Duration, Instant}
};
use wait_timeout::ChildExt;

#[derive(Debug)]
pub struct Environment {
	pub time_limit: Option<Duration>,
}
#[derive(Debug, Eq, PartialEq)]
pub enum ExitKind {
	Normal,
	TimeLimitExceeded,
}
#[derive(Debug)]
pub struct Run {
	pub stdout: String,
	pub stderr: String,
	pub status: ExitStatus,
	pub exit_kind: ExitKind,
	pub time: Duration,
}

impl Run {
	pub fn success(&self) -> bool {
		self.status.success() && self.exit_kind == ExitKind::Normal
	}
}

#[derive(Debug, Clone)]
pub struct Executable {
	pub path: PathBuf,
}
impl Executable {
	pub fn new(path: PathBuf) -> Executable {
		Executable { path }
	}

	pub async fn run(&self, input: &str, args: &[&str], environment: &Environment) -> R<Run> {
		let mut cmd = Command::new(&self.path);
		cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).args(args);
		let t1 = Instant::now();
		let mut kid = cmd.spawn().wrap(format!("failed to execute {:?}", self.path))?;
		let _ = kid.stdin.as_mut().unwrap().write_all(input.as_bytes());
		let _ = kid.stdin.as_mut().unwrap().flush();
		let kid_stdout = capture(kid.stdout.take().unwrap());
		let kid_stderr = capture(kid.stderr.take().unwrap());
		let (status, exit_kind) = if let Some(time_limit) = environment.time_limit {
			if let Some(status) = kid.wait_timeout(time_limit).wrap(format!("lost child process of {:?}", self.path))? {
				(status, ExitKind::Normal)
			} else {
				kid.kill().wrap(format!("could not kill process of {:?} after time limit", self.path))?;
				(kid.wait().wrap(format!("lost child process of {:?}", self.path))?, ExitKind::TimeLimitExceeded)
			}
		} else {
			(kid.wait().wrap(format!("lost child process of {:?}", self.path))?, ExitKind::Normal)
		};
		let t2 = Instant::now();
		Ok(Run {
			stdout: kid_stdout
				.await
				.unwrap()
				.map_err(|e| E::from_std(e).context(format!("could not extract stdout of process of {:?}", self.path)))?,
			stderr: kid_stderr
				.await
				.unwrap()
				.map_err(|e| E::from_std(e).context(format!("could not extract stderr of process of {:?}", self.path)))?,
			status,
			exit_kind,
			time: t2 - t1,
		})
	}
}

fn capture(mut r: impl Read+Send+'static) -> oneshot::Receiver<io::Result<String>> {
	let (tx, rx) = oneshot::channel();
	evscode::runtime::spawn_async(async move {
		let mut buf = String::new();
		tx.send(match r.read_to_string(&mut buf) {
			Ok(_) => Ok(buf),
			Err(e) => Err(e),
		})
		.unwrap();
		Ok(())
	});
	rx
}
