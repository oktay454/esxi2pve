//! Komut yürütme sınırı: argüman dizileri, ayrı kalıcı stdout/stderr, zaman aşımı.
use crate::{Error, Result, ensure, json};
use std::{
	fs::{self, File, OpenOptions},
	io::{Read, Seek, SeekFrom, Write},
	os::unix::fs::{DirBuilderExt, OpenOptionsExt},
	path::{Path, PathBuf},
	process::{Command, Stdio},
	thread,
	time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

pub fn epoch() -> u64 {
	SystemTime::now()
		.duration_since(UNIX_EPOCH)
		.unwrap_or_default()
		.as_secs()
}
/// Tek kimlik biçimi: mevcut iş veya günlükle çakışma halinde üzerine yazılmaz.
pub(crate) fn allocate_job_dir(
	state_root: &Path,
	log_root: &Path,
	job: &str,
) -> Result<(String, PathBuf)> {
	ensure(
		!log_root.join(format!("{job}.jsonl")).exists(),
		tr!("Benzersiz iş dizini oluşturulamadı"),
	)?;
	let dir = state_root.join(job);
	fs::DirBuilder::new().mode(0o700).create(&dir)?;
	Ok((job.to_string(), dir))
}

pub fn private_dir(path: &Path) -> Result<()> {
	let mut b = fs::DirBuilder::new();
	b.mode(0o700);
	b.recursive(true);
	b.create(path)?;
	Ok(())
}
pub fn write_private(path: &Path, data: impl AsRef<[u8]>) -> Result<()> {
	let mut f = OpenOptions::new()
		.write(true)
		.create(true)
		.truncate(true)
		.mode(0o600)
		.open(path)?;
	f.write_all(data.as_ref())?;
	f.sync_all()?;
	Ok(())
}
pub fn atomic_write(path: &Path, data: impl AsRef<[u8]>) -> Result<()> {
	let tmp = path.with_extension("new");
	write_private(&tmp, data)?;
	fs::rename(&tmp, path)?;
	if let Some(p) = path.parent() {
		File::open(p)?.sync_all()?;
	}
	Ok(())
}
pub fn read_text(path: &Path) -> Result<String> {
	ensure(
		fs::metadata(path)?.len() <= 128 * 1024 * 1024,
		tr!("Metin/JSON komut çıktısı 128MiB sınırını aştı"),
	)?;
	Ok(fs::read_to_string(path)?)
}
#[derive(Clone, Debug)]
pub struct CommandSpec {
	pub program: String,
	pub args: Vec<String>,
	pub timeout: Duration,
	pub interactive: bool,
}
impl CommandSpec {
	pub fn new(
		program: impl Into<String>,
		args: impl IntoIterator<Item = impl Into<String>>,
		timeout: Duration,
	) -> Self {
		Self {
			program: program.into(),
			args: args.into_iter().map(Into::into).collect(),
			timeout,
			interactive: false,
		}
	}
}
#[derive(Debug)]
pub struct CommandOutput {
	pub code: i32,
	pub timed_out: bool,
}
/// Çıktılar yürütme sırasında dosyalara aktarılmalıdır; üretim uygulaması bellekte biriktirmez.
pub trait CommandExecutor {
	fn execute(
		&mut self,
		spec: &CommandSpec,
		stdout: &Path,
		stderr: &Path,
	) -> Result<CommandOutput>;
}
#[derive(Default)]
pub struct ProcessExecutor;
impl CommandExecutor for ProcessExecutor {
	fn execute(
		&mut self,
		spec: &CommandSpec,
		stdout: &Path,
		stderr: &Path,
	) -> Result<CommandOutput> {
		let out = OpenOptions::new()
			.create_new(true)
			.write(true)
			.mode(0o600)
			.open(stdout)?;
		let err = OpenOptions::new()
			.create_new(true)
			.write(true)
			.mode(0o600)
			.open(stderr)?;
		let mut command = Command::new(&spec.program);
		command
			.args(&spec.args)
			.stdout(Stdio::from(out))
			.stderr(if spec.interactive {
				Stdio::inherit()
			} else {
				Stdio::from(err)
			})
			.stdin(if spec.interactive {
				Stdio::inherit()
			} else {
				Stdio::null()
			});
		let mut child = command
			.spawn()
			.map_err(|e| Error(tr!("{} çalıştırılamadı: {e}", spec.program, e = e)))?;
		let started = Instant::now();
		loop {
			if let Some(status) = child.try_wait()? {
				return Ok(CommandOutput {
					code: status.code().unwrap_or_else(|| {
						use std::os::unix::process::ExitStatusExt;
						128 + status.signal().unwrap_or(0)
					}),
					timed_out: false,
				});
			}
			if started.elapsed() >= spec.timeout {
				child.kill()?;
				child.wait()?;
				return Ok(CommandOutput {
					code: 124,
					timed_out: true,
				});
			}
			thread::sleep(Duration::from_millis(50));
		}
	}
}
pub(crate) struct Journal<E> {
	pub executor: E,
	pub dir: PathBuf,
	log: File,
	count: u64,
	pub phase: String,
	pub timeout: Duration,
	pub console: crate::presentation::Console,
}
impl<E: CommandExecutor> Journal<E> {
	pub fn new(executor: E, dir: PathBuf, log_path: &Path, timeout: Duration) -> Result<Self> {
		private_dir(&dir.join("commands"))?;
		let log = OpenOptions::new()
			.create_new(true)
			.append(true)
			.mode(0o600)
			.open(log_path)?;
		Ok(Self {
			executor,
			dir,
			log,
			count: 0,
			phase: "initializing".into(),
			timeout,
			console: crate::presentation::Console::default(),
		})
	}
	pub fn log(&mut self, level: &str, message: &str) -> Result<()> {
		let now = epoch();
		let row = json::object([
			("epoch", json::number(now)),
			("level", json::text(level)),
			(
				"language",
				json::text(crate::i18n::current_language().code()),
			),
			("phase", json::text(&self.phase)),
			("message", json::text(message)),
		])
		.encode();
		writeln!(self.log, "{row}")?;
		self.log.sync_data()?;
		self.console.event(now, level, &self.phase, message);
		Ok(())
	}
	pub fn progress(&mut self, current: u64, total: u64, message: &str) -> Result<()> {
		self.log("PROGRESS", message)?;
		self.console
			.progress(epoch(), &self.phase, current, total, message);
		Ok(())
	}
	pub fn run(
		&mut self,
		program: &str,
		args: impl IntoIterator<Item = impl Into<String>>,
	) -> Result<String> {
		self.run_spec(CommandSpec::new(program, args, self.timeout))
	}
	pub fn attempt(
		&mut self,
		program: &str,
		args: impl IntoIterator<Item = impl Into<String>>,
	) -> Result<String> {
		self.run_spec(CommandSpec::new(program, args, self.timeout))
	}
	pub fn run_spec(&mut self, spec: CommandSpec) -> Result<String> {
		self.count += 1;
		let base = self.dir.join("commands").join(format!("{:06}", self.count));
		let argv = json::Value::Array(
			std::iter::once(json::text(&spec.program))
				.chain(spec.args.iter().map(json::text))
				.collect(),
		)
		.encode();
		write_private(&base.with_extension("argv.json"), &argv)?;
		self.log("CMD", &format!("{} {argv}", base.display()))?;
		let out = base.with_extension("stdout");
		let err = base.with_extension("stderr");
		self.console
			.command_start(epoch(), &self.phase, self.count, &spec);
		let started = Instant::now();
		let result = self.executor.execute(&spec, &out, &err);
		match result {
			Ok(status) => {
				atomic_write(
					&base.with_extension("result.json"),
					json::object([
						("code", json::number(status.code.max(0) as u64)),
						("timed_out", json::Value::Bool(status.timed_out)),
					])
					.encode(),
				)?;
				self.log(
					"CMD",
					&tr!(
						"Çıkış={}; stdout={} stderr={}",
						status.code,
						out.display(),
						err.display()
					),
				)?;
				self.console.command_end(
					epoch(),
					&self.phase,
					self.count,
					&spec,
					status.code,
					started.elapsed(),
				);
				if status.code != 0 {
					let tail = error_tail(&err).unwrap_or_default();
					let detail = if tail.is_empty() {
						String::new()
					} else {
						format!("; stderr: {tail}")
					};
					return Err(Error(tr!(
						"{} başarısız ({}); {}{detail}",
						spec.program,
						status.code,
						err.display(),
						detail = detail
					)));
				}
				read_text(&out)
			}
			Err(e) => {
				self.log("ERROR", &e.to_string())?;
				Err(e)
			}
		}
	}
	pub fn resource(&mut self, kind: &str, values: &[&str]) -> Result<()> {
		let now = epoch();
		let row = json::object([
			("epoch", json::number(now)),
			("kind", json::text(kind)),
			(
				"values",
				json::Value::Array(values.iter().map(|v| json::text(*v)).collect()),
			),
		])
		.encode();
		let mut f = OpenOptions::new()
			.create(true)
			.append(true)
			.mode(0o600)
			.open(self.dir.join("resources.jsonl"))?;
		writeln!(f, "{row}")?;
		f.sync_all()?;
		Ok(())
	}
}
fn error_tail(path: &Path) -> Result<String> {
	let mut file = File::open(path)?;
	let length = file.metadata()?.len();
	file.seek(SeekFrom::Start(length.saturating_sub(8192)))?;
	let mut bytes = Vec::new();
	file.take(8192).read_to_end(&mut bytes)?;
	let text = String::from_utf8_lossy(&bytes);
	let lines = text.lines().rev().take(8).collect::<Vec<_>>();
	Ok(lines.into_iter().rev().collect::<Vec<_>>().join(" | "))
}

pub(crate) fn remote_quote(s: &str) -> String {
	format!("'{}'", s.replace('\'', "'\\''"))
}
pub(crate) fn find_program(name: &str) -> bool {
	std::env::var_os("PATH").is_some_and(|v| {
		std::env::split_paths(&v).any(|p| {
			use std::os::unix::fs::PermissionsExt;
			fs::metadata(p.join(name))
				.is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
		})
	})
}
#[cfg(test)]
mod tests {
	use super::*;
	#[test]
	fn quote_injection() {
		assert_eq!(remote_quote("a'$(x)\n"), "'a'\\''$(x)\n'");
	}
}

#[cfg(test)]
mod execution_tests {
	use super::*;
	fn root() -> PathBuf {
		let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
			.join("target/process-tests")
			.join(format!(
				"{}-{}",
				std::process::id(),
				SystemTime::now()
					.duration_since(UNIX_EPOCH)
					.unwrap()
					.as_nanos()
			));
		private_dir(&p).unwrap();
		p
	}
	#[test]
	fn stdout_stderr_and_exact_arguments_survive_failure() {
		let root = root();
		let mut j = Journal::new(
			ProcessExecutor,
			root.join("state"),
			&root.join("events"),
			Duration::from_secs(2),
		)
		.unwrap();
		let error = j
			.run(
				"/bin/ls",
				[root.join("missing $(never-run)").to_str().unwrap()],
			)
			.unwrap_err();
		assert!(error.to_string().contains("stderr:"));
		assert!(error.to_string().contains("missing $(never-run)"));
		let commands = j.dir.join("commands");
		assert!(commands.join("000001.stdout").is_file());
		assert!(
			read_text(&commands.join("000001.stderr"))
				.unwrap()
				.contains("missing $(never-run)")
		);
		let result =
			json::parse(&read_text(&commands.join("000001.result.json")).unwrap()).unwrap();
		assert_ne!(result.get("code").unwrap().integer().unwrap(), 0);
		let argv = json::parse(&read_text(&commands.join("000001.argv.json")).unwrap()).unwrap();
		assert!(
			argv.array().unwrap()[1]
				.string()
				.unwrap()
				.contains("$(never-run)")
		);
		drop(j);
		fs::remove_dir_all(root).unwrap();
	}
	#[test]
	fn timeout_is_logged_and_child_reaped() {
		let root = root();
		let mut j = Journal::new(
			ProcessExecutor,
			root.join("state"),
			&root.join("events"),
			Duration::from_millis(50),
		)
		.unwrap();
		assert!(j.run("/bin/sleep", ["5"]).is_err());
		let result =
			json::parse(&read_text(&j.dir.join("commands/000001.result.json")).unwrap()).unwrap();
		assert!(result.get("timed_out").unwrap().is_true());
		assert_eq!(result.get("code").unwrap().integer().unwrap(), 124);
		drop(j);
		fs::remove_dir_all(root).unwrap();
	}
}

#[cfg(test)]
mod stderr_tests {
	#[test]
	fn limits_diagnostic_tail() {
		let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
			.join("target/stderr-tail-tests")
			.join(format!(
				"{}-{}",
				std::process::id(),
				std::time::SystemTime::now()
					.duration_since(std::time::UNIX_EPOCH)
					.unwrap()
					.as_nanos()
			));
		super::private_dir(&root).unwrap();
		let path = root.join("stderr");
		super::write_private(
			&path,
			format!(
				"{}\n{}",
				"x".repeat(20000),
				(0..20).map(|i| format!("line {i}\n")).collect::<String>()
			),
		)
		.unwrap();
		let tail = super::error_tail(&path).unwrap();
		assert!(!tail.contains("line 11"));
		assert!(tail.contains("line 12") && tail.contains("line 19"));
		assert!(tail.len() < 8192);
		std::fs::remove_dir_all(root).unwrap();
	}
}

#[cfg(test)]
mod presentation_tests {
	use super::*;
	fn temp() -> PathBuf {
		let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
			.join("target/presentation-tests")
			.join(format!(
				"{}-{}",
				std::process::id(),
				SystemTime::now()
					.duration_since(UNIX_EPOCH)
					.unwrap()
					.as_nanos()
			));
		private_dir(&path).unwrap();
		path
	}
	#[test]
	fn fixed_job_allocation_is_exclusive_and_never_changes_naming_format() {
		let root = temp();
		let state = root.join("state");
		let logs = root.join("log");
		private_dir(&state).unwrap();
		private_dir(&logs).unwrap();
		let job = "2026-10-24T103000.123456789Z_esxi-72_pve-108281_p42";
		let threads: Vec<_> = (0..8)
			.map(|_| {
				let state = state.clone();
				let logs = logs.clone();
				std::thread::spawn(move || allocate_job_dir(&state, &logs, job))
			})
			.collect();
		let results: Vec<_> = threads.into_iter().map(|t| t.join().unwrap()).collect();
		assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
		assert_eq!(fs::read_dir(&state).unwrap().count(), 1);
		write_private(&state.join(job).join("keep"), "unchanged").unwrap();
		assert!(allocate_job_dir(&state, &logs, job).is_err());
		assert_eq!(
			read_text(&state.join(job).join("keep")).unwrap(),
			"unchanged"
		);
		let log_only = "2026-10-24T103001.123456789Z_esxi-72_pve-108281_p42";
		write_private(&logs.join(format!("{log_only}.jsonl")), "old log").unwrap();
		assert!(allocate_job_dir(&state, &logs, log_only).is_err());
		fs::remove_dir_all(root).unwrap();
	}

	#[test]
	fn console_throttling_and_verbose_mode_preserve_every_json_progress_record() {
		let root = temp();
		let mut journal = Journal::new(
			ProcessExecutor,
			root.join("state"),
			&root.join("events.jsonl"),
			Duration::from_secs(1),
		)
		.unwrap();
		journal.phase = "online_precopy".into();
		for block in 1..=5 {
			journal.console.verbose = block > 3;
			journal
				.progress(block, 5, &format!("record-{block}"))
				.unwrap();
		}
		let rows = read_text(&root.join("events.jsonl")).unwrap();
		let rows: Vec<_> = rows
			.lines()
			.map(|line| json::parse(line).unwrap())
			.collect();
		assert_eq!(rows.len(), 5);
		for (index, row) in rows.iter().enumerate() {
			let json::Value::Object(fields) = row else {
				panic!("object expected")
			};
			assert_eq!(fields.len(), 5);
			assert_eq!(row.get("level").unwrap().string().unwrap(), "PROGRESS");
			assert_eq!(
				row.get("phase").unwrap().string().unwrap(),
				"online_precopy"
			);
			assert_eq!(
				row.get("message").unwrap().string().unwrap(),
				format!("record-{}", index + 1)
			);
			assert!(row.get("epoch").unwrap().integer().unwrap() > 0);
		}
		drop(journal);
		fs::remove_dir_all(root).unwrap();
	}
}
