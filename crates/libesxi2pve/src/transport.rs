//! SSHFS, ssh_command değerini kabuk gibi ayrıştırmaz. SSH seçenekleri
//! FUSE/SSHFS'nin yerel -o arayüzünden, tek tek argümanlar halinde iletilir.
use crate::{Error, Result, ensure};
use std::path::Path;

pub(crate) fn ssh_config_path(value: &str) -> Result<String> {
	ensure(
		!value.chars().any(|c| c.is_control()),
		tr!("SSH yolu kontrol karakteri içeriyor"),
	)?;
	Ok(format!(
		"\"{}\"",
		value.replace('\\', "\\\\").replace('\"', "\\\"")
	))
}

fn fuse_option(value: &str) -> Result<String> {
	ensure(
		!value.chars().any(|c| c.is_control()),
		tr!("SSHFS seçeneği kontrol karakteri içeriyor"),
	)?;
	// libfuse -o listesinde virgül ayırıcı, ters eğik çizgi kaçış karakteridir.
	Ok(value.replace('\\', "\\\\").replace(',', "\\,"))
}

pub(crate) fn sshfs_args(ssh: &[String], destination: &str, mount: &Path) -> Result<Vec<String>> {
	let mut result = vec![
		"-o".into(),
		"ro".into(),
		"-o".into(),
		"dir_cache=no".into(),
		"-o".into(),
		"attr_timeout=0".into(),
		"-o".into(),
		"entry_timeout=0".into(),
	];
	let mut options = ssh.iter();
	while let Some(flag) = options.next() {
		let value = options
			.next()
			.ok_or_else(|| Error(tr!("SSH seçeneği değeri eksik: {flag}", flag = flag)))?;
		match flag.as_str() {
			"-p" => result.extend(["-p".into(), value.clone()]),
			"-o" => result.extend(["-o".into(), fuse_option(value)?]),
			"-i" => result.extend([
				"-o".into(),
				fuse_option(&format!("IdentityFile={}", ssh_config_path(value)?))?,
			]),
			_ => {
				return Err(Error(tr!(
					"SSHFS'ye aktarılamayan SSH seçeneği: {flag}",
					flag = flag
				)));
			}
		}
	}
	result.extend([
		"-o".into(),
		"BatchMode=yes".into(),
		format!("{destination}:/vmfs/volumes"),
		mount.to_string_lossy().into_owned(),
	]);
	Ok(result)
}

#[cfg(test)]
mod tests {
	use super::*;
	use std::{
		fs,
		os::unix::fs::symlink,
		path::PathBuf,
		process::Command,
		time::{SystemTime, UNIX_EPOCH},
	};

	fn options() -> Vec<String> {
		[
			"-p",
			"2222",
			"-o",
			"ControlPath=/run/esxi2pve/job/ssh",
			"-o",
			"UserKnownHostsFile=/root/key files/hosts,backup",
			"-o",
			"StrictHostKeyChecking=yes",
			"-i",
			"/root/key files/id_rsa",
			"-o",
			"ControlMaster=no",
		]
		.into_iter()
		.map(str::to_string)
		.collect()
	}

	#[test]
	fn uses_native_options_without_shell_command() {
		let args = sshfs_args(&options(), "root@192.0.2.1", Path::new("/run/job/esxi")).unwrap();
		assert!(!args.iter().any(|a| a.starts_with("ssh_command=")));
		assert!(args.contains(&"IdentityFile=\"/root/key files/id_rsa\"".into()));
		assert!(args.contains(&"UserKnownHostsFile=/root/key files/hosts\\,backup".into()));
		assert!(args.contains(&"ControlPath=/run/esxi2pve/job/ssh".into()));
		assert!(args.contains(&"ro".into()));
		assert!(args.contains(&"BatchMode=yes".into()));
	}

	#[test]
	fn rejects_unknown_or_incomplete_options() {
		assert!(sshfs_args(&["-o".into()], "root@192.0.2.1", Path::new("/run/job")).is_err());
		assert!(
			sshfs_args(
				&["-X".into(), "yes".into()],
				"root@192.0.2.1",
				Path::new("/run/job")
			)
			.is_err()
		);
		assert!(fuse_option("ControlPath=/run/line\nbreak").is_err());
		assert_eq!(fuse_option(r"A=x\y,z").unwrap(), r"A=x\\y\,z");
	}

	fn sshfs_program() -> Command {
		let path = std::env::var_os("PATH")
			.and_then(|value| {
				std::env::split_paths(&value)
					.map(|dir| dir.join("sshfs"))
					.find(|p| p.is_file())
			})
			.expect("sshfs ayrıştırma sınamaları için sshfs gerekli; paket kurulmadı");
		Command::new(path)
	}

	struct Temp(PathBuf);
	impl Temp {
		fn new() -> Self {
			let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
				.join("target/sshfs-tests")
				.join(format!(
					"{}-{}",
					std::process::id(),
					SystemTime::now()
						.duration_since(UNIX_EPOCH)
						.unwrap()
						.as_nanos()
				));
			fs::create_dir_all(dir.join("mount")).unwrap();
			// SSHFS yalnızca bu sahte ssh'yi başlatabilir; ağa bağlanamaz.
			symlink("/bin/false", dir.join("ssh")).unwrap();
			Self(dir)
		}
	}
	impl Drop for Temp {
		fn drop(&mut self) {
			let _ = fs::remove_dir_all(&self.0);
		}
	}

	#[test]
	fn real_sshfs_parser_forwards_native_arguments_without_fuse_or_network() {
		let temp = Temp::new();
		let mut args = sshfs_args(&options(), "root@fake.invalid", &temp.0.join("mount")).unwrap();
		args.insert(0, "-d".into());
		let result = sshfs_program()
			.args(args)
			.env("PATH", &temp.0)
			.output()
			.expect("SSHFS ayrıştırma sınaması için sshfs gerekli; paket kurulmadı");
		let stderr = String::from_utf8_lossy(&result.stderr);
		// Sahte ssh bilerek 1 döner; bağlantı ve gerçek mount beklenmez.
		assert!(!result.status.success());
		assert!(stderr.contains("executing <ssh>"), "{stderr}");
		assert!(
			stderr.contains("<-oControlPath=/run/esxi2pve/job/ssh>"),
			"{stderr}"
		);
		assert!(
			stderr.contains("<-oUserKnownHostsFile=/root/key files/hosts,backup>"),
			"{stderr}"
		);
		assert!(
			stderr.contains("<-oIdentityFile=\"/root/key files/id_rsa\">"),
			"{stderr}"
		);
		assert!(!stderr.contains("failed to execute"), "{stderr}");
	}

	#[test]
	fn real_sshfs_reproduces_old_single_quote_failure() {
		let temp = Temp::new();
		let result = sshfs_program()
			.args([
				"-d",
				"-o",
				"ssh_command='ssh' '-o' 'BatchMode=yes'",
				"root@fake.invalid:/vmfs/volumes",
				temp.0.join("mount").to_str().unwrap(),
			])
			.env("PATH", &temp.0)
			.output()
			.expect("sshfs gerekli; paket kurulmadı");
		let stderr = String::from_utf8_lossy(&result.stderr);
		assert!(!result.status.success());
		assert!(stderr.contains("failed to execute ''ssh''"), "{stderr}");
		assert!(stderr.contains("Connection reset by peer"), "{stderr}");
	}
}

#[cfg(test)]
mod openssh_tests {
	#[test]
	fn quoted_file_paths_are_accepted_by_real_openssh_without_network() {
		let path = "/tmp/key files/id_rsa";
		let option = format!("IdentityFile={}", super::ssh_config_path(path).unwrap());
		let output = std::process::Command::new("ssh")
			.args(["-F", "/dev/null", "-G", "-o", &option, "fake.invalid"])
			.output()
			.expect("openssh-client gerekli; paket kurulmadı");
		assert!(
			output.status.success(),
			"{}",
			String::from_utf8_lossy(&output.stderr)
		);
		assert!(
			String::from_utf8_lossy(&output.stdout)
				.lines()
				.any(|line| line == format!("identityfile {path}"))
		);
	}
}
