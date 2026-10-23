//! İnce CLI: aktarım işlemleri libesxi2pve kütüphanesindedir.
use libesxi2pve::tr;
use libesxi2pve::{Config, DeltaEngine, Error, HealthCheck, Method, Migration, Result, json};
use std::{
	collections::BTreeSet, env, net::SocketAddr, path::PathBuf, process::ExitCode, time::Duration,
};
fn help() -> &'static str {
	match libesxi2pve::i18n::current_language() {
		libesxi2pve::i18n::Language::Turkish => include_str!("../locales/tr/help.txt"),
		libesxi2pve::i18n::Language::English => include_str!("../locales/en/help.txt"),
	}
}

fn value(args: &[String], i: &mut usize) -> Result<String> {
	*i += 1;
	args.get(*i)
		.filter(|v| !v.starts_with("--"))
		.cloned()
		.ok_or_else(|| Error(tr!("{} değeri eksik", args[*i - 1])))
}
fn numeric<T: std::str::FromStr>(s: String, name: &str) -> Result<T> {
	s.parse()
		.map_err(|_| Error(tr!("{name} sayısal değeri geçersiz", name = name)))
}
fn parse(args: &[String]) -> Result<Config> {
	let mut cfg = Config {
		execute: args[0] == "migrate",
		..Config::default()
	};
	let mut method = false;
	let mut health_address = None;
	let mut health_timeout = 600u64;
	let mut i = 1;
	let mut seen = BTreeSet::new();
	while i < args.len() {
		let opt = &args[i];
		if !seen.insert(opt.clone()) {
			return Err(Error(tr!("Yinelenen seçenek: {opt}", opt = opt)));
		}
		match opt.as_str() {
			"--yes" => cfg.yes = true,
			"--start" => cfg.start = true,
			"--method" | "--mode" => {
				cfg.method = Method::parse(&value(args, &mut i)?)?;
				method = true;
			}
			"--esxi" => cfg.esxi = value(args, &mut i)?,
			"--esxi-vmid" => cfg.esxi_vmid = numeric(value(args, &mut i)?, opt)?,
			"--vmid" => cfg.vmid = numeric(value(args, &mut i)?, opt)?,
			"--storage" => cfg.storage = value(args, &mut i)?,
			"--staging-storage" => cfg.staging_storage = Some(value(args, &mut i)?),
			"--bridge" => cfg.bridge = value(args, &mut i)?,
			"--ssh-user" => cfg.ssh_user = value(args, &mut i)?,
			"--ssh-port" => cfg.ssh_port = numeric(value(args, &mut i)?, opt)?,
			"--ssh-key" => cfg.ssh_key = Some(value(args, &mut i)?.into()),
			"--known-hosts" => cfg.known_hosts = value(args, &mut i)?.into(),
			"--disk-bus" => cfg.disk_bus = value(args, &mut i)?,
			"--scsihw" => cfg.scsi_hw = Some(value(args, &mut i)?),
			"--bios" => cfg.bios = value(args, &mut i)?,
			"--name" => cfg.name = Some(value(args, &mut i)?),
			"--health-tcp" => {
				health_address = Some(value(args, &mut i)?.parse::<SocketAddr>().map_err(|_| {
					Error(tr!("--health-tcp IP:PORT olmalı; DNS adı kabul edilmez"))
				})?)
			}
			"--health-timeout" => health_timeout = numeric(value(args, &mut i)?, opt)?,
			"--shutdown-timeout" => {
				cfg.shutdown_timeout = Duration::from_secs(numeric(value(args, &mut i)?, opt)?)
			}
			"--command-timeout" => {
				cfg.command_timeout = Duration::from_secs(numeric(value(args, &mut i)?, opt)?)
			}
			"--iterations" => cfg.iterations = numeric(value(args, &mut i)?, opt)?,
			"--convergence-mib" => cfg.convergence_mib = numeric(value(args, &mut i)?, opt)?,
			"--mirror-mib" => cfg.mirror_mib = numeric(value(args, &mut i)?, opt)?,
			"--max-read-latency-ms" => {
				cfg.max_read_latency_ms = numeric(value(args, &mut i)?, opt)?
			}
			"--min-read-mib" => cfg.min_read_mib = numeric(value(args, &mut i)?, opt)?,
			"--delta-engine" => {
				cfg.delta_engine = match value(args, &mut i)?.as_str() {
					"qemu" => DeltaEngine::Qemu,
					"clone" => DeltaEngine::Clone,
					_ => return Err(Error(tr!("--delta-engine qemu|clone olmalı"))),
				}
			}
			"--log-root" => cfg.log_root = value(args, &mut i)?.into(),
			"--state-root" => cfg.state_root = value(args, &mut i)?.into(),
			"--run-root" => cfg.run_root = value(args, &mut i)?.into(),
			_ => return Err(Error(tr!("Bilinmeyen seçenek: {opt}", opt = opt))),
		}
		i += 1;
	}
	if !method {
		return Err(Error(tr!("--method gerekli")));
	}
	if seen.contains("--mode") && seen.contains("--method") {
		return Err(Error(tr!("--mode ve --method birlikte kullanılamaz")));
	}
	if seen.contains("--health-timeout") && health_address.is_none() {
		return Err(Error(tr!("--health-timeout, --health-tcp gerektirir")));
	}
	cfg.health = health_address.map(|address| HealthCheck {
		address,
		timeout: Duration::from_secs(health_timeout),
	});
	cfg.validate()?;
	if cfg.start && cfg.health.is_none() {
		return Err(Error(tr!("--start, --health-tcp gerektirir")));
	}
	Ok(cfg)
}
fn main_result() -> Result<()> {
	let args: Vec<_> = env::args().skip(1).collect();
	if args.is_empty() || args.iter().any(|s| s == "--help" || s == "-h") {
		print!("{}", help());
		return Ok(());
	}
	if args == ["--version"] {
		println!("esxi2pve {}", env!("CARGO_PKG_VERSION"));
		return Ok(());
	}
	if args[0] == "status" {
		if args.len() != 2 {
			return Err(Error(tr!("status için bir iş dizini gerekli")));
		}
		let path = PathBuf::from(&args[1]).join("state.json");
		let data = std::fs::read_to_string(path)?;
		let state = json::parse(&data)?;
		println!("{}", state.encode());
		return Ok(());
	}
	if !matches!(args[0].as_str(), "plan" | "migrate") {
		return Err(Error(tr!("Komut: plan | migrate | status")));
	}
	let report = Migration::new(parse(&args)?).run()?;
	println!(
		"{}",
		json::object([
			("job", json::text(report.job)),
			("phase", json::text(report.phase)),
			("state_dir", json::text(report.state_dir.to_string_lossy())),
			("log_file", json::text(report.log_file.to_string_lossy())),
			(
				"downtime_seconds",
				report
					.downtime_seconds
					.map(json::number)
					.unwrap_or(json::Value::Null)
			)
		])
		.encode()
	);
	Ok(())
}
fn main() -> ExitCode {
	match main_result() {
		Ok(()) => ExitCode::SUCCESS,
		Err(e) => {
			eprintln!("{}", tr!("HATA: {e}", e = e));
			ExitCode::FAILURE
		}
	}
}
#[cfg(test)]
mod tests {
	use super::*;
	fn args(tail: &str) -> Vec<String> {
		format!("plan --method snapshot-delta --esxi 192.0.2.10 --esxi-vmid 4 --vmid 104 --storage local-lvm {tail}").split_whitespace().map(str::to_string).collect()
	}
	#[test]
	fn plan_default_no_mutation() {
		let c = parse(&args("--iterations 0")).unwrap();
		assert!(!c.execute);
		assert_eq!(c.iterations, 0);
		assert_eq!(c.method, Method::SnapshotIterative);
	}
	#[test]
	fn unknown_duplicate_missing() {
		for s in [
			"--magic",
			"--vmid 105",
			"--iterations",
			"--iterations -1",
			"--start",
			"--health-timeout 3",
		] {
			assert!(parse(&args(s)).is_err(), "{s}");
		}
	}
	#[test]
	fn method_required() {
		assert!(parse(&["migrate".into()]).is_err());
	}
	#[test]
	fn live_mirror_gate() {
		let mut a = args("--start --health-tcp 192.0.2.30:443 --staging-storage local");
		a[2] = "live-mirror".into();
		assert!(parse(&a).is_ok());
	}
}
