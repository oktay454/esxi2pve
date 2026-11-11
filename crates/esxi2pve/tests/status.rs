//! Eski iş adları ve yönlendirilen JSON çıktı için uyumluluk.
use libesxi2pve::json;
use std::{
	fs,
	path::PathBuf,
	process::Command,
	time::{SystemTime, UNIX_EPOCH},
};
struct State(PathBuf);
impl State {
	fn new() -> Self {
		let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
			.join("target/status-tests")
			.join(format!(
				"{}-{}",
				std::process::id(),
				SystemTime::now()
					.duration_since(UNIX_EPOCH)
					.unwrap()
					.as_nanos()
			))
			.join("1792768847-108281-3804955-18e131c4930d4f34");
		fs::create_dir_all(&path).unwrap();
		fs::write(path.join("state.json"),
			r#"{"job":"1792768847-108281-3804955-18e131c4930d4f34","phase":"completed","vmid":108281,"esxi":"192.0.2.10","esxi_vmid":72}"#).unwrap();
		Self(path)
	}
}
impl Drop for State {
	fn drop(&mut self) {
		let _ = fs::remove_dir_all(self.0.parent().unwrap());
	}
}
#[test]
fn redirected_status_preserves_json_and_reads_legacy_job_names() {
	let state = State::new();
	for locale in ["tr", "en"] {
		let output = Command::new(env!("CARGO_BIN_EXE_esxi2pve"))
			.env("ESXI2PVE_LANG", locale)
			.args(["status", state.0.to_str().unwrap()])
			.output()
			.unwrap();
		assert!(output.status.success());
		let value = json::parse(std::str::from_utf8(&output.stdout).unwrap()).unwrap();
		assert_eq!(value.get("phase").unwrap().string().unwrap(), "completed");
		assert_eq!(value.get("vmid").unwrap().integer().unwrap(), 108281);
		assert_eq!(
			value.encode(),
			json::parse(&fs::read_to_string(state.0.join("state.json")).unwrap())
				.unwrap()
				.encode()
		);
		assert!(output.stderr.is_empty());
	}
}
#[test]
fn explicit_json_matches_redirected_status_and_rejects_duplicate_flags() {
	let state = State::new();
	let output = Command::new(env!("CARGO_BIN_EXE_esxi2pve"))
		.args(["status", state.0.to_str().unwrap(), "--json"])
		.output()
		.unwrap();
	assert!(output.status.success());
	json::parse(std::str::from_utf8(&output.stdout).unwrap()).unwrap();
	let duplicate = Command::new(env!("CARGO_BIN_EXE_esxi2pve"))
		.args(["status", state.0.to_str().unwrap(), "--json", "--json"])
		.output()
		.unwrap();
	assert!(!duplicate.status.success());
}
