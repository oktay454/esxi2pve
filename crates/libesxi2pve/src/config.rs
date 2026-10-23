use crate::{Result, ensure};
use std::{net::SocketAddr, path::PathBuf, time::Duration};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
	Offline,
	LiveClone,
	SnapshotIterative,
	LiveMirror,
}
impl Method {
	pub fn parse(s: &str) -> Result<Self> {
		match s {
			"offline" => Ok(Self::Offline),
			"live-clone" => Ok(Self::LiveClone),
			"snapshot-iterative" | "snapshot-delta" => Ok(Self::SnapshotIterative),
			"live-mirror" => Ok(Self::LiveMirror),
			_ => Err(crate::Error(tr!("Bilinmeyen yöntem: {s}", s = s))),
		}
	}
	pub fn name(self) -> &'static str {
		match self {
			Self::Offline => "offline",
			Self::LiveClone => "live-clone",
			Self::SnapshotIterative => "snapshot-iterative",
			Self::LiveMirror => "live-mirror",
		}
	}
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeltaEngine {
	Qemu,
	Clone,
}
/// TCP yalnızca portun erişilebilirliğini doğrular. Uygulama sağlığı için kütüphane
/// tüketicisi `Migration::with_health_check` ile kendi denetimini sağlayabilir.
#[derive(Clone, Debug)]
pub struct HealthCheck {
	pub address: SocketAddr,
	pub timeout: Duration,
}
#[derive(Clone, Debug)]
pub struct Config {
	pub language: crate::i18n::Language,
	pub method: Method,
	pub esxi: String,
	pub esxi_vmid: u32,
	pub vmid: u32,
	pub storage: String,
	pub staging_storage: Option<String>,
	pub bridge: String,
	pub ssh_user: String,
	pub ssh_port: u16,
	pub ssh_key: Option<PathBuf>,
	pub known_hosts: PathBuf,
	pub disk_bus: String,
	pub scsi_hw: Option<String>,
	pub bios: String,
	pub name: Option<String>,
	pub execute: bool,
	pub yes: bool,
	pub start: bool,
	pub health: Option<HealthCheck>,
	pub shutdown_timeout: Duration,
	pub command_timeout: Duration,
	pub iterations: u32,
	pub convergence_mib: u64,
	pub delta_engine: DeltaEngine,
	pub mirror_mib: u64,
	pub max_read_latency_ms: u64,
	pub min_read_mib: u64,
	pub log_root: PathBuf,
	pub state_root: PathBuf,
	pub run_root: PathBuf,
}
impl Default for Config {
	fn default() -> Self {
		Self {
			language: crate::i18n::current_language(),
			method: Method::SnapshotIterative,
			esxi: String::new(),
			esxi_vmid: 0,
			vmid: 0,
			storage: String::new(),
			staging_storage: None,
			bridge: "vmbr0".into(),
			ssh_user: "root".into(),
			ssh_port: 22,
			ssh_key: None,
			known_hosts: PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
				.join(".ssh/known_hosts"),
			disk_bus: "auto".into(),
			scsi_hw: None,
			bios: "auto".into(),
			name: None,
			execute: false,
			yes: false,
			start: false,
			health: None,
			shutdown_timeout: Duration::from_secs(600),
			command_timeout: Duration::from_secs(86400),
			iterations: 6,
			convergence_mib: 64,
			delta_engine: DeltaEngine::Qemu,
			mirror_mib: 64,
			max_read_latency_ms: 25,
			min_read_mib: 20,
			log_root: "/var/log/esxi2pve".into(),
			state_root: "/var/lib/esxi2pve".into(),
			run_root: "/run/esxi2pve".into(),
		}
	}
}
pub(crate) fn identifier(s: &str) -> bool {
	!s.is_empty()
		&& s.len() <= 128
		&& s.as_bytes()[0].is_ascii_alphabetic()
		&& s.bytes()
			.all(|c| c.is_ascii_alphanumeric() || b"_.-".contains(&c))
}
impl Config {
	pub fn validate(&self) -> Result<()> {
		crate::i18n::with_language(self.language, || self.validate_inner())
	}
	fn validate_inner(&self) -> Result<()> {
		ensure(
			!self.esxi.is_empty()
				&& self.esxi.len() <= 253
				&& self.esxi.as_bytes()[0].is_ascii_alphanumeric()
				&& self
					.esxi
					.bytes()
					.all(|c| c.is_ascii_alphanumeric() || b".-".contains(&c)),
			tr!("--esxi IPv4/DNS adı olmalı"),
		)?;
		ensure(identifier(&self.ssh_user), tr!("Geçersiz SSH kullanıcısı"))?;
		ensure(
			self.esxi_vmid > 0 && self.vmid >= 100,
			tr!("ESXi VMID >0, PVE VMID >=100 olmalı"),
		)?;
		ensure(
			identifier(&self.storage) && identifier(&self.bridge),
			tr!("Geçersiz depolama/köprü adı"),
		)?;
		if let Some(s) = &self.staging_storage {
			ensure(identifier(s), tr!("Geçersiz staging depolaması"))?;
		}
		if let Some(n) = &self.name {
			ensure(
				!n.is_empty()
					&& n.len() <= 63
					&& n.as_bytes()[0].is_ascii_alphanumeric()
					&& n.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'),
				tr!("Geçersiz VM adı"),
			)?;
		}
		ensure(
			matches!(self.disk_bus.as_str(), "auto" | "scsi" | "sata"),
			tr!("Disk veri yolu auto/scsi/sata olmalı"),
		)?;
		ensure(
			self.scsi_hw
				.as_deref()
				.is_none_or(|s| matches!(s, "pvscsi" | "lsi")),
			tr!("SCSI pvscsi/lsi olmalı"),
		)?;
		ensure(
			matches!(self.bios.as_str(), "auto" | "seabios" | "ovmf"),
			tr!("BIOS auto/seabios/ovmf olmalı"),
		)?;
		ensure(
			self.iterations <= 16 && self.ssh_port > 0,
			tr!("Tur 0..16; SSH portu 1..65535 olmalı"),
		)?;
		for n in [
			self.convergence_mib,
			self.mirror_mib,
			self.max_read_latency_ms,
			self.min_read_mib,
			self.shutdown_timeout.as_secs(),
			self.command_timeout.as_secs(),
		] {
			ensure(
				(1..=99999999).contains(&n),
				tr!("Sayısal seçenek sınırı 1..99999999"),
			)?;
		}
		ensure(
			self.known_hosts.is_absolute(),
			tr!("known_hosts mutlak yol olmalı"),
		)?;
		if let Some(p) = &self.ssh_key {
			ensure(p.is_absolute(), tr!("SSH anahtarı mutlak yol olmalı"))?;
		}
		for p in [&self.log_root, &self.state_root, &self.run_root] {
			ensure(
				p.is_absolute(),
				tr!("Günlük/durum/çalışma dizini mutlak olmalı"),
			)?;
		}
		if let Some(h) = &self.health {
			ensure(
				h.address.port() > 0
					&& !h.address.ip().is_unspecified()
					&& (1..=99999999).contains(&h.timeout.as_secs()),
				tr!("Geçersiz TCP denetimi"),
			)?;
		}
		if self.method == Method::LiveMirror {
			ensure(self.start, tr!("Live Mirror --start gerektirir"))?;
			ensure(
				self.staging_storage
					.as_ref()
					.is_some_and(|s| s != &self.storage),
				tr!("Live Mirror farklı --staging-storage gerektirir"),
			)?;
		}
		Ok(())
	}
}

#[cfg(test)]
mod locale_tests {
	use crate::i18n::{Language, current_language, with_language};
	#[test]
	fn explicit_config_language_overrides_and_restores_callers_language() {
		with_language(Language::English, || {
			let config = super::Config {
				language: Language::Turkish,
				..Default::default()
			};
			assert!(
				config
					.validate()
					.unwrap_err()
					.to_string()
					.contains("--esxi IPv4/DNS adı olmalı")
			);
			assert_eq!(current_language(), Language::English);
		});
	}
}
