use crate::{
	ci::{self, test::Outcome}, discover::{
		comms::{Food, Note}, render::render
	}, telemetry::TELEMETRY, util
};
use evscode::{
	error::{cancel_on, ResultExt}, E, R
};
use futures::{
	channel::mpsc, poll, stream::{select, StreamExt}, Poll, SinkExt, Stream
};
use std::{future::Future, pin::Pin};

fn webview_create() -> R<evscode::Webview> {
	Ok(evscode::Webview::new("icie.discover", "ICIE Discover", 1).enable_scripts().retain_context_when_hidden().create())
}

fn webview_manage_wrapper(handle: evscode::goodies::WebviewHandle) -> Pin<Box<dyn Future<Output=R<()>>+Send>> {
	Box::pin(webview_manage(handle))
}
async fn webview_manage(handle: evscode::goodies::WebviewHandle) -> R<()> {
	let (mut stream, mut worker_tx) = {
		let view = handle.lock().await;
		view.set_html(render());
		let (worker_tx, worker_rx) = mpsc::unbounded();
		let worker_reports = spawn_worker(worker_rx);
		let stream = cancel_on::<R<ManagerMessage>, _, _>(
			select(view.listener().map(|n| Ok(ManagerMessage::Note(Note::from(n)))), worker_reports.map(ManagerMessage::Report).map(Ok)),
			view.disposer(),
		)
		.boxed();
		(stream, worker_tx)
	};

	let mut best_fitness = None;
	let mut paused = false;

	while let Some(msg) = stream.next().await {
		let view = handle.lock().await;
		match msg?? {
			ManagerMessage::Note(note) => match note {
				Note::Start => {
					TELEMETRY.discover_start.spark();
					paused = false;
					worker_tx.send(WorkerOrder::Start).await.unwrap();
					view.post_message(Food::State { running: true, reset: false });
				},
				Note::Pause => {
					paused = true;
					worker_tx.send(WorkerOrder::Pause).await.unwrap();
					view.post_message(Food::State { running: false, reset: false });
				},
				Note::Reset => {
					best_fitness = None;
					paused = false;
					worker_tx.send(WorkerOrder::Reset).await.unwrap();
					view.post_message(Food::State { running: false, reset: true });
				},
				Note::Save { input } => {
					if !paused {
						paused = true;
						worker_tx.send(WorkerOrder::Pause).await.unwrap();
					}
					view.post_message(Food::State { running: false, reset: false });
					evscode::runtime::spawn(add_test_input(input));
				},
			},
			ManagerMessage::Report(report) => match report {
				Ok(row) => {
					let is_failed = !row.solution.verdict.success();
					let new_best = is_failed && best_fitness.map(|bf| bf < row.fitness).unwrap_or(true);
					if new_best {
						best_fitness = Some(row.fitness);
					}
					view.post_message(Food::Row {
						number: row.number,
						outcome: row.solution.verdict,
						fitness: row.fitness,
						input: if new_best { Some(row.input) } else { None },
					});
				},
				Err(e) => {
					best_fitness = None;
					paused = false;
					view.post_message(Food::State { running: false, reset: true });
					e.emit();
				},
			},
		}
	}
	Ok(())
}

fn spawn_worker(orders: mpsc::UnboundedReceiver<WorkerOrder>) -> impl Stream<Item=WorkerReport> {
	let (tx, rx) = futures::channel::mpsc::unbounded();
	evscode::runtime::spawn(async move {
		worker_thread(tx, orders).await;
		Ok(())
	});
	rx
}

async fn worker_thread(mut carrier: mpsc::UnboundedSender<WorkerReport>, mut orders: mpsc::UnboundedReceiver<WorkerOrder>) {
	loop {
		match orders.next().await {
			Some(WorkerOrder::Start) => (),
			Some(WorkerOrder::Pause) | Some(WorkerOrder::Reset) => continue,
			None => break,
		};
		// orders is moved like that, because mpsc::Receiver is non-Sync, so a reference to it is non-Send, which makes the whole future non-Send
		match worker_run(&mut carrier, orders).await {
			Ok(ret_orders) => orders = ret_orders,
			Err(e) => {
				let _ = carrier.send(Err(e)).await;
				break;
			},
		};
	}
}

async fn worker_run(
	carrier: &mut mpsc::UnboundedSender<WorkerReport>,
	mut orders: mpsc::UnboundedReceiver<WorkerOrder>,
) -> R<mpsc::UnboundedReceiver<WorkerOrder>> {
	let solution = crate::build::build(crate::dir::solution()?, &ci::cpp::Codegen::Debug, false).await?;
	let brut = crate::build::build(crate::dir::brut()?, &ci::cpp::Codegen::Release, false).await?;
	let gen = crate::build::build(crate::dir::gen()?, &ci::cpp::Codegen::Release, false).await?;
	let task = ci::task::Task {
		checker: crate::checker::get_checker().await?,
		environment: ci::exec::Environment { time_limit: crate::test::time_limit() },
	};
	let mut _status = crate::STATUS.push("Discovering");
	for number in 1.. {
		match poll!(orders.next()) {
			Poll::Ready(Some(WorkerOrder::Start)) => (),
			Poll::Ready(Some(WorkerOrder::Pause)) => {
				drop(_status);
				loop {
					match orders.next().await {
						Some(WorkerOrder::Start) => break,
						Some(WorkerOrder::Pause) => (),
						Some(WorkerOrder::Reset) => return Err(E::cancel()),
						None => return Err(E::cancel()),
					}
				}
				_status = crate::STATUS.push("Discovering");
			},
			Poll::Ready(Some(WorkerOrder::Reset)) => break,
			Poll::Ready(None) => return Err(E::cancel()),
			Poll::Pending => (),
		}
		let run_gen = gen.run("", &[], &task.environment).await.map_err(|e| e.context("failed to run the test generator"))?;
		if !run_gen.success() {
			return Err(E::error(format!("test generator failed {:?}", run_gen)));
		}
		let input = run_gen.stdout;
		let run_brut = brut.run(&input, &[], &task.environment).await.map_err(|e| e.context("failed to run slow solution"))?;
		if !run_brut.success() {
			return Err(E::error(format!("brut failed {:?}", run_brut)));
		}
		let desired = run_brut.stdout;
		let outcome =
			ci::test::simple_test(&solution, &input, Some(&desired), None, &task).await.map_err(|e| e.context("failed to run test in discover"))?;
		let fitness = -(input.len() as i64);
		let row = Row { number, solution: outcome, fitness, input };
		if carrier.send(Ok(row)).await.is_err() {
			break;
		}
	}
	Ok(orders)
}

async fn add_test_input(input: String) -> R<()> {
	let _status = crate::STATUS.push("Adding new test");
	let brut = crate::build::build(crate::dir::brut()?, &ci::cpp::Codegen::Release, false).await?;
	let run =
		brut.run(&input, &[], &ci::exec::Environment { time_limit: None }).await.map_err(|e| e.context("failed to generate output for the test"))?;
	if !run.success() {
		return Err(E::error("brut failed when generating output for the added test"));
	}
	let desired = run.stdout;
	add_test(&input, &desired).await?;
	Ok(())
}

pub async fn add_test(input: &str, output: &str) -> R<()> {
	let dir = crate::dir::custom_tests()?;
	util::fs_create_dir_all(&dir).await?;
	let used = std::fs::read_dir(&dir)
		.wrap("failed to read tests directory")?
		.map(|der| {
			der.ok()
				.and_then(|de| de.path().file_stem().map(std::ffi::OsStr::to_owned))
				.and_then(|stem| stem.to_str().map(str::to_owned))
				.and_then(|name| name.parse::<i64>().ok())
		})
		.filter_map(|o| o)
		.collect::<Vec<_>>();
	let id = crate::util::mex(1, used);
	let in_path = dir.join(format!("{}.in", id));
	let out_path = dir.join(format!("{}.out", id));
	util::fs_write(&in_path, input).await?;
	util::fs_write(&out_path, output).await?;
	crate::test::view().await?;
	Ok(())
}

#[derive(Debug)]
pub struct Row {
	pub number: usize,
	pub solution: Outcome,
	pub fitness: i64,
	pub input: String,
}

enum ManagerMessage {
	Note(Note),
	Report(WorkerReport),
}
enum WorkerOrder {
	Start,
	Pause,
	Reset,
}
type WorkerReport = R<Row>;

lazy_static::lazy_static! {
	pub static ref WEBVIEW: evscode::WebviewSingleton = evscode::WebviewSingleton::new(webview_create, webview_manage_wrapper);
}
