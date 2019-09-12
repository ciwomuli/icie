mod comms;
pub mod manage;
mod render;

use evscode::R;

#[evscode::command(title = "ICIE Discover", key = "alt+9")]
async fn open() -> R<()> {
	let handle = manage::WEBVIEW.handle().await?;
	let lck = handle.lock().await;
	lck.reveal(1, false);
	Ok(())
}
