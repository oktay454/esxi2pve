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
