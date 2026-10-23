//! Gerçek QEMU ile VMDK snapshot, sıfır bölge, zincir ve NBD sınamaları.
//! Sadece test dizininde dosyalar yaratır; ESXi/PVE bağlantısı yoktur.
use libesxi2pve::{
	disk::{self, NbdReader, Range},
	json, sha256, vmware,
};
use std::{
	fs,
	io::{Read, Write},
	os::unix::net::UnixListener,
	path::{Path, PathBuf},
	process::{Child, Command, Stdio},
	thread,
	time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
struct Temp(PathBuf);
impl Temp {
	fn new() -> Self {
		let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
			.parent()
			.unwrap()
			.parent()
			.unwrap()
			.join(".t/qemu")
			.join(format!(
				"{}-{}",
				std::process::id(),
				SystemTime::now()
					.duration_since(UNIX_EPOCH)
					.unwrap()
					.as_nanos()
			));
		fs::create_dir_all(&p).unwrap();
		Self(p)
	}
	fn p(&self, name: &str) -> PathBuf {
		self.0.join(name)
	}
}
impl Drop for Temp {
	fn drop(&mut self) {
		let _ = fs::remove_dir_all(&self.0);
	}
}
fn run(tool: &str, args: &[&str]) -> String {
	let out = Command::new(tool)
		.args(args)
		.output()
		.unwrap_or_else(|e| panic!("{tool} gerekli; paket kurulmadı: {e}"));
	assert!(
		out.status.success(),
		"{tool}: {}",
		String::from_utf8_lossy(&out.stderr)
	);
	String::from_utf8(out.stdout).unwrap()
}
fn s(p: &Path) -> &str {
	p.to_str().unwrap()
}
struct Server(Child, PathBuf);
impl Server {
	fn new(path: &Path, socket: &Path, format: &str, read_only: bool) -> Self {
		let mut c = Command::new("qemu-nbd");
		c.args([
			"--persistent",
			"--shared=4",
			"--format",
			format,
			"--socket",
			s(socket),
		]);
		if read_only {
			c.arg("--read-only");
		}
		let child = c
			.arg(path)
			.stdout(Stdio::null())
			.stderr(Stdio::inherit())
			.spawn()
			.unwrap();
		let mut server = Self(child, socket.into());
		let now = Instant::now();
		while !socket.exists() {
			assert!(
				server.0.try_wait().unwrap().is_none(),
				"qemu-nbd erken kapandı"
			);
			assert!(now.elapsed() < Duration::from_secs(5));
			thread::sleep(Duration::from_millis(10));
		}
		server
	}
}
impl Drop for Server {
	fn drop(&mut self) {
		let _ = self.0.kill();
		let _ = self.0.wait();
		let _ = fs::remove_file(&self.1);
	}
}
fn create_layer(path: &Path, parent: &Path) {
	run(
		"qemu-img",
		&[
			"create",
			"-f",
			"vmdk",
			"-F",
			"vmdk",
			"-b",
			parent.file_name().unwrap().to_str().unwrap(),
			"-o",
			"subformat=twoGbMaxExtentSparse",
			s(path),
		],
	);
}
fn apply(layer: &Path, previous: &Path, target: &Path, temp: &Temp, tag: &str) -> u64 {
	vmware::parent_link(layer, previous).unwrap();
	vmware::validate_descriptor(layer, &temp.0, 0).unwrap();
	let text = run(
		"qemu-img",
		&["map", "--output=json", "-f", "vmdk", s(layer)],
	);
	let entries = json::parse(&text).unwrap();
	assert!(entries.array().unwrap().iter().any(|e| {
		e.optional("offset")
			.is_some_and(|o| o.integer().unwrap() != e.get("start").unwrap().integer().unwrap())
	}));
	let ranges = disk::map_ranges(&text, 8 * 1024 * 1024).unwrap();
	let socket = temp.p(&format!("{tag}.sock"));
	let _server = Server::new(layer, &socket, "vmdk", true);
	let bytes = disk::logical_copy(
		&socket,
		target,
		&ranges,
		8 * 1024 * 1024,
		&temp.p(&format!("{tag}.verified.jsonl")),
	)
	.unwrap();
	run(
		"qemu-img",
		&["compare", "-f", "vmdk", "-F", "raw", s(layer), s(target)],
	);
	bytes
}
#[test]
fn real_snapshot_layers_zero_and_revert() {
	let t = Temp::new();
	let base = t.p("base.vmdk");
	let one = t.p("one.vmdk");
	let two = t.p("two.vmdk");
	let target = t.p("target.raw");
	run(
		"qemu-img",
		&[
			"create",
			"-f",
			"vmdk",
			"-o",
			"subformat=twoGbMaxExtentSparse",
			s(&base),
			"8M",
		],
	);
	run(
		"qemu-io",
		&["-f", "vmdk", "-c", "write -P 0x77 0 8M", s(&base)],
	);
	run(
		"qemu-img",
		&["convert", "-f", "vmdk", "-O", "raw", s(&base), s(&target)],
	);
	create_layer(&one, &base);
	run(
		"qemu-io",
		&[
			"-f",
			"vmdk",
			"-c",
			"write -P 0xa5 1M 64K",
			"-c",
			"write -z 2M 64K",
			s(&one),
		],
	);
	let n = apply(&one, &base, &target, &t, "one");
	assert!(n > 0 && n < 8 * 1024 * 1024);
	create_layer(&two, &one);
	run(
		"qemu-io",
		&[
			"-f",
			"vmdk",
			"-c",
			"write -P 0x77 1M 64K",
			"-c",
			"write -P 0x55 3M 64K",
			s(&two),
		],
	);
	apply(&two, &one, &target, &t, "two");
	assert!(vmware::parent_link(&two, &base).is_err());
	let mut f = fs::OpenOptions::new().append(true).open(&one).unwrap();
	writeln!(f, "\n# chain verified").unwrap();
	let before = disk::checksums(&target, 8 * 1024 * 1024, 1024 * 1024).unwrap();
	let data = fs::read(&target).unwrap();
	assert_eq!(before[0], sha256::digest(&data[..1024 * 1024]));
	let external = run("sha256sum", &[s(&target)]);
	assert_eq!(
		external.split_whitespace().next().unwrap(),
		sha256::digest(&data)
	);
}
#[test]
fn real_remote_overlay_keeps_base_unchanged() {
	let t = Temp::new();
	let base = t.p("base.raw");
	let overlay = t.p("overlay.qcow2");
	let independent = t.p("independent.raw");
	let socket = t.p("base.sock");
	fs::write(&base, vec![0x77; 8 * 1024 * 1024]).unwrap();
	let original = disk::checksums(&base, 8 * 1024 * 1024, 1024 * 1024).unwrap();
	let _server = Server::new(&base, &socket, "raw", true);
	let uri = format!("nbd+unix:///?socket={}", socket.display());
	run(
		"qemu-img",
		&[
			"create",
			"-f",
			"qcow2",
			"-F",
			"raw",
			"-b",
			&uri,
			s(&overlay),
		],
	);
	run(
		"qemu-io",
		&["-f", "qcow2", "-c", "write -P 0xa5 1M 64K", s(&overlay)],
	);
	assert_eq!(
		disk::checksums(&base, 8 * 1024 * 1024, 1024 * 1024).unwrap(),
		original
	);
	run(
		"qemu-img",
		&[
			"convert",
			"-f",
			"qcow2",
			"-O",
			"raw",
			s(&overlay),
			s(&independent),
		],
	);
	run(
		"qemu-img",
		&[
			"compare",
			"-f",
			"qcow2",
			"-F",
			"raw",
			s(&overlay),
			s(&independent),
		],
	);
	let sample = disk::benchmark(&socket, 5000, 1, 64).unwrap();
	assert!(sample.mirror_mib > 0);
}
#[test]
fn writable_nbd_rejected_before_target_write() {
	let t = Temp::new();
	let base = t.p("base.raw");
	let target = t.p("target.raw");
	let socket = t.p("rw.sock");
	fs::write(&base, vec![5; 8192]).unwrap();
	fs::write(&target, vec![7; 8192]).unwrap();
	let _server = Server::new(&base, &socket, "raw", false);
	assert!(
		disk::logical_copy(
			&socket,
			&target,
			&[Range {
				start: 0,
				length: 512
			}],
			8192,
			&t.p("journal")
		)
		.is_err()
	);
	assert_eq!(fs::read(&target).unwrap(), vec![7; 8192]);
}
#[test]
fn truncated_nbd_reply_rejected() {
	let t = Temp::new();
	let socket = t.p("short.sock");
	let listener = UnixListener::bind(&socket).unwrap();
	let server = thread::spawn(move || {
		let (mut s, _) = listener.accept().unwrap();
		s.write_all(b"NBDMAGIC").unwrap();
		s.write_all(&0x49484156454f5054u64.to_be_bytes()).unwrap();
		s.write_all(&3u16.to_be_bytes()).unwrap();
		let mut c = [0; 20];
		s.read_exact(&mut c).unwrap();
		s.write_all(&8192u64.to_be_bytes()).unwrap();
		s.write_all(&3u16.to_be_bytes()).unwrap();
		let mut request = [0; 28];
		s.read_exact(&mut request).unwrap();
		s.write_all(&0x67446698u32.to_be_bytes()).unwrap();
		s.write_all(&0u32.to_be_bytes()).unwrap();
		s.write_all(&1u64.to_be_bytes()).unwrap();
		s.write_all(&[1, 2, 3]).unwrap();
	});
	let mut reader = NbdReader::connect(&socket).unwrap();
	assert!(reader.read(0, 512).is_err());
	server.join().unwrap();
}
#[test]
fn invalid_ranges_leave_target_unchanged() {
	let t = Temp::new();
	let target = t.p("target.raw");
	fs::write(&target, vec![3; 1024]).unwrap();
	assert!(
		disk::logical_copy(
			&t.p("missing.sock"),
			&target,
			&[Range {
				start: 1000,
				length: 512
			}],
			1024,
			&t.p("journal")
		)
		.is_err()
	);
	assert_eq!(fs::read(target).unwrap(), vec![3; 1024]);
}

#[test]
fn qmp_wire_ids_events_and_pivot_verification() {
	let t = Temp::new();
	let socket = t.p("qmp.sock");
	let listener = UnixListener::bind(&socket).unwrap();
	let server = thread::spawn(move || {
		use std::io::{BufRead, BufReader};
		let (stream, _) = listener.accept().unwrap();
		let mut stream = BufReader::new(stream);
		stream.get_mut().write_all(b"{\"QMP\":{}}\r\n").unwrap();
		for _ in 0..3 {
			let mut line = String::new();
			stream.read_line(&mut line).unwrap();
			let request = json::parse(&line).unwrap();
			let id = request.get("id").unwrap().integer().unwrap();
			let command = request.get("execute").unwrap().string().unwrap();
			stream
				.get_mut()
				.write_all(b"{\"event\":\"BLOCK_JOB_COMPLETED\"}\r\n")
				.unwrap();
			let value = match command {
				"qmp_capabilities" => "{}",
				"query-block" => {
					r#"[{"device":"drive-scsi0","inserted":{"image":{"filename":"/disk/target.raw","format":"raw"}}}]"#
				}
				"query-block-jobs" => "[]",
				_ => panic!("{command}"),
			};
			stream
				.get_mut()
				.write_all(format!("{{\"return\":{value},\"id\":{id}}}\r\n").as_bytes())
				.unwrap();
		}
	});
	let mut q = disk::Qmp::connect(&socket).unwrap();
	let blocks = q.call("query-block").unwrap();
	let jobs = q.call("query-block-jobs").unwrap();
	disk::verify_active(&blocks, &jobs, "drive-scsi0", "/disk/target.raw").unwrap();
	assert_eq!(q.events.len(), 3);
	assert!(q.transcript.len() >= 7);
	drop(q);
	server.join().unwrap();
}
