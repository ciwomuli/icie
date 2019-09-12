use crate::{
	build::build, ci::{cpp::Codegen, exec::Environment}, dir, discover::manage::add_test, telemetry::TELEMETRY, test::{
		self, time_limit, view::{render::render, SCROLL_TO_FIRST_FAILED, SKILL_ACTIONS}, TestRun
	}, util
};
use evscode::{
	error::{cancel_on, ResultExt}, goodies::{webview_resultmap::Computation, WebviewHandle}, Webview, WebviewResultmap, E, R
};
use futures::{future::BoxFuture, StreamExt};
use std::{
	fs, future::Future, path::{Path, PathBuf}, pin::Pin
};

lazy_static::lazy_static! {
	pub static ref COLLECTION: WebviewResultmap<TestViewLogic> = WebviewResultmap::new(TestViewLogic);
}

pub fn touch_input(webview: &Webview) {
	webview.post_message(json::object! {
		"tag" => "new_start",
	});
}

pub struct TestViewLogic;

impl Computation for TestViewLogic {
	type K = Option<PathBuf>;
	type V = Report;

	fn compute<'a>(&'a self, source: &'a Option<PathBuf>) -> BoxFuture<'a, R<Report>> {
		Box::pin(async move { Ok(Report { runs: test::run(source).await? }) })
	}

	fn create_empty_webview(&self, source: &Option<PathBuf>) -> R<Webview> {
		let title = util::fmt_verb("ICIE Test View", &source);
		let webview = evscode::Webview::new("icie.test.view", title, 2).enable_scripts().retain_context_when_hidden().create();
		Ok(webview)
	}

	fn update<'a>(&'a self, _: &'a Option<PathBuf>, report: &'a Report, webview: &'a Webview) -> BoxFuture<'a, R<()>> {
		Box::pin(async move {
			webview.set_html(render(&report.runs).await?);
			webview.reveal(2, true);
			if *SCROLL_TO_FIRST_FAILED.get() {
				webview.post_message(json::object! {
					"tag" => "scroll_to_wa",
				});
			}
			Ok(())
		})
	}

	fn manage(&self, source: &Option<PathBuf>, _: &Report, webview: WebviewHandle) -> R<Pin<Box<dyn Future<Output=R<()>>+Send+'static>>> {
		let source = source.clone();
		Ok(Box::pin(async move {
			let mut stream = {
				let webview = webview.lock().await;
				cancel_on(webview.listener(), webview.disposer()).boxed()
			};
			while let Some(note) = stream.next().await {
				let note = note?;
				match note["tag"].as_str() {
					Some("trigger_rr") => evscode::runtime::spawn_async({
						let source = source.clone();
						async move {
							let in_path = PathBuf::from(note["in_path"].as_str().unwrap());
							crate::debug::rr(in_path, source)
						}
					}),
					Some("trigger_gdb") => evscode::runtime::spawn_async({
						let source = source.clone();
						async move {
							let in_path = PathBuf::from(note["in_path"].as_str().unwrap());
							crate::debug::gdb(in_path, source)
						}
					}),
					Some("new_test") => evscode::runtime::spawn_async(async move {
						crate::test::add(note["input"].as_str().unwrap(), note["desired"].as_str().unwrap()).await
					}),
					Some("set_alt") => evscode::runtime::spawn_async({
						let source = source.clone();
						async move {
							TELEMETRY.test_alternative_add.spark();
							let in_path = PathBuf::from(note["in_path"].as_str().unwrap());
							let out = note["out"].as_str().unwrap();
							fs::write(in_path.with_extension("alt.out"), format!("{}\n", out.trim()))
								.wrap("failed to save alternative out as a file")?;
							COLLECTION.get_force(source).await?;
							Ok(())
						}
					}),
					Some("del_alt") => evscode::runtime::spawn_async({
						let source = source.clone();
						async move {
							TELEMETRY.test_alternative_delete.spark();
							let in_path = PathBuf::from(note["in_path"].as_str().unwrap());
							fs::remove_file(in_path.with_extension("alt.out")).wrap("failed to remove alternative out file")?;
							COLLECTION.get_force(source).await?;
							Ok(())
						}
					}),
					Some("edit") => {
						TELEMETRY.test_edit.spark();
						evscode::open_editor(Path::new(note["path"].as_str().unwrap())).open().await;
					},
					Some("action_notice") => SKILL_ACTIONS.add_use().await,
					Some("eval_req") => {
						let webview = webview.clone();
						evscode::runtime::spawn_async(async move {
							let _status = crate::STATUS.push("Evaluating");
							let id = note["id"].as_i64().unwrap();
							let input = note["input"].as_str().unwrap();
							if let Ok(brut) = dir::brut() {
								if brut.exists() {
									TELEMETRY.test_eval.spark();
									let brut = build(brut, &Codegen::Release, false).await?;
									let run = brut.run(input, &[], &Environment { time_limit: time_limit() }).await?;
									if run.success() {
										add_test(input, &run.stdout).await?;
										let webview = webview.lock().await;
										webview.post_message(json::object! {
											"tag" => "eval_resp",
											"id" => id,
											"input" => input,
										});
									} else {
										return Err(E::error("brut did not evaluate test successfully"));
									}
								}
							}
							Ok(())
						});
					},
					_ => return Err(E::error(format!("invalid webview message `{}`", note.dump()))),
				}
			}
			Ok(())
		}))
	}
}

pub struct Report {
	pub runs: Vec<TestRun>,
}
