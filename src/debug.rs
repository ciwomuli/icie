use crate::{build, telemetry::TELEMETRY, term, util};
use evscode::{error::ResultExt, E, R};
use std::{
	fs::File, path::PathBuf, process::{Command, Stdio}
};

pub fn gdb(in_path: PathBuf, source: Option<PathBuf>) -> R<()> {
	TELEMETRY.debug_gdb.spark();
	if cfg!(not(unix)) {
		return Err(E::error("GDB debugging is only supported on Linux"));
	}
	if !util::is_installed("gdb")? {
		return Err(E::error("GDB is not installed").action_if(util::is_installed("apt")?, "🔐 Auto-install", install_gdb()));
	}
	term::debugger("GDB", &in_path, &[
		"gdb",
		"-q",
		build::exec_path(source)?.to_str().unwrap(),
		"-ex",
		&format!("set args < {}", util::bash_escape(in_path.to_str().unwrap())),
	])
}

pub fn rr(in_path: PathBuf, source: Option<PathBuf>) -> R<()> {
	TELEMETRY.debug_rr.spark();
	if cfg!(not(unix)) {
		return Err(E::error("RR debugging is only supported on Linux"));
	}
	if !util::is_installed("rr")? {
		return Err(E::error("RR is not installed").action_if(util::is_installed("apt")?, "🔐 Auto-install", install_rr()));
	}
	let record_out = Command::new("rr")
		.arg("record")
		.arg(build::exec_path(source)?)
		.stdin(File::open(&in_path).wrap("failed to redirect test input")?)
		.stdout(Stdio::null())
		.stderr(Stdio::piped())
		.output()
		.wrap("failed to spawn rr record session")?;
	if std::str::from_utf8(&record_out.stderr).wrap("rr record has written non-utf8 text to stderr")?.contains("/proc/sys/kernel/perf_event_paranoid")
	{
		return Err(E::error("RR is not configured properly (this is to be expected), kernel.perf_event_paranoid must be <= 1")
			.action("🔐 Auto-configure", configure_kernel_perf_event_paranoid()));
	}
	term::debugger("RR", &in_path, &["rr", "replay", "--", "-q"])
}

async fn install_gdb() -> R<()> {
	term::install("GDB", &["pkexec", "apt", "install", "-y", "gdb"])
}
async fn install_rr() -> R<()> {
	term::install("RR", &["pkexec", "apt", "install", "-y", "rr"])
}
async fn configure_kernel_perf_event_paranoid() -> R<()> {
	term::Internal::raw(
		"ICIE Auto-configure RR",
		"echo 'kernel.perf_event_paranoid=1' | pkexec tee -a /etc/sysctl.conf && echo 1 | pkexec tee -a /proc/sys/kernel/perf_event_paranoid",
	)
}
