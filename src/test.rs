pub mod view;

use crate::{build, ci, dir, telemetry::TELEMETRY, util, STATUS};
use evscode::{error::ResultExt, E, R};
use futures::{stream::Stream, SinkExt, StreamExt};
use std::{
	path::{Path, PathBuf}, time::Duration
};

#[derive(Debug)]
pub struct TestRun {
	in_path: PathBuf,
	out_path: PathBuf,
	outcome: ci::test::Outcome,
}
impl TestRun {
	pub fn success(&self) -> bool {
		self.outcome.success()
	}
}

/// The maximum time an executable can run before getting a Time Limit Exceeded verdict, specified in milliseconds. Leaving this empty(which denotes no limit) is not recommended, because this will cause stuck processes to run indefinitely, wasting system resources.
#[evscode::config]
static TIME_LIMIT: evscode::Config<Option<u64>> = Some(1500);

pub async fn run(main_source: &Option<PathBuf>) -> R<Vec<TestRun>> {
	let _status = STATUS.push("Testing");
	TELEMETRY.test_run.spark();
	let solution = build::build(main_source, &ci::cpp::Codegen::Debug, false).await?;
	let task = ci::task::Task { checker: crate::checker::get_checker().await?, environment: ci::exec::Environment { time_limit: time_limit() } };
	let test_dir = dir::tests()?;
	let ins = ci::scan::scan_and_order(&test_dir);
	let mut runs = Vec::new();
	let test_count = ins.len();
	let progress = evscode::Progress::new().title(util::fmt_verb("Testing", &main_source)).cancellable().show();
	let mut worker = run_thread(ins, task, solution);
	for _ in 0..test_count {
		let run = worker.next().await.wrap("did not ran all tests due to an internal panic")??;
		let name = run.in_path.strip_prefix(&test_dir).wrap("found test outside of test directory")?;
		progress.update_inc(
			100.0 / test_count as f64,
			format!("{} on `{}` in {}", run.outcome.verdict, name.display(), util::fmt_time_short(&run.outcome.time)),
		);
		runs.push(run);
	}
	Ok(runs)
}

pub fn time_limit() -> Option<Duration> {
	TIME_LIMIT.get().map(|ms| Duration::from_millis(ms as u64))
}

fn run_thread(ins: Vec<PathBuf>, task: ci::task::Task, solution: ci::exec::Executable) -> impl Stream<Item=R<TestRun>> {
	let (tx, rx) = futures::channel::mpsc::unbounded();
	evscode::runtime::spawn(async {
		let _status = STATUS.push("Executing");
		let mut tx = tx;
		let task = task;
		let solution = solution;
		for in_path in ins {
			let r = try {
				let out_path = in_path.with_extension("out");
				let alt_path = in_path.with_extension("alt.out");
				let input = util::fs_read_to_string(&in_path).await?;
				let output = match tokio::fs::read(&out_path).await.map(String::from_utf8) {
					Ok(Ok(output)) => Some(output),
					Ok(Err(e)) => return Err(E::from_std(e).context(format!("test output for {} is not valid utf8", in_path.display()))),
					Err(ref e) if e.kind() == std::io::ErrorKind::NotFound => None,
					Err(e) => return Err(E::from_std(e).context(format!("failed to read test out {}", out_path.display()))),
				};
				let alt = if alt_path.exists() { Some(util::fs_read_to_string(&alt_path).await?) } else { None };
				let outcome = ci::test::simple_test(&solution, &input, output.as_ref().map(String::as_str), alt.as_ref().map(|p| p.as_str()), &task)
					.await
					.map_err(|e| e.context("failed to run test"))?;
				let run = TestRun { in_path, out_path, outcome };
				if tx.send(Ok(run)).await.is_err() {
					break;
				}
			};
			match r {
				Ok(()) => (),
				Err(e) => {
					let _ = tx.send(Err(e)).await;
				},
			}
		}
		Ok(())
	});
	rx
}

#[evscode::command(title = "ICIE Open Test View", key = "alt+0")]
pub async fn view() -> R<()> {
	TELEMETRY.test_alt0.spark();
	view::manage::COLLECTION.get_force(None).await?;
	Ok(())
}

#[evscode::command(title = "ICIE Open Test View (current editor)", key = "alt+\\ alt+0")]
async fn view_current() -> R<()> {
	TELEMETRY.test_current.spark();
	view::manage::COLLECTION.get_force(util::active_tab().await?).await?;
	Ok(())
}

async fn add(input: &str, desired: &str) -> R<()> {
	TELEMETRY.test_add.spark();
	let tests = dir::custom_tests()?;
	util::fs_create_dir_all(&tests).await?;
	let id = unused_test_id(&tests)?;
	let in_path = tests.join(format!("{}.in", id));
	let out_path = tests.join(format!("{}.out", id));
	util::fs_write(&in_path, input).await?;
	util::fs_write(&out_path, desired).await?;
	view::manage::COLLECTION.update_all().await;
	Ok(())
}

#[evscode::command(title = "ICIE New Test", key = "alt+-")]
pub async fn input() -> evscode::R<()> {
	TELEMETRY.test_input.spark();
	let view = if let Some(view) = view::manage::COLLECTION.find_active().await { view } else { view::manage::COLLECTION.get_lazy(None).await? };
	let view = view.lock().await;
	// FIXME: Despite this reveal, VS Code does not focus the webview hard enough for a .focus() in the JS code to work.
	view.reveal(2, false);
	view::manage::touch_input(&*view);
	Ok(())
}

fn unused_test_id(dir: &Path) -> evscode::R<i64> {
	let mut taken = std::collections::HashSet::new();
	for test in dir.read_dir().wrap("failed to read tests directory")? {
		let test = test.wrap("failed to read a test file entry in tests directory")?;
		if let Ok(id) = test.path().file_stem().unwrap().to_str().unwrap().parse::<i64>() {
			taken.insert(id);
		}
	}
	let mut id = 1;
	while taken.contains(&id) {
		id += 1;
	}
	Ok(id)
}
