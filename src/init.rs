use crate::{
	interpolation::Interpolation, net::{self, BackendMeta}, telemetry::TELEMETRY, util::{self, fs_create_dir_all}
};
use evscode::{quick_pick, QuickPick, E, R};
use futures::executor::block_on;
use std::{
	path::{Path, PathBuf}, sync::Arc
};
use unijudge::{
	boxed::{BoxedContestURL, BoxedTaskURL}, chrono::Local, Backend, Resource, TaskDetails, URL
};

pub mod contest;
mod files;
pub mod names;
mod scan;

/// The name of the code template used for initializing new projects. The list of code templates' names and paths can be found under the icie.template.list configuration entry.
#[evscode::config]
static SOLUTION_TEMPLATE: evscode::Config<String> = "C++";

#[evscode::command(title = "ICIE Init Scan", key = "alt+f9")]
fn scan() -> R<()> {
	TELEMETRY.init_scan.spark();
	let mut contests = scan::fetch_contests();
	contests.sort_by_key(|contest| contest.1.start);
	let pick = block_on(
		QuickPick::new()
			.items(contests.iter().enumerate().map(|(index, (sess, contest, _))| {
				let site_prefix = sess.backend.contest_site_prefix();
				let label = if contest.title.starts_with(site_prefix) { contest.title.clone() } else { format!("{} {}", site_prefix, contest.title) };
				let start = contest.start.with_timezone(&Local).to_rfc2822();
				quick_pick::Item::new(index.to_string(), label).description(start)
			}))
			.match_on_description()
			.ignore_focus_out()
			.show(),
	)
	.ok_or_else(E::cancel)?;
	let (sess, contest, backend) = &contests[pick.parse::<usize>().unwrap()];
	backend.counter.spark();
	TELEMETRY.init_scan_ok.spark();
	contest::sprint(sess.clone(), &contest.id, Some(&contest.title))?;
	Ok(())
}

#[evscode::command(title = "ICIE Init URL", key = "alt+f11")]
fn url() -> R<()> {
	let _status = crate::STATUS.push("Initializing");
	TELEMETRY.init_url.spark();
	let raw_url = ask_url()?;
	match url_to_command(raw_url.as_ref())? {
		InitCommand::Task(url) => {
			TELEMETRY.init_url_task.spark();
			let meta = url.map(|(url, backend)| fetch_task_details(url, backend)).transpose()?;
			let root = block_on(names::design_task_name(&*crate::dir::PROJECT_DIRECTORY.get(), meta.as_ref()))?;
			let dir = util::TransactionDir::new(&root)?;
			init_task(&root, raw_url, meta)?;
			dir.commit();
			evscode::open_folder(root, false);
		},
		InitCommand::Contest { url, backend } => {
			TELEMETRY.init_url_contest.spark();
			let sess = net::Session::connect(&url.domain, backend.backend)?;
			let Resource::Contest(contest) = url.resource;
			contest::sprint(Arc::new(sess), &contest, None)?;
		},
	}
	Ok(())
}

#[evscode::command(title = "ICIE Init URL (current directory)")]
fn url_existing() -> R<()> {
	let _status = crate::STATUS.push("Initializing");
	TELEMETRY.init_url_existing.spark();
	let raw_url = ask_url()?;
	let url = match url_to_command(raw_url.as_ref())? {
		InitCommand::Task(task) => task,
		InitCommand::Contest { .. } => return Err(E::error("it is forbidden to init a contest in an existing directory")),
	};
	let meta = url.map(|(url, backend)| fetch_task_details(url, backend)).transpose()?;
	let root = evscode::workspace_root()?;
	init_task(&root, raw_url, meta)?;
	Ok(())
}

fn ask_url() -> R<Option<String>> {
	Ok(block_on(
		evscode::InputBox::new()
			.prompt("Enter task/contest URL or leave empty")
			.placeholder("https://codeforces.com/contest/.../problem/...")
			.ignore_focus_out()
			.show(),
	)
	.map(|url| if url.trim().is_empty() { None } else { Some(url) })
	.ok_or_else(E::cancel)?)
}

#[allow(unused)]
enum InitCommand {
	Task(Option<(BoxedTaskURL, &'static BackendMeta)>),
	Contest { url: BoxedContestURL, backend: &'static BackendMeta },
}
fn url_to_command(url: Option<&String>) -> R<InitCommand> {
	Ok(match url {
		Some(raw_url) => {
			let (URL { domain, site, resource }, backend) = net::interpret_url(&raw_url)?;
			match resource {
				Resource::Task(task) => InitCommand::Task(Some((URL { domain, site, resource: Resource::Task(task) }, backend))),
				Resource::Contest(contest) => InitCommand::Contest { url: URL { domain, site, resource: Resource::Contest(contest) }, backend },
			}
		},
		None => InitCommand::Task(None),
	})
}

fn fetch_task_details(url: BoxedTaskURL, backend: &'static BackendMeta) -> R<TaskDetails> {
	let Resource::Task(task) = &url.resource;
	let sess = net::Session::connect(&url.domain, backend.backend)?;
	let meta = {
		let _status = crate::STATUS.push("Fetching task");
		sess.run(|backend, sess| backend.task_details(sess, &task))?
	};
	Ok(meta)
}

fn init_task(root: &Path, url: Option<String>, meta: Option<TaskDetails>) -> R<()> {
	let _status = crate::STATUS.push("Initializing");
	fs_create_dir_all(root)?;
	let examples = meta.as_ref().and_then(|meta| meta.examples.as_ref()).map(|examples| examples.as_slice()).unwrap_or(&[]);
	let statement = meta.as_ref().and_then(|meta| meta.statement.clone());
	files::init_manifest(root, &url, statement)?;
	files::init_template(root)?;
	files::init_examples(root, examples)?;
	Ok(())
}

pub fn help_init() -> R<()> {
	block_on(evscode::open_external("https://github.com/pustaczek/icie/blob/master/README.md#quick-start"))?;
	Ok(())
}

/// Default project directory name. This key uses special syntax to allow using dynamic content, like task names. Variable contest.title is not available in this context. See example list:
///
/// {task.symbol case.upper}-{task.name case.kebab} -> A-diverse-strings (default)
/// {random.cute}-{random.animal} -> kawaii-hedgehog
/// {site.short}/{contest.id case.kebab}/{task.symbol case.upper}-{task.name case.kebab} -> cf/1144/A-diverse-strings
/// {task.symbol case.upper}-{{ -> A-{
///
/// {random.cute} -> kawaii
/// {random.animal} -> hedgehog
/// {task.symbol} -> A
/// {task.name} -> Diverse Strings
/// {contest.id} -> 1144
/// {contest.title} -> Codeforces Round #550 (Div. 3)
/// {site.short} -> cf
///
/// {task.name} -> Diverse Strings
/// {task.name case.camel} -> diverseStrings
/// {task.name case.pascal} -> DiverseStrings
/// {task.name case.snake} -> diverse_strings
/// {task.name case.kebab} -> diverse-strings
/// {task.name case.upper} -> DIVERSE_STRINGS
#[evscode::config]
static PROJECT_NAME_TEMPLATE: evscode::Config<Interpolation<names::TaskVariable>> =
	"{task.symbol case.upper}-{task.name case.kebab}".parse().unwrap();

/// By default, when initializing a project, the project directory will be created in the directory determined by icie.dir.projectDirectory configuration entry, and the name will be chosen according to the icie.init.projectNameTemplate configuration entry. This option allows to instead specify the directory every time.
#[evscode::config]
static ASK_FOR_PATH: evscode::Config<PathDialog> = PathDialog::None;

#[derive(Debug, PartialEq, Eq, evscode::Configurable)]
enum PathDialog {
	#[evscode(name = "No")]
	None,
	#[evscode(name = "With a VS Code input box")]
	InputBox,
	#[evscode(name = "With a system dialog")]
	SystemDialog,
}

impl PathDialog {
	async fn query(&self, directory: &Path, codename: &str) -> R<PathBuf> {
		let basic = format!("{}/{}", directory.to_str().unwrap(), codename);
		match self {
			PathDialog::None => Ok(PathBuf::from(basic)),
			PathDialog::InputBox => Ok(PathBuf::from(
				evscode::InputBox::new()
					.ignore_focus_out()
					.prompt("New project directory")
					.value(&basic)
					.value_selection(basic.len() - codename.len(), basic.len())
					.show()
					.await
					.ok_or_else(E::cancel)?,
			)),
			PathDialog::SystemDialog => Ok(evscode::OpenDialog::new().directory().action_label("Init").show().await.ok_or_else(E::cancel)?),
		}
	}
}
