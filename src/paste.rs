mod library;
mod logic;
mod piece_parse;

use crate::{dir, telemetry::TELEMETRY};
use evscode::{error::ResultExt, E, R};
use futures::executor::block_on;
use itertools::Itertools;
use logic::{Library, Piece};
use std::{path::PathBuf, time::SystemTime};

#[evscode::command(title = "ICIE Quick Paste", key = "alt+[")]
fn quick() -> R<()> {
	let _status = crate::STATUS.push("Copy-pasting");
	TELEMETRY.paste_quick.spark();
	let library = library::CACHED_LIBRARY.update()?;
	let piece_id = block_on(
		evscode::QuickPick::new()
			.match_on_all()
			.items(library.pieces.iter().sorted_by_key(|(_, piece)| &piece.name).map(|(id, piece)| {
				let mut item = evscode::quick_pick::Item::new(id.clone(), piece.name.clone());
				if let Some(description) = &piece.description {
					item = item.description(description.clone());
				}
				if let Some(detail) = &piece.detail {
					item = item.detail(detail.clone());
				}
				item
			}))
			.show(),
	)
	.ok_or_else(E::cancel)?;
	let context = block_on(query_context(&library))?;
	TELEMETRY.paste_quick_ok.spark();
	library.walk_graph(&piece_id, context)?;
	Ok(())
}

#[evscode::command(title = "ICIE Quick input struct", key = "alt+i")]
fn qistruct() -> R<()> {
	let _status = crate::STATUS.push("Qistructing");
	TELEMETRY.paste_qistruct.spark();
	let name = block_on(evscode::InputBox::new().prompt("Qistruct name").placeholder("Person").show()).ok_or_else(E::cancel)?;
	let mut members = Vec::new();
	while let Some(member) =
		block_on(evscode::InputBox::new().prompt(&format!("Qistruct member {}", members.len() + 1)).placeholder("int age").show())
	{
		if member.trim().is_empty() {
			break;
		}
		let i = member.rfind(' ').wrap("incorrect member syntax, should be e.g., int age")?;
		let typ = &member[..i];
		let ide = &member[i + 1..];
		members.push((typ.to_string(), ide.to_string()));
	}
	let mut code = format!("struct {} {{\n", name);
	for (typ, ide) in &members {
		code += &format!("\t{} {};\n", typ, ide);
	}
	code += &format!("\tfriend istream& operator>>(istream& in, {}& x) {{ return in", name);
	for (_, ide) in &members {
		code += &format!(" >> x.{}", ide);
	}
	code += "; }\n};";
	let piece = Piece {
		name: String::new(),
		description: None,
		detail: None,
		code,
		guarantee: format!("struct {} {{", name),
		dependencies: Vec::new(),
		parent: None,
		modified: SystemTime::now(),
	};
	let mut library = Library::new_empty();
	library.pieces.insert(String::from("__qistruct"), piece);
	let context = block_on(query_context(&library))?;
	library.walk_graph("__qistruct", context)?;
	Ok(())
}

async fn query_context(library: &Library) -> R<VscodePaste<'_>> {
	let solution = dir::solution()?;
	let text = evscode::query_document_text(&solution).await;
	let context = VscodePaste { solution, text, library };
	Ok(context)
}

pub struct VscodePaste<'a> {
	solution: PathBuf,
	text: String,
	library: &'a Library,
}
impl logic::PasteContext for VscodePaste<'_> {
	fn has(&mut self, piece_id: &str) -> bool {
		let piece = &self.library.pieces[piece_id];
		self.text.contains(&piece.guarantee)
	}

	fn paste(&mut self, piece_id: &str) -> R<()> {
		let (position, snippet) = self.library.place(piece_id, &self.text);
		block_on(evscode::edit_paste(&self.solution, &snippet, position));
		self.text = block_on(evscode::query_document_text(&self.solution));
		Ok(())
	}
}

fn qpaste_doc_error(s: impl AsRef<str>) -> String {
	format!("{}; see [quickpasting docs](https://github.com/pustaczek/icie#quickpasting-setup)", s.as_ref())
}
