//! Dört yöntemi yöneten aktarım durum makinesi. Hata anında veri kaynaklarını korur.
use crate::{
	Config, DeltaEngine, Error, Method, Result,
	disk::{self, Qmp},
	ensure,
	json::{self, Value},
	process::{self, CommandExecutor, CommandSpec, Journal, ProcessExecutor},
	vmware::{self, Manifest},
};
use std::{
	collections::BTreeSet,
	fs::{self, File, OpenOptions},
	io::{self, IsTerminal, Write},
	net::TcpStream,
	os::unix::fs::OpenOptionsExt,
	path::{Path, PathBuf},
	thread,
	time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
const BLOCK: u64 = 64 * 1024 * 1024;
type HealthCallback = Box<dyn FnMut(u32) -> Result<bool>>;

#[derive(Debug)]
pub struct Report {
	pub job: String,
	pub phase: String,
	pub state_dir: PathBuf,
	pub log_file: PathBuf,
	pub first_ready_epoch: Option<u64>,
	pub downtime_seconds: Option<u64>,
}
pub struct Migration<E: CommandExecutor = ProcessExecutor> {
	config: Config,
	executor: E,
	health: Option<(Duration, HealthCallback)>,
}
impl Migration<ProcessExecutor> {
	pub fn new(config: Config) -> Self {
		Self::with_executor(config, ProcessExecutor)
	}
}
impl<E: CommandExecutor> Migration<E> {
	pub fn with_executor(config: Config, executor: E) -> Self {
		Self {
			config,
			executor,
			health: None,
		}
	}
	/// Özel uygulama sağlık denetimi; true hazır, false henüz hazır değil. Her çağrı sınırlı sürede dönmelidir.
	pub fn with_health_check(
		mut self,
		timeout: Duration,
		check: impl FnMut(u32) -> Result<bool> + 'static,
	) -> Self {
		self.health = Some((timeout, Box::new(check)));
		self
	}
	pub fn run(self) -> Result<Report> {
		crate::i18n::with_language(self.config.language, || self.run_localized())
	}
	fn run_localized(self) -> Result<Report> {
		self.config.validate()?;
		ensure(
			!self.config.start || self.config.health.is_some() || self.health.is_some(),
			tr!("--start için --health-tcp veya özel Rust sağlık denetimi gerekli"),
		)?;
		let mut engine = Engine::new(self.config, self.executor, self.health)?;
		let result = engine.perform();
		if let Err(error) = result {
			let message = tr!(
				"{error}; kaynaklar korundu. Kurtarma: {}",
				engine.journal.dir.join("RECOVERY.txt").display(),
				error = error
			);
			let _ = engine.journal.log("ERROR", &message);
			let _ = engine.save_state();
			return Err(Error(message));
		}
		Ok(Report {
			job: engine.job,
			phase: engine.journal.phase,
			state_dir: engine.journal.dir,
			log_file: engine.log_file,
			first_ready_epoch: engine.first_ready,
			downtime_seconds: engine
				.first_ready
				.map(|n| n.saturating_sub(engine.downtime_start)),
		})
	}
}
struct Target {
	volume: String,
	path: PathBuf,
}
struct Nbd {
	pid: u32,
	socket: PathBuf,
	stopped: bool,
}
struct Engine<E: CommandExecutor> {
	cfg: Config,
	journal: Journal<E>,
	job: String,
	run: PathBuf,
	log_file: PathBuf,
	mount: PathBuf,
	ssh: Vec<String>,
	vmx: String,
	initial: Option<Manifest>,
	current: Option<Manifest>,
	sizes: Vec<u64>,
	targets: Vec<Target>,
	overlays: Vec<Target>,
	applied: Vec<String>,
	nbds: Vec<Nbd>,
	created: bool,
	create_attempted: bool,
	mount_attempted: bool,
	mount_established: bool,
	shutdown_attempted: bool,
	start_attempted: bool,
	source_lock: String,
	source_lock_held: bool,
	downtime_start: u64,
	first_ready: Option<u64>,
	local_lock: Option<File>,
	health: Option<(Duration, HealthCallback)>,
}
impl<E: CommandExecutor> Engine<E> {
	fn new(cfg: Config, executor: E, health: Option<(Duration, HealthCallback)>) -> Result<Self> {
		let nanos = SystemTime::now()
			.duration_since(UNIX_EPOCH)
			.unwrap_or_default()
			.as_nanos();
		let job = format!(
			"{}-{}-{}-{:x}",
			process::epoch(),
			cfg.vmid,
			std::process::id(),
			nanos
		);
		process::private_dir(&cfg.log_root)?;
		process::private_dir(&cfg.state_root)?;
		process::private_dir(&cfg.run_root)?;
		let dir = cfg.state_root.join(&job);
		ensure(!dir.exists(), tr!("İş dizini zaten var"))?;
		process::private_dir(&dir)?;
		// Unix socket yolu 108 bayttan kısa kalır; kalıcı iş adı bundan bağımsızdır.
		let run = cfg
			.run_root
			.join(format!("{}-{:x}", std::process::id(), nanos & 0xffffffffff));
		process::private_dir(&run)?;
		ensure(
			run.as_os_str().len() < 72,
			tr!("--run-root Unix soketleri için fazla uzun"),
		)?;
		let mount = run.join("esxi");
		let log_file = cfg.log_root.join(format!("{job}.jsonl"));
		let journal = Journal::new(executor, dir, &log_file, cfg.command_timeout)?;
		let mut e = Self {
			cfg,
			journal,
			job,
			run,
			log_file,
			mount,
			ssh: Vec::new(),
			vmx: String::new(),
			initial: None,
			current: None,
			sizes: Vec::new(),
			targets: Vec::new(),
			overlays: Vec::new(),
			applied: Vec::new(),
			nbds: Vec::new(),
			created: false,
			create_attempted: false,
			mount_attempted: false,
			mount_established: false,
			shutdown_attempted: false,
			start_attempted: false,
			source_lock: String::new(),
			source_lock_held: false,
			downtime_start: 0,
			first_ready: None,
			local_lock: None,
			health,
		};
		e.save_state()?;
		Ok(e)
	}
	fn paths(&self) -> Vec<String> {
		self.current
			.as_ref()
			.map(|m| m.disks.iter().map(|d| d.remote.clone()).collect())
			.unwrap_or_default()
	}
	fn vmid(&self) -> String {
		self.cfg.vmid.to_string()
	}
	fn phase(&mut self, name: &str) -> Result<()> {
		self.journal.phase = name.into();
		self.save_state()?;
		self.journal.log("INFO", &tr!("Aşama: {name}", name = name))
	}
	fn save_state(&mut self) -> Result<()> {
		let state = json::object([
			("job", json::text(&self.job)),
			("method", json::text(self.cfg.method.name())),
			("language", json::text(self.cfg.language.code())),
			("phase", json::text(&self.journal.phase)),
			("esxi", json::text(&self.cfg.esxi)),
			("esxi_vmid", json::number(self.cfg.esxi_vmid as u64)),
			("vmid", json::number(self.cfg.vmid as u64)),
			("target_created", Value::Bool(self.created)),
			(
				"target_create_attempted",
				Value::Bool(self.create_attempted),
			),
			("mount_attempted", Value::Bool(self.mount_attempted)),
			("mount_established", Value::Bool(self.mount_established)),
			(
				"source_shutdown_attempted",
				Value::Bool(self.shutdown_attempted),
			),
			("target_start_attempted", Value::Bool(self.start_attempted)),
			("source_lock", json::text(&self.source_lock)),
			("source_lock_held", Value::Bool(self.source_lock_held)),
			("mount", json::text(self.mount.to_string_lossy())),
			("run_dir", json::text(self.run.to_string_lossy())),
			("downtime_start_epoch", json::number(self.downtime_start)),
			(
				"first_ready_epoch",
				self.first_ready.map(json::number).unwrap_or(Value::Null),
			),
		]);
		process::atomic_write(&self.journal.dir.join("state.json"), state.encode())?;
		let yes_no = |value| if value { tr!("EVET") } else { tr!("HAYIR") };
		let mut note = tr!(
			"İş: {}\nYöntem: {}\nAşama: {}\nESXi: {} VMID {}\nPVE VMID: {}\n\nHedef oluşturma komutu denendi: {}\nHedef oluşturulduğu doğrulandı: {}\nHedef başlatma komutu denendi: {}\nKaynak kapama komutu denendi: {}\nSSHFS bağlama denendi: {}\nSSHFS bağlantısı doğrulandı: {}\nKaynak kilidi alındığı doğrulandı: {}\n",
			self.job,
			self.cfg.method.name(),
			self.journal.phase,
			self.cfg.esxi,
			self.cfg.esxi_vmid,
			self.cfg.vmid,
			yes_no(self.create_attempted),
			yes_no(self.created),
			yes_no(self.start_attempted),
			yes_no(self.shutdown_attempted),
			yes_no(self.mount_attempted),
			yes_no(self.mount_established),
			yes_no(self.source_lock_held)
		);
		if !self.create_attempted {
			note.push_str(&tr!("Bu iş hedef VM oluşturma aşamasına ulaşmadı.\n"));
		} else if !self.start_attempted {
			note.push_str(&tr!(
				"Bu iş hedefi başlatma aşamasına ulaşmadı. Hedefi aktarma doğrulanmadan başlatmayın.\n"
			));
		} else {
			note.push_str(&tr!("Hedef başlatılmaya çalışıldı; kaynakta olmayan yeni yazmalar bulunabilir. Aynı anda iki kopyayı açmayın.\n"));
		}
		if !self.shutdown_attempted {
			note.push_str(&tr!(
				"Bu iş kaynak VM'ye kapama komutu göndermedi; güncel güç durumu ayrıca sorgulanmalıdır.\n"
			));
		}
		if self.cfg.method == Method::LiveMirror
			&& self.start_attempted
			&& !self.journal.dir.join("mirror-local.verified").is_file()
		{
			note.push_str(&tr!("Live Mirror henüz bağımsız hedefi doğrulamadı. {} altındaki NBD/SSHFS ve staging overlay hedef diskin parçası olabilir. Bağımlılıklar incelenmeden bu süreçleri veya sunucuyu yeniden başlatmayın.\n", self.run.display()));
		}
		if !self.source_lock.is_empty() {
			note.push_str(&tr!("Kaynak kilidi yolu: {}\n", self.source_lock));
		}
		note.push_str(&tr!("\nKomutların tam çıktısı commands/ altında, kaynak envanteri resources.jsonl içindedir.\nHata halinde kaynak, snapshot ve yarım hedefler otomatik silinmez.\nÖnce ESXi/PVE güç ve disk yapılandırmasını doğrulayın; incelemeden qm unlock/başlatma/silme yapmayın.\nYalnızca bitmiş işe ait boş kilit, o işin veya bağımlı süreçlerin çalışmadığı doğrulandıktan sonra kaldırılabilir.\nKomut zaman aşımında veya SIGTERM/SIGKILL sonrasında uzaktaki işler devam ediyor olabilir.\nYeni çalıştırma kaldığı yerden sürdürmez (resume yok); tamamlanma: phase=completed.\n"));
		process::atomic_write(&self.journal.dir.join("RECOVERY.txt"), note)?;
		Ok(())
	}
	fn confirm(&mut self, message: &str) -> Result<()> {
		if self.cfg.yes {
			return self
				.journal
				.log("INFO", &tr!("--yes: {message}", message = message));
		}
		ensure(
			io::stdin().is_terminal(),
			tr!("Onay için terminal gerekli; otomasyonda --yes seçin"),
		)?;
		eprint!("{}", tr!("{message} [EVET yazın]: ", message = message));
		io::stderr().flush()?;
		let mut s = String::new();
		io::stdin().read_line(&mut s)?;
		ensure(s.trim() == tr!("EVET"), tr!("Operatör iptal etti"))
	}
	fn preflight(&mut self) -> Result<()> {
		let status = fs::read_to_string("/proc/self/status")?;
		let euid = status
			.lines()
			.find(|l| l.starts_with("Uid:"))
			.and_then(|l| l.split_whitespace().nth(2))
			.unwrap_or("");
		ensure(
			euid == "0",
			tr!("Hedef PVE düğümünde root olarak çalıştırın"),
		)?;
		let mut tools = vec![
			"ssh",
			"sshfs",
			"qemu-img",
			"qemu-nbd",
			"qm",
			"pvesm",
			"fusermount3",
			"findmnt",
			"kill",
		];
		tools.sort_unstable();
		let missing: Vec<_> = tools
			.into_iter()
			.filter(|t| !process::find_program(t))
			.collect();
		ensure(
			missing.is_empty(),
			tr!("Eksik araçlar: {}. Paket kurulmadı.", missing.join(", ")),
		)?;
		ensure(
			self.cfg.known_hosts.is_file(),
			tr!("Doğrulanmış known_hosts okunamıyor"),
		)?;
		if let Some(key) = &self.cfg.ssh_key {
			ensure(key.is_file(), tr!("SSH anahtarı bulunamadı"))?;
		}
		ensure(
			Path::new("/sys/class/net")
				.join(&self.cfg.bridge)
				.join("bridge")
				.is_dir(),
			tr!("PVE Linux köprüsü bulunamadı"),
		)?;
		let file = OpenOptions::new()
			.create(true)
			.truncate(false)
			.read(true)
			.write(true)
			.mode(0o600)
			.open(
				self.cfg
					.run_root
					.join(format!("vmid-{}.lock", self.cfg.vmid)),
			)?;
		file.try_lock()
			.map_err(|_| Error(tr!("Hedef VMID başka aktarım tarafından kilitli")))?;
		self.local_lock = Some(file);
		// qm list başarısızsa mevcut VM yok varsayımı yapılmaz.
		let list = self.journal.run("qm", ["list"])?;
		let exists = list.lines().skip(1).any(|l| {
			l.split_whitespace()
				.next()
				.and_then(|s| s.parse::<u32>().ok())
				== Some(self.cfg.vmid)
		});
		ensure(!exists, tr!("Hedef VMID zaten var; üzerine yazılmadı"))
	}
	fn connect(&mut self) -> Result<()> {
		let socket = self.run.join("ssh");
		self.ssh = vec![
			"-p".into(),
			self.cfg.ssh_port.to_string(),
			"-o".into(),
			"StrictHostKeyChecking=yes".into(),
			"-o".into(),
			format!(
				"UserKnownHostsFile={}",
				crate::transport::ssh_config_path(&self.cfg.known_hosts.to_string_lossy())?
			),
			"-o".into(),
			"ConnectTimeout=15".into(),
			"-o".into(),
			"ServerAliveInterval=15".into(),
			"-o".into(),
			"ServerAliveCountMax=3".into(),
			"-o".into(),
			format!(
				"ControlPath={}",
				crate::transport::ssh_config_path(&socket.to_string_lossy())?
			),
			"-o".into(),
			"ControlMaster=no".into(),
		];
		if let Some(key) = &self.cfg.ssh_key {
			self.ssh.extend([
				"-i".into(),
				key.to_string_lossy().into_owned(),
				"-o".into(),
				"IdentitiesOnly=yes".into(),
			]);
		}
		let mut key = self.ssh.clone();
		key.extend([
			"-o".into(),
			"BatchMode=yes".into(),
			"-o".into(),
			"ControlMaster=yes".into(),
			"-o".into(),
			"ControlPersist=86400".into(),
			"-MNf".into(),
			self.destination(),
		]);
		if self.journal.attempt("ssh", key).is_err() {
			ensure(
				io::stdin().is_terminal(),
				tr!("Anahtarla SSH başarısız; parola için terminal gerekli"),
			)?;
			self.journal
				.log("INFO", &tr!("Parolayı OpenSSH soracak; parola kaydedilmez"))?;
			let mut args = self.ssh.clone();
			args.extend([
				"-o".into(),
				"BatchMode=no".into(),
				"-o".into(),
				"ControlMaster=yes".into(),
				"-o".into(),
				"ControlPersist=86400".into(),
				"-o".into(),
				"PreferredAuthentications=keyboard-interactive,password".into(),
				"-MNf".into(),
				self.destination(),
			]);
			let mut spec = CommandSpec::new("ssh", args, self.cfg.command_timeout);
			spec.interactive = true;
			self.journal.run_spec(spec)?;
		}
		for tool in [
			"vim-cmd",
			"vmkfstools",
			"sha256sum",
			"dd",
			"stat",
			"esxcli",
			"readlink",
		] {
			self.remote(&["which", tool])?;
		}
		Ok(())
	}
	fn destination(&self) -> String {
		format!("{}@{}", self.cfg.ssh_user, self.cfg.esxi)
	}
	fn remote(&mut self, args: &[&str]) -> Result<String> {
		let command = args
			.iter()
			.map(|s| process::remote_quote(s))
			.collect::<Vec<_>>()
			.join(" ");
		self.remote_command(command)
	}
	fn remote_command(&mut self, command: String) -> Result<String> {
		let mut args = vec!["-n".into()];
		args.extend(self.ssh.clone());
		args.extend([
			"-o".into(),
			"BatchMode=yes".into(),
			self.destination(),
			command,
		]);
		self.journal.run("ssh", args)
	}
	fn source_state(&mut self) -> Result<bool> {
		let text = self.remote(&[
			"vim-cmd",
			"vmsvc/power.getstate",
			&self.cfg.esxi_vmid.to_string(),
		])?;
		let on = text.lines().any(|s| s.trim() == "Powered on");
		let off = text.lines().any(|s| s.trim() == "Powered off");
		ensure(on != off, tr!("Kaynak güç durumu belirsiz"))?;
		Ok(on)
	}
	fn require_source_off(&mut self) -> Result<()> {
		let on = self.source_state()?;
		ensure(
			!on,
			tr!("Kaynak açık; hedef başlatılamaz/aktarım bitirilemez"),
		)
	}
	fn refresh(&mut self, label: &str) -> Result<()> {
		let text = self.remote(&["cat", &self.vmx.clone()])?;
		process::write_private(&self.journal.dir.join(format!("{label}.vmx")), &text)?;
		let mut manifest = vmware::parse_vmx(&text, &self.vmx)?;
		if let Some(initial) = &self.initial {
			ensure(
				initial
					.disks
					.iter()
					.map(|d| &d.slot)
					.eq(manifest.disks.iter().map(|d| &d.slot))
					&& initial.cores == manifest.cores
					&& initial.memory == manifest.memory
					&& initial.firmware == manifest.firmware
					&& initial.secure_boot == manifest.secure_boot
					&& initial.controllers == manifest.controllers
					&& initial.nics == manifest.nics,
				tr!("Kaynak donanım aktarım sırasında değişti"),
			)?;
		}
		let mut seen = BTreeSet::new();
		for disk in &mut manifest.disks {
			let value = self.remote(&["readlink", "-f", &disk.remote])?;
			disk.remote = vmware::safe_remote(value.trim())?;
			ensure(
				seen.insert(disk.remote.clone()),
				tr!("Aynı kanonik disk birden fazla yuvaya bağlı"),
			)?;
		}
		self.current = Some(manifest);
		Ok(())
	}
	fn inspect(&mut self) -> Result<()> {
		let text = self.remote(&[
			"vim-cmd",
			"vmsvc/get.config",
			&self.cfg.esxi_vmid.to_string(),
		])?;
		process::write_private(&self.journal.dir.join("source-config.txt"), &text)?;
		let path = vmware::config_path(&text)?;
		let path = self.remote(&["readlink", "-f", &path])?;
		self.vmx = vmware::safe_remote(path.trim())?;
		self.refresh("initial")?;
		self.initial = self.current.clone();
		for (i, path) in self.paths().iter().enumerate() {
			let text = self.remote(&["cat", path])?;
			process::write_private(
				&self.journal.dir.join(format!("initial-disk-{i}.vmdk")),
				&text,
			)?;
			let descriptor = vmware::parse_descriptor(&text)?;
			ensure(
				descriptor.parent.is_none() && descriptor.parent_cid.is_none_or(|c| c == u32::MAX),
				tr!("Başlangıçta snapshot zinciri var; ESXi üzerinde önce birleştirin"),
			)?;
			self.sizes.push(descriptor.size);
		}
		let snapshots = self.remote(&[
			"vim-cmd",
			"vmsvc/snapshot.get",
			&self.cfg.esxi_vmid.to_string(),
		])?;
		ensure(
			!snapshots.contains("Snapshot Id"),
			tr!("Başlangıçta snapshot var; yabancı snapshot korunur"),
		)?;
		let on = self.source_state()?;
		self.journal.log(
			"INFO",
			&tr!(
				"Kaynak={} VMID={} güç={} disk={}",
				self.cfg.esxi,
				self.cfg.esxi_vmid,
				if on { tr!("açık") } else { tr!("kapalı") },
				self.sizes.len()
			),
		)?;
		self.save_state()
	}
	fn configure(&mut self) -> Result<()> {
		let m = self
			.initial
			.as_ref()
			.ok_or_else(|| Error(tr!("Kaynak incelenmedi")))?
			.clone();
		if self.cfg.disk_bus == "auto" {
			match m.controllers.as_slice() {
				[s] if s == "pvscsi" => {
					self.cfg.disk_bus = "scsi".into();
					self.cfg.scsi_hw = Some("pvscsi".into());
				}
				[s] if s == "lsilogic" => {
					self.cfg.disk_bus = "scsi".into();
					self.cfg.scsi_hw = Some("lsi".into());
				}
				_ => {
					return Err(Error(tr!(
						"Denetleyici otomatik eşlenemedi; --disk-bus/--scsihw ile açık seçim gerekli"
					)));
				}
			}
		}
		ensure(
			self.cfg.disk_bus != "scsi" || self.cfg.scsi_hw.is_some(),
			tr!("SCSI için --scsihw gerekli"),
		)?;
		if self.cfg.bios == "auto" {
			self.cfg.bios = if m.firmware == "efi" {
				"ovmf"
			} else {
				"seabios"
			}
			.into();
		}
		ensure(
			!m.secure_boot || self.cfg.bios == "ovmf",
			tr!("Secure Boot etkin kaynak BIOS kipine düşürülemez"),
		)?;
		self.journal.log(
			"INFO",
			&tr!(
				"PLAN: VMID={} cpu={} bellek={} bios={} disk={}/{:?} köprü={}",
				self.cfg.vmid,
				m.cores,
				m.memory,
				self.cfg.bios,
				self.cfg.disk_bus,
				self.cfg.scsi_hw,
				self.cfg.bridge
			),
		)?;
		if !self.cfg.execute {
			return Ok(());
		}
		let mut args = vec![
			"create".into(),
			self.vmid(),
			"--name".into(),
			self.cfg
				.name
				.clone()
				.unwrap_or_else(|| format!("esxi-{}", self.cfg.esxi_vmid)),
			"--cores".into(),
			m.cores.to_string(),
			"--memory".into(),
			m.memory.to_string(),
			"--cpu".into(),
			"host".into(),
			"--bios".into(),
			self.cfg.bios.clone(),
			"--agent".into(),
			"enabled=1".into(),
			"--onboot".into(),
			"0".into(),
			"--hotplug".into(),
			"0".into(),
			"--vga".into(),
			"std".into(),
		];
		if self.cfg.bios == "ovmf" {
			args.extend(["--machine".into(), "q35".into()]);
		}
		if let Some(scsi) = &self.cfg.scsi_hw {
			args.extend(["--scsihw".into(), scsi.clone()]);
		}
		if let Some(nic) = m.nics.first() {
			args.extend([
				"--net0".into(),
				format!("{}={},bridge={}", nic.model, nic.mac, self.cfg.bridge),
			]);
		}
		self.journal
			.resource("target-create-intent", &[&self.vmid()])?;
		self.create_attempted = true;
		self.save_state()?;
		self.journal.run("qm", args)?;
		self.created = true;
		self.save_state()?;
		self.journal
			.run("qm", ["set", &self.vmid(), "--lock", "migrate"])?;
		Ok(())
	}
	fn mount(&mut self) -> Result<()> {
		process::private_dir(&self.mount)?;
		let args = crate::transport::sshfs_args(&self.ssh, &self.destination(), &self.mount)?;
		self.mount_attempted = true;
		self.save_state()?;
		self.journal
			.resource("sshfs-intent", &[&self.mount.to_string_lossy()])?;
		self.journal.run("sshfs", args)?;
		self.mount_established = true;
		self.journal
			.resource("sshfs-mounted", &[&self.mount.to_string_lossy()])?;
		self.save_state()
	}
	fn unmount(&mut self) -> Result<()> {
		if self.mount_established {
			self.journal
				.run("fusermount3", ["-u", &self.mount.to_string_lossy()])?;
			self.mount_established = false;
			self.journal
				.resource("sshfs-unmounted", &[&self.mount.to_string_lossy()])?;
			self.save_state()?;
		}
		Ok(())
	}
	fn local(&self, remote: &str) -> Result<PathBuf> {
		vmware::local_path(&self.mount, remote)
	}
	fn validate(&self, path: &Path) -> Result<vmware::Descriptor> {
		vmware::validate_descriptor(path, &self.mount, 0)
	}
	fn source_space(&mut self, path: &str, required: u64) -> Result<()> {
		let text = self.remote(&["esxcli", "--formatter=csv", "storage", "filesystem", "list"])?;
		let free = vmware::source_free(&text, path)?;
		ensure(
			free >= required.saturating_add(16 * 1024 * 1024 * 1024),
			tr!(
				"ESXi boş alanı={free}; klon={required} + 16GiB snapshot rezervi gerekli",
				free = free,
				required = required
			),
		)?;
		self.journal.log(
			"INFO",
			&tr!(
				"ESXi alan={free}; sonraki klon={required}; rezerv=16GiB",
				free = free,
				required = required
			),
		)
	}
	fn snapshot(&mut self, label: &str) -> Result<()> {
		let before = self.paths();
		for path in &before {
			self.source_space(path, 0)?;
		}
		let name = format!("esxi2pve-{}-{label}", self.job);
		self.journal.resource("snapshot-intent", &[&name])?;
		self.remote(&[
			"vim-cmd",
			"vmsvc/snapshot.create",
			&self.cfg.esxi_vmid.to_string(),
			&name,
			&format!("esxi2pve {}", self.job),
			"0",
			"1",
		])?;
		let text = self.remote(&[
			"vim-cmd",
			"vmsvc/snapshot.get",
			&self.cfg.esxi_vmid.to_string(),
		])?;
		let id = vmware::snapshot_id(&text, &name)?;
		self.journal.resource("snapshot", &[&id, &name])?;
		self.refresh(&format!("snapshot-{label}"))?;
		ensure(
			before.iter().zip(self.paths()).all(|(a, b)| a != &b),
			tr!("Snapshot bütün diskleri yeni descriptor'a geçirmedi"),
		)?;
		self.journal.log(
			"INFO",
			&tr!(
				"Snapshot doğrulandı: {name} id={id}; memory=0 quiesce=1",
				name = name,
				id = id
			),
		)
	}
	fn stop_source(&mut self) -> Result<()> {
		self.phase("source_shutdown")?;
		self.confirm(&tr!(
			"Kaynak düzenli kapatılıp son fark aktarılacak. Devam?"
		))?;
		self.downtime_start = process::epoch();
		self.save_state()?;
		if self.source_state()? {
			self.shutdown_attempted = true;
			self.save_state()?;
			self.remote(&[
				"vim-cmd",
				"vmsvc/power.shutdown",
				&self.cfg.esxi_vmid.to_string(),
			])?;
		}
		let now = Instant::now();
		while now.elapsed() < self.cfg.shutdown_timeout {
			if !self.source_state()? {
				self.refresh("stopped")?;
				return self
					.journal
					.log("INFO", &tr!("Kaynak kapalı; zorla kapatma/reset yapılmadı"));
			}
			thread::sleep(Duration::from_secs(3));
		}
		Err(Error(tr!(
			"Düzenli kapanış zaman aşımı; kaynak zorla kapatılmadı"
		)))
	}
	fn storage(&mut self, name: &str) -> Result<(String, u64)> {
		let text = self.journal.run("pvesm", ["status", "--storage", name])?;
		vmware::storage_status(&text, name)
	}
	fn allocate(&mut self, i: usize, overlay: bool) -> Result<Target> {
		let store = if overlay {
			self.cfg.staging_storage.clone().unwrap_or_default()
		} else {
			self.cfg.storage.clone()
		};
		let (kind, _) = self.storage(&store)?;
		let format = if overlay { "qcow2" } else { "raw" };
		let suffix = if matches!(
			kind.as_str(),
			"dir" | "nfs" | "cifs" | "cephfs" | "btrfs" | "glusterfs"
		) {
			format!(".{format}")
		} else {
			String::new()
		};
		let name = format!("vm-{}-disk-{i}{suffix}", self.cfg.vmid);
		self.journal.resource("volume-intent", &[&store, &name])?;
		let text = self.journal.run(
			"pvesm",
			[
				"alloc",
				&store,
				&self.vmid(),
				&name,
				&self.sizes[i].div_ceil(1024).to_string(),
				"--format",
				format,
			],
		)?;
		let volume = vmware::allocated_volume(&text, &store)?;
		self.journal.resource("volume", &[&volume, format])?;
		let path = PathBuf::from(self.journal.run("pvesm", ["path", &volume])?.trim());
		ensure(
			path.is_absolute(),
			tr!("Hedef yerel mutlak yol değil; ham Ceph için krbd gerekli"),
		)?;
		if overlay {
			ensure(
				kind == "dir" && path.is_file(),
				tr!("Live Mirror staging yerel dir türünde olmalı"),
			)?;
			let fs = self.journal.run(
				"findmnt",
				["-n", "-o", "FSTYPE", "--target", &path.to_string_lossy()],
			)?;
			ensure(
				matches!(
					fs.trim(),
					"ext2" | "ext3" | "ext4" | "xfs" | "btrfs" | "zfs"
				),
				tr!("Staging gerçek yerel dosya sisteminde değil"),
			)?;
		} else {
			disk::target_size(&path, self.sizes[i])?;
		}
		Ok(Target { volume, path })
	}
	fn prepare_raw(&mut self) -> Result<()> {
		for i in 0..self.sizes.len() {
			let t = self.allocate(i, false)?;
			self.targets.push(t);
		}
		Ok(())
	}
	fn clone_disk(&mut self, i: usize, source: &str, label: &str) -> Result<String> {
		self.source_space(source, self.sizes[i])?;
		let parent = source.rsplit_once('/').unwrap().0;
		let clone = format!("{parent}/.esxi2pve-{}-{label}-{i}.vmdk", self.job);
		self.journal.resource("clone-intent", &[&clone])?;
		self.remote(&["vmkfstools", "-i", source, &clone, "-d", "thin"])?;
		self.journal.resource("clone", &[&clone])?;
		let path = self.local(&clone)?;
		let descriptor = self.validate(&path)?;
		ensure(
			descriptor.size == self.sizes[i],
			tr!("Klon kapasitesi değişti"),
		)?;
		let text = self.journal.run(
			"qemu-img",
			[
				"info",
				"--output=json",
				"-f",
				"vmdk",
				&path.to_string_lossy(),
			],
		)?;
		ensure(
			json::parse(&text)?.get("virtual-size")?.integer()? == self.sizes[i],
			tr!("QEMU klon kapasitesi uyuşmuyor"),
		)?;
		Ok(clone)
	}
	fn release_clone(&mut self, clone: &str) -> Result<()> {
		let expected = format!(".esxi2pve-{}-", self.job);
		ensure(
			clone
				.rsplit('/')
				.next()
				.is_some_and(|s| s.starts_with(&expected) && s.ends_with(".vmdk")),
			tr!("Klon bu işe ait değil"),
		)?;
		self.remote(&["vmkfstools", "-U", clone])?;
		self.journal.resource("clone-deleted", &[clone])
	}
	fn hash_target(&mut self, i: usize) -> Result<Vec<String>> {
		self.journal.log(
			"INFO",
			&tr!("Disk {i}: yerel SHA-256 taraması başladı", i = i),
		)?;
		let path = self.targets[i].path.clone();
		let size = self.sizes[i];
		let pending = self.journal.dir.join(format!("target-{i}.sha256.pending"));
		let mut stream = OpenOptions::new()
			.create(true)
			.truncate(true)
			.write(true)
			.mode(0o600)
			.open(&pending)?;
		let hashes = disk::checksums_with_progress(&path, size, BLOCK, |block, hash| {
			writeln!(stream, "{block} {hash}")?;
			stream.sync_data()?;
			self.journal.log(
				"PROGRESS",
				&tr!(
					"Disk {i}: SHA-256 blok {}/{}",
					block + 1,
					size.div_ceil(BLOCK),
					i = i
				),
			)
		})?;
		stream.sync_all()?;
		fs::rename(
			&pending,
			self.journal.dir.join(format!("target-{i}.sha256")),
		)?;
		File::open(&self.journal.dir)?.sync_all()?;
		self.journal.log(
			"INFO",
			&tr!("Disk {i}: {} SHA-256 blok hesaplandı", hashes.len(), i = i),
		)?;
		Ok(hashes)
	}
	fn full_copy(&mut self, i: usize, source: &str) -> Result<()> {
		let local = self.local(source)?;
		let d = self.validate(&local)?;
		ensure(d.size == self.sizes[i], tr!("Kaynak kapasitesi değişti"))?;
		let target = self.targets[i].path.clone();
		self.journal.run(
			"qemu-img",
			[
				"convert",
				"-p",
				"-n",
				"-f",
				"vmdk",
				"-O",
				"raw",
				&local.to_string_lossy(),
				&target.to_string_lossy(),
			],
		)?;
		File::options()
			.read(true)
			.write(true)
			.open(&target)?
			.sync_all()?;
		self.journal.run(
			"qemu-img",
			[
				"compare",
				"-p",
				"-f",
				"vmdk",
				"-F",
				"raw",
				&local.to_string_lossy(),
				&target.to_string_lossy(),
			],
		)?;
		self.hash_target(i)?;
		self.journal
			.resource("full-copy-verified", &[&i.to_string(), source])
	}
	fn start_nbd(&mut self, i: usize, source: &Path, label: &str) -> Result<usize> {
		let socket = self
			.run
			.join(format!("{label}-{i}-{}.sock", self.nbds.len()));
		let pid_file = socket.with_extension("pid");
		self.journal.resource(
			"nbd-intent",
			&[&socket.to_string_lossy(), &pid_file.to_string_lossy()],
		)?;
		self.journal.run(
			"qemu-nbd",
			vec![
				"--fork".into(),
				"--persistent".into(),
				"--read-only".into(),
				"--format=vmdk".into(),
				format!("--socket={}", socket.display()),
				format!("--pid-file={}", pid_file.display()),
				source.to_string_lossy().into_owned(),
			],
		)?;
		let pid = process::read_text(&pid_file)?
			.trim()
			.parse::<u32>()
			.map_err(|_| Error(tr!("NBD PID geçersiz")))?;
		ensure(pid > 1, tr!("NBD PID geçersiz"))?;
		self.journal
			.resource("nbd", &[&pid.to_string(), &socket.to_string_lossy()])?;
		self.nbds.push(Nbd {
			pid,
			socket,
			stopped: false,
		});
		Ok(self.nbds.len() - 1)
	}
	fn stop_nbd(&mut self, index: usize) -> Result<()> {
		let n = &self.nbds[index];
		if n.stopped {
			return Ok(());
		}
		let proc = PathBuf::from(format!("/proc/{}/cmdline", n.pid));
		if proc.exists() {
			let args = fs::read(&proc)?;
			if !args.is_empty() {
				let args: Vec<_> = args
					.split(|b| *b == 0)
					.map(|b| String::from_utf8_lossy(b).into_owned())
					.collect();
				ensure(
					args.first()
						.is_some_and(|s| Path::new(s).file_name().is_some_and(|v| v == "qemu-nbd"))
						&& args
							.iter()
							.any(|s| s == &format!("--socket={}", n.socket.display())),
					tr!("PID beklenen qemu-nbd/socket değil; sonlandırılmadı"),
				)?;
				self.journal.run("kill", ["-TERM", &n.pid.to_string()])?;
				let now = Instant::now();
				loop {
					if !proc.exists() || fs::read(&proc)?.is_empty() {
						break;
					}
					ensure(
						now.elapsed() < Duration::from_secs(10),
						tr!("NBD kapanmadı"),
					)?;
					thread::sleep(Duration::from_millis(100));
				}
			}
		}
		self.nbds[index].stopped = true;
		Ok(())
	}
	fn replay_clone(&mut self, i: usize, clone: &str, label: &str) -> Result<u64> {
		let path = self.local(clone)?;
		let descriptor = self.validate(&path)?;
		ensure(
			descriptor.parent.is_none()
				&& descriptor.extents.len() == 1
				&& matches!(descriptor.extents[0].0.as_str(), "VMFS" | "FLAT"),
			tr!("Klon tek ham extent olmalı"),
		)?;
		let remote = format!(
			"{}/{}",
			clone.rsplit_once('/').unwrap().0,
			descriptor.extents[0].1
		);
		let size = self.remote(&["stat", "-c", "%s", &remote])?;
		ensure(
			size.trim().parse::<u64>().ok() == Some(self.sizes[i]),
			tr!("Ham klon boyutu uyuşmuyor"),
		)?;
		let mut hashes = Vec::new();
		for block in 0..self.sizes[i].div_ceil(BLOCK) {
			// Blok özetleri ESXi'nin yerleşik /bin/sh boru hattında hesaplanır.
			// pipefail ve byte sayısı denetimi eksik/başarısız dd çıktısını reddeder.
			let command = format!(
				"set -eu; set -o pipefail; dd if={} bs={BLOCK} skip={block} count=1 | sha256sum",
				process::remote_quote(&remote)
			);
			let text = self.remote_command(command)?;
			let hash = text.split_whitespace().next().unwrap_or("");
			ensure(
				hash.len() == 64
					&& hash
						.bytes()
						.all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
				tr!("ESXi SHA-256 çıktısı geçersiz"),
			)?;
			hashes.push(hash.to_string());
		}
		disk::save_hashes(
			&self.journal.dir.join(format!("{label}-source-{i}.sha256")),
			&hashes,
		)?;
		let previous = disk::load_hashes(
			&self.journal.dir.join(format!("target-{i}.sha256")),
			self.sizes[i],
			BLOCK,
		)?;
		let ranges = disk::hash_diffs(&hashes, &previous, self.sizes[i], BLOCK)?;
		let bytes = ranges.iter().map(|r| r.length).sum();
		let ranges_file = self.journal.dir.join(format!("{label}-ranges-{i}.json"));
		process::write_private(&ranges_file, disk::encode_ranges(&ranges))?;
		if !ranges.is_empty() {
			let n = self.start_nbd(i, &path, label)?;
			disk::logical_copy(
				&self.nbds[n].socket,
				&self.targets[i].path,
				&ranges,
				self.sizes[i],
				&ranges_file.with_extension("verified.jsonl"),
			)?;
			self.stop_nbd(n)?;
		}
		let actual = self.hash_target(i)?;
		ensure(
			hashes == actual,
			tr!("Son klon ile hedef SHA-256 blokları eşit değil"),
		)?;
		self.journal
			.resource("clone-replay-verified", &[&i.to_string(), clone])?;
		Ok(bytes)
	}
	fn replay_delta(&mut self, i: usize, source: &str, label: &str) -> Result<u64> {
		let path = self.local(source)?;
		self.validate(&path)?;
		vmware::parent_link(&path, &self.local(&self.applied[i])?)?;
		let map = self.journal.run(
			"qemu-img",
			[
				"map",
				"--output=json",
				"-f",
				"vmdk",
				&path.to_string_lossy(),
			],
		)?;
		process::write_private(
			&self.journal.dir.join(format!("{label}-map-{i}.json")),
			&map,
		)?;
		let ranges = disk::map_ranges(&map, self.sizes[i])?;
		let bytes: u64 = ranges.iter().map(|r| r.length).sum();
		let file = self.journal.dir.join(format!("{label}-ranges-{i}.json"));
		process::write_private(&file, disk::encode_ranges(&ranges))?;
		self.journal.log(
			"INFO",
			&tr!(
				"Disk {i}: fark={bytes} bayt; mantıksal NBD okuması uygulanacak",
				i = i,
				bytes = bytes
			),
		)?;
		if !ranges.is_empty() {
			let n = self.start_nbd(i, &path, label)?;
			disk::logical_copy(
				&self.nbds[n].socket,
				&self.targets[i].path,
				&ranges,
				self.sizes[i],
				&file.with_extension("verified.jsonl"),
			)?;
			self.stop_nbd(n)?;
		}
		self.applied[i] = source.into();
		self.journal
			.resource("delta-verified", &[&i.to_string(), source])?;
		Ok(bytes)
	}
	fn replay(&mut self, i: usize, source: &str, label: &str) -> Result<u64> {
		if self.cfg.delta_engine == DeltaEngine::Qemu {
			self.replay_delta(i, source, label)
		} else {
			let clone = self.clone_disk(i, source, label)?;
			let bytes = self.replay_clone(i, &clone, label)?;
			self.release_clone(&clone)?;
			Ok(bytes)
		}
	}
	fn attach(&mut self, overlay: bool) -> Result<()> {
		for i in 0..self.sizes.len() {
			let vol = if overlay {
				&self.overlays[i].volume
			} else {
				&self.targets[i].volume
			};
			self.journal.run(
				"qm",
				[
					"set",
					&self.vmid(),
					"--skiplock",
					"1",
					&format!("--{}{i}", self.cfg.disk_bus),
					&format!("{vol},cache=none,aio=threads"),
				],
			)?;
		}
		self.journal.run(
			"qm",
			[
				"set",
				&self.vmid(),
				"--skiplock",
				"1",
				"--boot",
				&format!("order={}0", self.cfg.disk_bus),
			],
		)?;
		if self.cfg.bios == "ovmf" {
			self.journal.run(
				"qm",
				[
					"set",
					&self.vmid(),
					"--skiplock",
					"1",
					"--efidisk0",
					&format!(
						"{}:0,efitype=4m,pre-enrolled-keys={}",
						self.cfg.storage,
						u8::from(self.initial.as_ref().is_some_and(|m| m.secure_boot))
					),
				],
			)?;
		}
		Ok(())
	}
	fn start_target(&mut self) -> Result<()> {
		self.require_source_off()?;
		self.start_attempted = true;
		self.save_state()?;
		self.journal.run("qm", ["unlock", &self.vmid()])?;
		self.journal.run("qm", ["start", &self.vmid()])?;
		let state = self.journal.run("qm", ["status", &self.vmid()])?;
		ensure(
			state.trim() == "status: running",
			tr!("Hedef QEMU çalışmıyor"),
		)?;
		self.journal
			.log("INFO", &tr!("QEMU running; hizmet denetimi henüz geçmedi"))
	}
	fn health(&mut self) -> Result<()> {
		let timeout = if let Some((d, _)) = &self.health {
			*d
		} else {
			self.cfg
				.health
				.as_ref()
				.ok_or_else(|| Error(tr!("Sağlık denetimi yok")))?
				.timeout
		};
		let now = Instant::now();
		while now.elapsed() < timeout {
			let ready = if let Some((_, callback)) = &mut self.health {
				callback(self.cfg.vmid)?
			} else {
				let h = self.cfg.health.as_ref().unwrap();
				TcpStream::connect_timeout(
					&h.address,
					Duration::from_secs(3).min(timeout.saturating_sub(now.elapsed())),
				)
				.is_ok()
			};
			if ready {
				if self.first_ready.is_none() {
					self.first_ready = Some(process::epoch());
					self.save_state()?;
				}
				return self.journal.log(
					"INFO",
					&tr!(
						"Hizmet denetimi geçti; ilk hazır oluşa kadar kesinti={} sn",
						self.first_ready
							.unwrap()
							.saturating_sub(self.downtime_start)
					),
				);
			}
			thread::sleep(Duration::from_secs(2).min(timeout.saturating_sub(now.elapsed())));
		}
		Err(Error(tr!(
			"QEMU çalışıyor olabilir; hizmet hazır olma denetimi zaman aşımına uğradı"
		)))
	}
	fn finish_cold(&mut self) -> Result<()> {
		self.phase("target_configuration")?;
		self.require_source_off()?;
		self.attach(false)?;
		if self.cfg.start {
			self.start_target()?;
			self.phase("service_readiness")?;
			self.health()?;
		} else {
			self.journal.run("qm", ["unlock", &self.vmid()])?;
			self.journal.log(
				"INFO",
				&tr!("Hedef kapalı bırakıldı; hizmet kesintisi sona erdi sayılmadı"),
			)?;
		}
		Ok(())
	}
	fn offline(&mut self) -> Result<()> {
		self.prepare_raw()?;
		self.stop_source()?;
		self.phase("offline_copy")?;
		for (i, path) in self.paths().iter().enumerate() {
			self.full_copy(i, path)?;
		}
		self.finish_cold()
	}
	fn precopy(&mut self) -> Result<()> {
		ensure(
			self.source_state()?,
			tr!("Çevrimiçi ön kopya çalışan kaynak gerektirir; kapalı kaynak için offline seçin"),
		)?;
		let frozen = self.paths();
		self.applied = frozen.clone();
		self.snapshot("initial")?;
		self.phase("online_precopy")?;
		for (i, path) in frozen.iter().enumerate() {
			let clone = self.clone_disk(i, path, "initial")?;
			self.full_copy(i, &clone)?;
			self.release_clone(&clone)?;
		}
		Ok(())
	}
	fn live_clone(&mut self) -> Result<()> {
		self.prepare_raw()?;
		self.precopy()?;
		self.stop_source()?;
		self.phase("live_clone_final_replay")?;
		for (i, path) in self.paths().iter().enumerate() {
			let clone = self.clone_disk(i, path, "final")?;
			self.replay_clone(i, &clone, "final")?;
			self.release_clone(&clone)?;
		}
		self.finish_cold()
	}
	fn snapshot_iterative(&mut self) -> Result<()> {
		self.prepare_raw()?;
		self.precopy()?;
		let mut converged = false;
		for iteration in 1..=self.cfg.iterations {
			let label = format!("iter{iteration}");
			self.phase(&format!("snapshot_{label}"))?;
			let frozen = self.paths();
			self.snapshot(&label)?;
			let mut bytes = 0u64;
			for (i, path) in frozen.iter().enumerate() {
				bytes = bytes
					.checked_add(self.replay(i, path, &label)?)
					.ok_or_else(|| Error(tr!("Tur byte sayısı taşması")))?;
			}
			self.journal.log(
				"INFO",
				&tr!(
					"Tur={iteration} değişen={bytes} bayt",
					iteration = iteration,
					bytes = bytes
				),
			)?;
			if bytes <= self.cfg.convergence_mib * 1048576 {
				converged = true;
				break;
			}
		}
		if !converged && self.cfg.iterations > 0 {
			self.journal.log(
				"WARN",
				&tr!("Tur sınırında yakınsama sağlanmadı; son farkın aktarımı uzun sürebilir"),
			)?;
		}
		self.stop_source()?;
		self.phase("snapshot_final_replay")?;
		for (i, path) in self.paths().iter().enumerate() {
			self.replay(i, path, "final")?;
		}
		self.finish_cold()
	}
	fn qmp(&mut self, operation: impl FnOnce(&mut Qmp) -> Result<String>) -> Result<String> {
		let socket = PathBuf::from(format!("/run/qemu-server/{}.qmp", self.cfg.vmid));
		let mut q = Qmp::connect(&socket)?;
		let result = operation(&mut q);
		for row in &q.transcript {
			self.journal.resource("qmp", &[&row.encode()])?;
		}
		result
	}
	fn live_mirror(&mut self) -> Result<()> {
		let stage = self.cfg.staging_storage.clone().unwrap();
		let (kind, free) = self.storage(&stage)?;
		let required = self
			.sizes
			.iter()
			.try_fold(2 * 1024 * 1024 * 1024u64, |a, b| {
				a.checked_add(*b)
					.ok_or_else(|| Error(tr!("Staging kapasite taşması")))
			})?;
		ensure(
			kind == "dir" && free >= required,
			tr!("Staging yerel dir türünde olmalı; bütün sanal diskler +2GiB boş alan gerekli"),
		)?;
		for i in 0..self.sizes.len() {
			let target = self.allocate(i, true)?;
			self.overlays.push(target);
		}
		self.stop_source()?;
		self.phase("mirror_remote_boot")?;
		let mut rates = Vec::new();
		for (i, path) in self.paths().iter().enumerate() {
			let local = self.local(path)?;
			self.validate(&local)?;
			let n = self.start_nbd(i, &local, "source")?;
			let sample = disk::benchmark(
				&self.nbds[n].socket,
				self.cfg.max_read_latency_ms,
				self.cfg.min_read_mib,
				self.cfg.mirror_mib,
			)?;
			self.journal.log(
				"INFO",
				&tr!(
					"Disk {i} I/O: p95={:.2}ms ardışık={:.2}MiB/s aynalama={}MiB/s",
					sample.p95_ms,
					sample.sequential_mib,
					sample.mirror_mib,
					i = i
				),
			)?;
			rates.push(sample.mirror_mib);
			let overlay = self.overlays[i].path.to_string_lossy().into_owned();
			let backing = format!("nbd+unix:///?socket={}", self.nbds[n].socket.display());
			self.journal.run(
				"qemu-img",
				[
					"resize",
					"-f",
					"qcow2",
					&overlay,
					&self.sizes[i].to_string(),
				],
			)?;
			self.journal.run(
				"qemu-img",
				[
					"rebase", "-u", "-f", "qcow2", "-F", "raw", "-b", &backing, &overlay,
				],
			)?;
		}
		self.attach(true)?;
		self.start_target()?;
		self.phase("service_readiness")?;
		self.health()?;
		self.phase("mirror_storage_move")?;
		for (i, rate) in rates.iter().enumerate() {
			let overlay = self.overlays[i].path.to_string_lossy().into_owned();
			let device = self.qmp(|q| disk::active_device(&q.call("query-block")?, &overlay))?;
			// qm, QMP mirror/pivot ve kalıcı VM yapılandırmasını birlikte yönetir.
			self.journal.run(
				"qm",
				[
					"move-disk",
					&self.vmid(),
					&format!("{}{i}", self.cfg.disk_bus),
					&self.cfg.storage,
					"--format",
					"raw",
					"--delete",
					"0",
					"--bwlimit",
					&(rate * 1024).to_string(),
				],
			)?;
			let text = self
				.journal
				.run("qm", ["config", &self.vmid(), "--current", "1"])?;
			let volume = vmware::config_volume(&text, &format!("{}{i}", self.cfg.disk_bus))?;
			ensure(
				volume.starts_with(&format!("{}:", self.cfg.storage))
					&& volume != self.overlays[i].volume,
				tr!("Kalıcı hedef birim yanlış"),
			)?;
			self.journal.resource("mirror-target", &[&volume])?;
			let path = self.journal.run("pvesm", ["path", &volume])?;
			let path = path.trim().to_string();
			self.qmp(|q| {
				let blocks = q.call("query-block")?;
				let jobs = q.call("query-block-jobs")?;
				disk::verify_active(&blocks, &jobs, &device, &path)?;
				Ok(path.clone())
			})?;
			self.targets.push(Target {
				volume,
				path: path.into(),
			});
		}
		self.phase("service_readiness")?;
		self.health()?;
		self.require_source_off()?;
		process::atomic_write(
			&self.journal.dir.join("mirror-local.verified"),
			"all local\n",
		)?;
		Ok(())
	}
	fn disconnect(&mut self) -> Result<()> {
		let mut args = self.ssh.clone();
		args.extend(["-O".into(), "exit".into(), self.destination()]);
		self.journal.run("ssh", args)?;
		Ok(())
	}
	fn cleanup(&mut self) -> Result<()> {
		self.phase("cleanup")?;
		self.require_source_off()?;
		for i in 0..self.nbds.len() {
			self.stop_nbd(i)?;
		}
		self.unmount()?;
		if self.source_lock_held {
			self.remote(&["rmdir", &self.source_lock.clone()])?;
			self.source_lock_held = false;
			self.save_state()?;
		}
		self.disconnect()?;
		self.phase("completed")?;
		self.journal.log("INFO",&tr!("Aktarım doğrulandı. Kaynak VM/snapshotlar ve varsa staging overlay korunuyor; resources.jsonl envanterini inceleyin"))
	}
	fn perform(&mut self) -> Result<()> {
		self.phase("preflight")?;
		self.preflight()?;
		self.phase("source_connection")?;
		self.connect()?;
		self.perform_connected()
	}
	fn perform_connected(&mut self) -> Result<()> {
		self.phase("source_inspection")?;
		self.inspect()?;
		self.phase("source_mount")?;
		self.mount()?;
		if !self.cfg.execute {
			self.configure()?;
			self.unmount()?;
			self.disconnect()?;
			self.phase("planned")?;
			return self.journal.log(
				"INFO",
				&tr!("Plan tamamlandı; kaynak/hedef VM değiştirilmedi"),
			);
		}
		self.confirm(&tr!(
			"{}: ESXi {}/{} → PVE {}. Uygula?",
			self.cfg.method.name(),
			self.cfg.esxi,
			self.cfg.esxi_vmid,
			self.cfg.vmid
		))?;
		self.phase("source_lock")?;
		self.source_lock = format!("{}/.esxi2pve.lock", self.vmx.rsplit_once('/').unwrap().0);
		self.save_state()?;
		self.journal
			.resource("source-lock-intent", &[&self.source_lock.clone()])?;
		self.remote(&["mkdir", &self.source_lock.clone()])?;
		self.source_lock_held = true;
		self.save_state()?;
		// Kilit alındıktan sonra kaynak yeniden okunur; keşif ile uygulama arasındaki değişiklikler reddedilir.
		let before = self.paths();
		self.refresh("locked")?;
		ensure(
			before == self.paths(),
			tr!("Kaynak disk yolları keşif sonrası değişti"),
		)?;
		self.phase("target_creation")?;
		self.configure()?;
		match self.cfg.method {
			Method::Offline => self.offline()?,
			Method::LiveClone => self.live_clone()?,
			Method::SnapshotIterative => self.snapshot_iterative()?,
			Method::LiveMirror => self.live_mirror()?,
		};
		self.cleanup()
	}
}

#[cfg(test)]
#[path = "migration_tests.rs"]
mod tests;
