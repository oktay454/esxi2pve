//! Locale tests launch child processes; they never mutate global test environment.
use std::process::{Command, Output};
fn run(vars: &[(&str, &str)], args: &[&str]) -> Output {
	let mut command = Command::new(env!("CARGO_BIN_EXE_esxi2pve"));
	for name in ["ESXI2PVE_LANG", "LC_ALL", "LC_MESSAGES", "LANG"] {
		command.env_remove(name);
	}
	for (name, value) in vars {
		command.env(name, value);
	}
	command.args(args).output().unwrap()
}
#[test]
fn help_is_available_in_both_languages() {
	for (language, expected, absent) in
		[("tr", "Kullanım:", "Usage:"), ("en", "Usage:", "Kullanım:")]
	{
		let out = run(&[("ESXI2PVE_LANG", language)], &["--help"]);
		assert!(out.status.success());
		let text = String::from_utf8(out.stdout).unwrap();
		assert!(text.contains(expected));
		assert!(!text.contains(absent));
		assert!(text.contains("ESXI2PVE_LANG"));
	}
}
#[test]
fn standard_locale_precedence_is_observed() {
	let out = run(
		&[("LANG", "en_US.UTF-8"), ("LC_MESSAGES", "tr_TR.UTF-8")],
		&["--help"],
	);
	assert!(String::from_utf8(out.stdout).unwrap().contains("Kullanım:"));
	let out = run(
		&[
			("LANG", "tr_TR.UTF-8"),
			("LC_MESSAGES", "tr_TR.UTF-8"),
			("LC_ALL", "C.UTF-8"),
		],
		&["--help"],
	);
	assert!(String::from_utf8(out.stdout).unwrap().contains("Usage:"));
	let out = run(
		&[("LC_ALL", "en_US.UTF-8"), ("ESXI2PVE_LANG", "tr")],
		&["--help"],
	);
	assert!(String::from_utf8(out.stdout).unwrap().contains("Kullanım:"));
}
#[test]
fn empty_and_unsupported_locales_fall_back_predictably() {
	for vars in [
		vec![],
		vec![("LANG", "de_DE.UTF-8")],
		vec![("ESXI2PVE_LANG", "xx"), ("LANG", "tr_TR.UTF-8")],
	] {
		let out = run(&vars, &["--help"]);
		assert!(String::from_utf8(out.stdout).unwrap().contains("Usage:"));
	}
	let out = run(&[("LC_ALL", ""), ("LANG", "tr_TR.UTF-8")], &["--help"]);
	assert!(String::from_utf8(out.stdout).unwrap().contains("Kullanım:"));
}
#[test]
fn cli_and_library_errors_use_the_same_locale() {
	for (locale, prefix, missing, address) in [
		(
			"en",
			"ERROR:",
			"--method is required",
			"--esxi must be an IPv4 address or DNS name",
		),
		(
			"tr",
			"HATA:",
			"--method gerekli",
			"--esxi IPv4/DNS adı olmalı",
		),
	] {
		let out = run(&[("ESXI2PVE_LANG", locale)], &["migrate"]);
		assert!(!out.status.success());
		let stderr = String::from_utf8(out.stderr).unwrap();
		assert!(
			stderr.contains(prefix) && stderr.contains(missing),
			"{stderr}"
		);
		let out = run(
			&[("ESXI2PVE_LANG", locale)],
			&["plan", "--method", "offline"],
		);
		assert!(!out.status.success());
		let stderr = String::from_utf8(out.stderr).unwrap();
		assert!(
			stderr.contains(prefix) && stderr.contains(address),
			"{stderr}"
		);
	}
}
#[test]
fn payloads_are_preserved_without_translating_user_data() {
	let out = run(&[("ESXI2PVE_LANG", "en")], &["plan", "--Geçersiz-VM-adı"]);
	let stderr = String::from_utf8(out.stderr).unwrap();
	assert!(
		stderr.contains("Unknown option: --Geçersiz-VM-adı"),
		"{stderr}"
	);
}
#[test]
fn version_is_language_independent() {
	let tr = run(&[("ESXI2PVE_LANG", "tr")], &["--version"]);
	let en = run(&[("ESXI2PVE_LANG", "en")], &["--version"]);
	assert_eq!(tr.stdout, en.stdout);
	assert_eq!(
		String::from_utf8(en.stdout).unwrap(),
		format!("esxi2pve {}\n", env!("CARGO_PKG_VERSION"))
	);
}

#[test]
fn turkey_test_preserves_dotted_and_dotless_i_in_cli_errors() {
	for locale in ["C", "en_US.UTF-8", "tr_TR.UTF-8"] {
		for value in [
			"i",
			"İ",
			"ı",
			"I",
			"I\u{307}",
			"İstanbul",
			"Isparta",
			"ışık",
		] {
			let option = format!("--{value}");
			let output = run(&[("LC_ALL", locale), ("LANG", locale)], &["plan", &option]);
			assert!(!output.status.success(), "{locale}: {option}");
			let stderr = String::from_utf8(output.stderr).unwrap();
			assert!(stderr.contains(&option), "{locale}: {stderr}");
		}
	}
}
#[test]
fn turkey_test_ascii_options_do_not_follow_turkish_case_mapping() {
	for locale in ["C", "en_US.UTF-8", "tr_TR.UTF-8"] {
		let output = run(
			&[("LC_ALL", locale), ("LANG", locale)],
			&[
				"plan",
				"--method",
				"snapshot-iterative",
				"--esxi",
				"192.0.2.10",
				"--esxi-vmid",
				"72",
				"--vmid",
				"108281",
				"--storage",
				"ISCSI",
				"--iterations",
				"0",
				"--io-max-depth",
				"0",
			],
		);
		assert!(!output.status.success());
		let stderr = String::from_utf8(output.stderr).unwrap();
		assert!(stderr.contains("1..64"), "{locale}: {stderr}");
		assert!(
			!stderr.contains("Unknown option") && !stderr.contains("Bilinmeyen seçenek"),
			"{stderr}"
		);
	}
}
#[test]
fn turkey_test_status_preserves_unicode_paths_keys_and_values() {
	use std::{
		fs,
		path::PathBuf,
		time::{SystemTime, UNIX_EPOCH},
	};
	struct Temp(PathBuf);
	impl Drop for Temp {
		fn drop(&mut self) {
			let _ = fs::remove_dir_all(&self.0);
		}
	}
	let root = Temp(
		PathBuf::from(env!("CARGO_MANIFEST_DIR"))
			.join("target/turkey-tests")
			.join(format!(
				"İş-ışık-iİıI-{}-{}",
				std::process::id(),
				SystemTime::now()
					.duration_since(UNIX_EPOCH)
					.unwrap()
					.as_nanos()
			)),
	);
	fs::create_dir_all(&root.0).unwrap();
	let state = libesxi2pve::json::object([
		("job", libesxi2pve::json::text("İstanbul-Isparta-ışık-iİıI")),
		("phase", libesxi2pve::json::text("completed")),
		("vmid", libesxi2pve::json::number(108281)),
		("iİıI", libesxi2pve::json::text("I\u{307}")),
	]);
	fs::write(root.0.join("state.json"), state.encode()).unwrap();
	for locale in ["C", "en_US.UTF-8", "tr_TR.UTF-8"] {
		let output = run(
			&[("LC_ALL", locale), ("LANG", locale)],
			&["status", root.0.to_str().unwrap()],
		);
		assert!(output.status.success(), "{locale}: {:?}", output.stderr);
		let text = String::from_utf8(output.stdout).unwrap();
		assert_eq!(libesxi2pve::json::parse(&text).unwrap(), state);
	}
}
