use super::*;
use std::process::Command;

struct FakeEsxi {
	mount: PathBuf,
	target: PathBuf,
	vmx: String,
	current: String,
	power: bool,
	snapshots: Vec<String>,
	fail: String,
	seen: Vec<String>,
}
impl FakeEsxi {
	fn actual(&self, remote: &str) -> PathBuf {
		self.mount
			.join(remote.strip_prefix("/vmfs/volumes/").unwrap())
	}
	fn quoted(command: &str) -> Vec<String> {
		command
			.trim_matches('\'')
			.split("' '")
			.map(str::to_string)
			.collect()
	}
	fn qemu(args: &[&str]) -> Result<String> {
		let o = Command::new("qemu-img").args(args).output()?;
		ensure(
			o.status.success(),
			String::from_utf8_lossy(&o.stderr).into_owned(),
		)?;
		Ok(String::from_utf8_lossy(&o.stdout).into_owned())
	}
	fn reply(
		&self,
		path: &Path,
		error: &Path,
		text: &str,
		code: i32,
	) -> Result<process::CommandOutput> {
		process::write_private(path, text)?;
		process::write_private(
			error,
			if code == 0 {
				""
			} else {
				"injected test failure"
			},
		)?;
		Ok(process::CommandOutput {
			code,
			timed_out: false,
		})
	}
}
impl CommandExecutor for FakeEsxi {
	fn execute(
		&mut self,
		spec: &CommandSpec,
		stdout: &Path,
		stderr: &Path,
	) -> Result<process::CommandOutput> {
		let command = format!("{} {}", spec.program, spec.args.join(" "));
		self.seen.push(command.clone());
		if !self.fail.is_empty() && command.contains(&self.fail) {
			return self.reply(stdout, stderr, "", 70);
		}
		if spec.program == "qemu-img" || spec.program == "qemu-nbd" || spec.program == "kill" {
			return ProcessExecutor.execute(spec, stdout, stderr);
		}
		if spec.program == "pvesm" {
			match spec.args[0].as_str() {
			"status"=>return self.reply(stdout,stderr,"Name Type Status Total Used Available %\nlocal-lvm lvmthin active 999999999 0 999999999 0\nlocal dir active 999999999 0 999999999 0\n",0),
			"alloc"=>{File::create(&self.target)?.set_len(8*1024*1024)?;return self.reply(stdout,stderr,"successfully created 'local-lvm:vm-100-disk-0'\n",0);},
			"path"=>return self.reply(stdout,stderr,&self.target.to_string_lossy(),0),_=>{}
		}
		}
		if spec.program == "qm" {
			return self.reply(
				stdout,
				stderr,
				if spec.args[0] == "status" {
					"status: running\n"
				} else {
					""
				},
				0,
			);
		}
		if spec.program == "sshfs" || spec.program == "fusermount3" {
			return self.reply(stdout, stderr, "", 0);
		}
		if spec.program == "ssh" {
			if spec.args.iter().any(|s| s == "-O") {
				return self.reply(stdout, stderr, "", 0);
			}
			let remote = spec.args.last().unwrap();
			let a = Self::quoted(remote);
			let text = match a.first().map(String::as_str) {
				Some("vim-cmd") => match a[1].as_str() {
					"vmsvc/get.config" => "vmPathName = \"[ds] vm/guest.vmx\",\n".into(),
					"vmsvc/power.getstate" => {
						if self.power {
							"Retrieved runtime info\nPowered on\n".into()
						} else {
							"Powered off\n".into()
						}
					}
					"vmsvc/power.shutdown" => {
						self.power = false;
						String::new()
					}
					"vmsvc/snapshot.get" => self
						.snapshots
						.iter()
						.enumerate()
						.map(|(i, name)| {
							format!("Snapshot Name : {name}\nSnapshot Id : {}\n", i + 1)
						})
						.collect(),
					"vmsvc/snapshot.create" => {
						self.snapshots.push(a[3].clone());
						let parent = self.actual(&self.current);
						let relative = format!("snap{}.vmdk", self.snapshots.len());
						let child = parent.parent().unwrap().join(&relative);
						Self::qemu(&[
							"create",
							"-f",
							"vmdk",
							"-F",
							"vmdk",
							"-b",
							parent.file_name().unwrap().to_str().unwrap(),
							"-o",
							"subformat=twoGbMaxExtentSparse",
							child.to_str().unwrap(),
						])?;
						let output = Command::new("qemu-io")
							.args([
								"-f",
								"vmdk",
								"-c",
								&format!("write -P {} 1M 64K", self.snapshots.len() + 32),
								child.to_str().unwrap(),
							])
							.output()?;
						ensure(output.status.success(), "qemu-io failed")?;
						self.current = format!("/vmfs/volumes/ds/vm/{relative}");
						let vmx = vmx_text(&relative);
						fs::write(self.actual(&self.vmx), vmx)?;
						String::new()
					}
					_ => return Err(Error(format!("Sahte ESXi komutu eksik: {a:?}"))),
				},
				Some("mkdir" | "rmdir") => String::new(),
				Some("cat") => fs::read_to_string(self.actual(&a[1]))?,
				Some("readlink") => a[2].clone(),
				Some("esxcli") => {
					"MountPoint,VolumeName,UUID,Free\n/vmfs/volumes/ds,ds,ds,1099511627776\n".into()
				}
				Some("vmkfstools") => {
					if a[1] == "-i" {
						let from = self.actual(&a[2]);
						let to = self.actual(&a[3]);
						Self::qemu(&[
							"convert",
							"-f",
							"vmdk",
							"-O",
							"vmdk",
							"-o",
							"subformat=monolithicFlat",
							from.to_str().unwrap(),
							to.to_str().unwrap(),
						])?;
					}
					String::new()
				}
				Some("stat") => fs::metadata(self.actual(&a[3]))?.len().to_string(),
				_ if remote.starts_with("set -eu") => {
					let name = remote
						.split("dd if='")
						.nth(1)
						.and_then(|s| s.split('\'').next())
						.ok_or_else(|| Error("hash command".into()))?;
					let data = fs::read(self.actual(name))?;
					format!("{}  -\n", crate::sha256::digest(&data))
				}
				_ => return Err(Error(format!("Beklenmeyen sahte SSH: {remote}"))),
			};
			return self.reply(stdout, stderr, &text, 0);
		}
		Err(Error(format!("Beklenmeyen sahte komut: {command}")))
	}
}
fn vmx_text(file: &str) -> String {
	format!(
		"memSize = \"512\"\nnumvcpus = \"1\"\nscsi0.virtualDev = \"lsilogic\"\nscsi0:0.fileName = \"{file}\"\n"
	)
}
struct Fixture {
	_language: crate::i18n::LanguageScope,
	engine: Engine<FakeEsxi>,
	root: PathBuf,
}
impl Fixture {
	fn new(method: Method) -> Self {
		let language = crate::i18n::enter_language(crate::i18n::Language::Turkish);
		let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
			.join("target/flows")
			.join(format!(
				"{}-{}",
				std::process::id(),
				SystemTime::now()
					.duration_since(UNIX_EPOCH)
					.unwrap()
					.as_nanos()
			));
		let cfg = Config {
			method,
			esxi: "192.0.2.10".into(),
			esxi_vmid: 1,
			vmid: 100,
			storage: "local-lvm".into(),
			execute: true,
			yes: true,
			iterations: 0,
			log_root: root.join("log"),
			state_root: root.join("state"),
			run_root: PathBuf::from(env!("CARGO_MANIFEST_DIR"))
				.parent()
				.unwrap()
				.parent()
				.unwrap()
				.join(".r"),
			..Config::default()
		};
		let fake = FakeEsxi {
			mount: PathBuf::new(),
			target: root.join("target.raw"),
			vmx: "/vmfs/volumes/ds/vm/guest.vmx".into(),
			current: "/vmfs/volumes/ds/vm/base.vmdk".into(),
			power: method != Method::Offline,
			snapshots: Vec::new(),
			fail: String::new(),
			seen: Vec::new(),
		};
		let mut e = Engine::new(cfg, fake, None).unwrap();
		let source = e.mount.join("ds/vm");
		fs::create_dir_all(&source).unwrap();
		e.journal.executor.mount = e.mount.clone();
		e.vmx = "/vmfs/volumes/ds/vm/guest.vmx".into();
		FakeEsxi::qemu(&[
			"create",
			"-f",
			"vmdk",
			"-o",
			"subformat=twoGbMaxExtentSparse",
			source.join("base.vmdk").to_str().unwrap(),
			"8M",
		])
		.unwrap();
		fs::write(source.join("guest.vmx"), vmx_text("base.vmdk")).unwrap();
		e.refresh("fixture").unwrap();
		e.initial = e.current.clone();
		e.sizes = vec![8 * 1024 * 1024];
		e.cfg.disk_bus = "scsi".into();
		e.cfg.scsi_hw = Some("lsi".into());
		e.cfg.bios = "seabios".into();
		Self {
			engine: e,
			root,
			_language: language,
		}
	}
	fn compare(&mut self) {
		let source = self.engine.local(&self.engine.paths()[0]).unwrap();
		FakeEsxi::qemu(&[
			"compare",
			"-f",
			"vmdk",
			"-F",
			"raw",
			source.to_str().unwrap(),
			self.engine.targets[0].path.to_str().unwrap(),
		])
		.unwrap();
	}
}
impl Drop for Fixture {
	fn drop(&mut self) {
		for i in 0..self.engine.nbds.len() {
			let _ = self.engine.stop_nbd(i);
		}
		let _ = fs::remove_dir_all(&self.engine.run);
		let _ = fs::remove_dir_all(&self.root);
	}
}
#[test]
fn offline_full_copy_verified_before_attach() {
	let mut f = Fixture::new(Method::Offline);
	f.engine.offline().unwrap();
	f.compare();
	let seen = &f.engine.journal.executor.seen;
	let compare = seen
		.iter()
		.position(|s| s.contains("qemu-img compare"))
		.unwrap();
	let attach = seen.iter().position(|s| s.contains("--scsi0")).unwrap();
	assert!(compare < attach);
	assert!(!seen.iter().any(|s| s.starts_with("qm start")));
}
#[test]
fn snapshot_zero_iterations_final_delta() {
	let mut f = Fixture::new(Method::SnapshotIterative);
	f.engine.snapshot_iterative().unwrap();
	f.compare();
	assert_eq!(f.engine.journal.executor.snapshots.len(), 1);
	assert!(
		f.engine
			.journal
			.executor
			.seen
			.iter()
			.any(|s| s.contains("qemu-img map"))
	);
}
#[test]
fn snapshot_iteration_chain() {
	let mut f = Fixture::new(Method::SnapshotIterative);
	f.engine.cfg.iterations = 2;
	f.engine.snapshot_iterative().unwrap();
	f.compare();
	assert_eq!(f.engine.journal.executor.snapshots.len(), 2);
}
#[test]
fn live_clone_final_hash_verification() {
	let mut f = Fixture::new(Method::LiveClone);
	f.engine.live_clone().unwrap();
	f.compare();
	assert!(
		f.engine
			.journal
			.executor
			.seen
			.iter()
			.any(|s| s.contains("sha256sum"))
	);
}
#[test]
fn graceful_shutdown_failure_never_copies_or_starts() {
	let mut f = Fixture::new(Method::Offline);
	f.engine.journal.executor.power = true;
	f.engine.journal.executor.fail = "vmsvc/power.shutdown".into();
	assert!(f.engine.offline().is_err());
	assert!(
		!f.engine
			.journal
			.executor
			.seen
			.iter()
			.any(|s| s.contains("qemu-img convert") || s.starts_with("qm start"))
	);
}
#[test]
fn initial_clone_failure_leaves_source_running() {
	let mut f = Fixture::new(Method::LiveClone);
	f.engine.journal.executor.fail = "'vmkfstools' '-i'".into();
	assert!(f.engine.live_clone().is_err());
	assert!(f.engine.journal.executor.power);
	assert!(
		!f.engine
			.journal
			.executor
			.seen
			.iter()
			.any(|s| s.contains("power.shutdown"))
	);
}
#[test]
fn delta_map_failure_no_target_start_or_fallback() {
	let mut f = Fixture::new(Method::SnapshotIterative);
	f.engine.journal.executor.fail = "qemu-img map".into();
	assert!(f.engine.snapshot_iterative().is_err());
	assert!(!f.engine.journal.executor.power);
	assert!(
		!f.engine
			.journal
			.executor
			.seen
			.iter()
			.any(|s| s.starts_with("qm start"))
	);
	let clone_calls = f
		.engine
		.journal
		.executor
		.seen
		.iter()
		.filter(|s| s.contains("'vmkfstools' '-i'"))
		.count();
	assert_eq!(clone_calls, 1);
}
#[test]
fn compare_failure_no_attach() {
	let mut f = Fixture::new(Method::Offline);
	f.engine.journal.executor.fail = "qemu-img compare".into();
	assert!(f.engine.offline().is_err());
	assert!(
		!f.engine
			.journal
			.executor
			.seen
			.iter()
			.any(|s| s.contains("--scsi0"))
	);
}
#[test]
fn source_state_ssh_failure_not_treated_as_off() {
	let mut f = Fixture::new(Method::Offline);
	f.engine.journal.executor.fail = "power.getstate".into();
	assert!(f.engine.require_source_off().is_err());
}
#[test]
fn mirror_space_failure_precedes_shutdown() {
	let mut f = Fixture::new(Method::LiveMirror);
	f.engine.cfg.staging_storage = Some("missing".into());
	assert!(f.engine.live_mirror().is_err());
	assert!(
		!f.engine
			.journal
			.executor
			.seen
			.iter()
			.any(|s| s.contains("power.shutdown") || s.starts_with("qm start"))
	);
}
#[test]
fn wrong_nbd_pid_not_killed() {
	let mut f = Fixture::new(Method::Offline);
	f.engine.nbds.push(Nbd {
		pid: std::process::id(),
		socket: f.engine.run.join("not-ours.sock"),
		stopped: false,
	});
	assert!(f.engine.stop_nbd(0).is_err());
	f.engine.nbds[0].stopped = true;
	assert!(
		!f.engine
			.journal
			.executor
			.seen
			.iter()
			.any(|s| s.starts_with("kill "))
	);
}

#[test]
fn source_secure_boot_disabled_stays_disabled() {
	let mut f = Fixture::new(Method::Offline);
	f.engine.cfg.bios = "ovmf".into();
	f.engine.offline().unwrap();
	assert!(
		f.engine
			.journal
			.executor
			.seen
			.iter()
			.any(|s| s.contains("pre-enrolled-keys=0"))
	);
}
#[test]
fn secure_boot_cannot_be_silently_downgraded() {
	let mut f = Fixture::new(Method::Offline);
	f.engine.initial.as_mut().unwrap().secure_boot = true;
	f.engine.cfg.bios = "seabios".into();
	assert!(f.engine.configure().is_err());
	assert!(
		!f.engine
			.journal
			.executor
			.seen
			.iter()
			.any(|s| s.starts_with("qm create"))
	);
}

#[test]
fn sshfs_failure_precedes_any_source_lock_or_target_creation() {
	let mut f = Fixture::new(Method::SnapshotIterative);
	f.engine.journal.executor.fail = "sshfs ".into();
	let error = f.engine.perform_connected().unwrap_err();
	assert!(error.to_string().contains("injected test failure"));
	assert_eq!(f.engine.journal.phase, "source_mount");
	assert!(!f.engine.create_attempted);
	assert!(!f.engine.created);
	assert!(!f.engine.source_lock_held);
	assert!(!f.engine.shutdown_attempted);
	assert!(f.engine.mount_attempted);
	assert!(!f.engine.mount_established);
	let seen = &f.engine.journal.executor.seen;
	assert!(!seen.iter().any(|s| s.starts_with("qm create")
		|| s.contains("'mkdir'")
		|| s.contains("snapshot.create")
		|| s.contains("power.shutdown")));
	let note = process::read_text(&f.engine.journal.dir.join("RECOVERY.txt")).unwrap();
	assert!(note.contains("Hedef oluşturma komutu denendi: HAYIR"));
	assert!(!note.contains("Live Mirror henüz"));
}
#[test]
fn plan_checks_transport_then_unmounts_without_changing_vm() {
	let mut f = Fixture::new(Method::SnapshotIterative);
	f.engine.cfg.execute = false;
	f.engine.perform_connected().unwrap();
	assert_eq!(f.engine.journal.phase, "planned");
	assert!(f.engine.mount_attempted);
	assert!(!f.engine.mount_established);
	let seen = &f.engine.journal.executor.seen;
	let mount = seen.iter().position(|s| s.starts_with("sshfs ")).unwrap();
	let unmount = seen
		.iter()
		.position(|s| s.starts_with("fusermount3 "))
		.unwrap();
	assert!(mount < unmount);
	assert!(!seen.iter().any(|s| s.starts_with("qm create")
		|| s.contains("snapshot.create")
		|| s.contains("power.shutdown")
		|| s.contains("'mkdir'")));
}
#[test]
fn failed_target_create_retains_attempt_and_mount_state() {
	let mut f = Fixture::new(Method::SnapshotIterative);
	f.engine.journal.executor.fail = "qm create ".into();
	assert!(f.engine.perform_connected().is_err());
	assert_eq!(f.engine.journal.phase, "target_creation");
	assert!(f.engine.create_attempted && f.engine.mount_established && f.engine.source_lock_held);
	assert!(!f.engine.created && !f.engine.start_attempted);
	let note = process::read_text(&f.engine.journal.dir.join("RECOVERY.txt")).unwrap();
	assert!(note.contains("Hedef oluşturma komutu denendi: EVET"));
	assert!(!note.contains("hedef VM oluşturma aşamasına ulaşmadı"));
}

#[test]
fn recovery_and_event_texts_follow_job_language_without_changing_keys() {
	let mut fixture = Fixture::new(Method::SnapshotIterative);
	for (language, header, field, message) in [
		(
			crate::i18n::Language::English,
			"Job:",
			"Target creation command attempted: NO",
			"Phase: source_mount",
		),
		(
			crate::i18n::Language::Turkish,
			"İş:",
			"Hedef oluşturma komutu denendi: HAYIR",
			"Aşama: source_mount",
		),
	] {
		crate::i18n::with_language(language, || {
			fixture.engine.cfg.language = language;
			fixture.engine.phase("source_mount").unwrap();
			let note =
				process::read_text(&fixture.engine.journal.dir.join("RECOVERY.txt")).unwrap();
			assert!(note.starts_with(header) && note.contains(field), "{note}");
			let state = json::parse(
				&process::read_text(&fixture.engine.journal.dir.join("state.json")).unwrap(),
			)
			.unwrap();
			assert_eq!(
				state.get("phase").unwrap().string().unwrap(),
				"source_mount"
			);
			assert_eq!(
				state.get("language").unwrap().string().unwrap(),
				language.code()
			);
			let log = process::read_text(&fixture.engine.log_file).unwrap();
			let event = json::parse(log.lines().last().unwrap()).unwrap();
			assert_eq!(event.get("message").unwrap().string().unwrap(), message);
			assert_eq!(
				event.get("language").unwrap().string().unwrap(),
				language.code()
			);
		});
	}
}
