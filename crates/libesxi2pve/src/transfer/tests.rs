use super::*;
use std::{
	collections::BTreeSet,
	io::{ErrorKind, Read},
	os::unix::net::UnixListener,
	path::PathBuf,
	sync::{Mutex, atomic::AtomicUsize},
	time::{SystemTime, UNIX_EPOCH},
};
struct Temp(PathBuf);
impl Temp {
	fn new() -> Self {
		let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
			.parent()
			.unwrap()
			.parent()
			.unwrap()
			.join(".t/pipeline")
			.join(format!(
				"{}-{}",
				std::process::id(),
				SystemTime::now()
					.duration_since(UNIX_EPOCH)
					.unwrap()
					.as_nanos()
			));
		std::fs::create_dir_all(&root).unwrap();
		Self(root)
	}
	fn p(&self, name: &str) -> PathBuf {
		self.0.join(name)
	}
	fn target(&self, size: u64) -> PathBuf {
		let path = self.p("target");
		File::create(&path).unwrap().set_len(size).unwrap();
		path
	}
}
impl Drop for Temp {
	fn drop(&mut self) {
		let _ = std::fs::remove_dir_all(&self.0);
	}
}
#[derive(Clone, Copy, Default)]
struct Behavior {
	reverse: bool,
	delay_ms: u64,
	mutate: bool,
	bad_handle: bool,
	truncate: bool,
	silent: bool,
}
struct Server {
	socket: PathBuf,
	thread: Option<thread::JoinHandle<()>>,
}
impl Server {
	fn new(temp: &Temp, source: Vec<u8>, behavior: Behavior) -> Self {
		let socket = temp.p("n");
		let listener = UnixListener::bind(&socket).unwrap();
		let thread = thread::spawn(move || {
			let (mut stream, _) = listener.accept().unwrap();
			stream.write_all(b"NBDMAGIC").unwrap();
			stream
				.write_all(&0x49484156454f5054u64.to_be_bytes())
				.unwrap();
			stream.write_all(&3u16.to_be_bytes()).unwrap();
			let mut hello = [0; 20];
			stream.read_exact(&mut hello).unwrap();
			stream
				.write_all(&(source.len() as u64).to_be_bytes())
				.unwrap();
			stream.write_all(&3u16.to_be_bytes()).unwrap();
			stream.set_nonblocking(true).unwrap();
			let mut header = [0; 28];
			let mut position = 0;
			let mut count = 0;
			let mut scheduled = VecDeque::new();
			let mut sending: Option<(Vec<u8>, usize)> = None;
			'connection: loop {
				loop {
					match stream.read(&mut header[position..]) {
						Ok(0) => break 'connection,
						Ok(n) => {
							position += n;
							if position == 28 {
								let handle = u64::from_be_bytes(header[8..16].try_into().unwrap());
								let offset =
									u64::from_be_bytes(header[16..24].try_into().unwrap()) as usize;
								let length =
									u32::from_be_bytes(header[24..28].try_into().unwrap()) as usize;
								scheduled.push_back((
									Instant::now() + Duration::from_millis(behavior.delay_ms),
									handle,
									offset,
									length,
									count,
								));
								count += 1;
								position = 0;
							}
						}
						Err(e) if e.kind() == ErrorKind::WouldBlock => break,
						Err(_) => break 'connection,
					}
				}
				if sending.is_none()
					&& !behavior.silent
					&& (!behavior.reverse || count >= 2)
					&& scheduled
						.front()
						.is_some_and(|(at, _, _, _, _)| *at <= Instant::now())
				{
					let (_, handle, offset, length, number) =
						if behavior.reverse && scheduled.len() >= 2 {
							scheduled.pop_back().unwrap()
						} else {
							scheduled.pop_front().unwrap()
						};
					let mut bytes = Vec::with_capacity(16 + length);
					bytes.extend(0x67446698u32.to_be_bytes());
					bytes.extend(0u32.to_be_bytes());
					bytes.extend((handle + u64::from(behavior.bad_handle)).to_be_bytes());
					if behavior.mutate && number > 0 {
						bytes.extend(vec![99; length]);
					} else {
						bytes.extend(&source[offset..offset + length]);
					}
					if behavior.truncate {
						bytes.truncate(19);
					}
					sending = Some((bytes, 0));
				}
				if let Some((bytes, pos)) = sending.as_mut() {
					match stream.write(&bytes[*pos..]) {
						Ok(n) => {
							*pos += n;
							if *pos == bytes.len() {
								sending = None;
								if behavior.truncate {
									break;
								}
							}
						}
						Err(e) if e.kind() == ErrorKind::WouldBlock => {}
						Err(_) => break,
					}
				}
				thread::sleep(Duration::from_millis(1));
			}
		});
		Self {
			socket,
			thread: Some(thread),
		}
	}
}
impl Drop for Server {
	fn drop(&mut self) {
		// copy dönüşünde bağlantı kapanmış, hedef işçileri toplanmış olmalıdır.
		if let Some(thread) = self.thread.take() {
			thread.join().unwrap();
		}
	}
}
fn source(size: usize) -> Vec<u8> {
	(0..size).map(|i| (i % 251) as u8).collect()
}
struct FaultTarget {
	local: LocalTarget,
	bad_once: bool,
	bad_always: bool,
	read_error: bool,
	sync_error: bool,
	write_error: bool,
	seen: Mutex<BTreeSet<u64>>,
	writes: AtomicUsize,
	syncs: AtomicUsize,
}
impl FaultTarget {
	fn new(path: &Path) -> Self {
		Self {
			local: LocalTarget::open(path, Verification::Buffered).unwrap(),
			bad_once: false,
			bad_always: false,
			read_error: false,
			sync_error: false,
			write_error: false,
			seen: Mutex::new(BTreeSet::new()),
			writes: AtomicUsize::new(0),
			syncs: AtomicUsize::new(0),
		}
	}
}
impl Target for FaultTarget {
	fn write(&self, offset: u64, bytes: &[u8]) -> Result<()> {
		self.writes.fetch_add(1, Ordering::Relaxed);
		if self.write_error {
			return Err(Error("injected write error".into()));
		}
		self.local.write(offset, bytes)
	}
	fn sync(&self) -> Result<()> {
		self.syncs.fetch_add(1, Ordering::Relaxed);
		if self.sync_error {
			return Err(Error("injected fsync EIO".into()));
		}
		self.local.sync()
	}
	fn read(&self, offset: u64, length: usize) -> Result<Vec<u8>> {
		if self.read_error {
			return Err(Error("injected read EIO".into()));
		}
		let mut bytes = self.local.read(offset, length)?;
		if self.bad_always || (self.bad_once && self.seen.lock().unwrap().insert(offset)) {
			bytes[0] ^= 1;
		}
		Ok(bytes)
	}
}
fn injected(
	temp: &Temp,
	server: &Server,
	target: Arc<FaultTarget>,
	ranges: &[Range],
	size: u64,
) -> Result<TransferReport> {
	copy_inner(
		&server.socket,
		target,
		Probe::new(&temp.p("target")),
		ranges,
		size,
		&temp.p("verified"),
		&TransferOptions {
			verification: Verification::Buffered,
			..TransferOptions::default()
		},
		|_| Ok(()),
	)
}
#[test]
fn out_of_order_reads_reach_the_correct_offsets_and_leave_other_bytes_unchanged() {
	let temp = Temp::new();
	let data = source(8704);
	let path = temp.target(data.len() as u64);
	let server = Server::new(
		&temp,
		data.clone(),
		Behavior {
			reverse: true,
			..Behavior::default()
		},
	);
	let report = copy(
		&server.socket,
		&path,
		&[
			Range {
				start: 512,
				length: 512,
			},
			Range {
				start: 8192,
				length: 512,
			},
		],
		data.len() as u64,
		&temp.p("verified"),
		&TransferOptions::default(),
		|_| Ok(()),
	)
	.unwrap();
	assert_eq!(report.verified_bytes, 1024);
	assert!(report.peak_in_flight >= 2);
	assert!(report.peak_buffer_bytes <= 128 * 1024 * 1024);
	let actual = std::fs::read(&path).unwrap();
	assert_eq!(&actual[512..1024], &data[512..1024]);
	assert_eq!(&actual[8192..], &data[8192..]);
	assert!(
		actual[..512]
			.iter()
			.chain(&actual[1024..8192])
			.all(|b| *b == 0)
	);
	let records = std::fs::read_to_string(temp.p("verified")).unwrap();
	assert_eq!(records.lines().count(), 2);
}
#[test]
fn one_checksum_mismatch_retries_only_its_region_and_preserves_the_original_hash() {
	let temp = Temp::new();
	let path = temp.target(8192);
	let data = source(8192);
	let server = Server::new(&temp, data.clone(), Behavior::default());
	let mut storage = FaultTarget::new(&path);
	storage.bad_once = true;
	let storage = Arc::new(storage);
	let report = injected(
		&temp,
		&server,
		storage.clone(),
		&[Range {
			start: 512,
			length: 512,
		}],
		8192,
	)
	.unwrap();
	assert_eq!(report.retries, 1);
	assert_eq!(storage.writes.load(Ordering::Relaxed), 2);
	assert_eq!(&std::fs::read(path).unwrap()[512..1024], &data[512..1024]);
	let records = std::fs::read_to_string(temp.p("verified")).unwrap();
	assert!(records.contains("\"attempt\":1"));
}
#[test]
fn a_second_mismatch_fails_and_no_writes_continue_after_return() {
	let temp = Temp::new();
	let path = temp.target(8192);
	let server = Server::new(&temp, source(8192), Behavior::default());
	let mut storage = FaultTarget::new(&path);
	storage.bad_always = true;
	let storage = Arc::new(storage);
	assert!(
		injected(
			&temp,
			&server,
			storage.clone(),
			&[Range {
				start: 0,
				length: 512
			}],
			8192
		)
		.is_err()
	);
	let writes = storage.writes.load(Ordering::Relaxed);
	thread::sleep(Duration::from_millis(30));
	assert_eq!(writes, 2);
	assert_eq!(storage.writes.load(Ordering::Relaxed), writes);
	assert!(
		std::fs::read_to_string(temp.p("verified"))
			.unwrap()
			.is_empty()
	);
}
#[test]
fn source_change_on_retry_is_rejected_before_rewriting_target() {
	let temp = Temp::new();
	let path = temp.target(8192);
	let server = Server::new(
		&temp,
		source(8192),
		Behavior {
			mutate: true,
			..Behavior::default()
		},
	);
	let mut storage = FaultTarget::new(&path);
	storage.bad_once = true;
	let storage = Arc::new(storage);
	assert!(
		injected(
			&temp,
			&server,
			storage.clone(),
			&[Range {
				start: 0,
				length: 512
			}],
			8192
		)
		.is_err()
	);
	assert_eq!(storage.writes.load(Ordering::Relaxed), 1);
}
#[test]
fn persistent_device_errors_are_not_retried_or_recorded_as_verified() {
	for kind in 0..3 {
		let temp = Temp::new();
		let path = temp.target(8192);
		let server = Server::new(&temp, source(8192), Behavior::default());
		let mut storage = FaultTarget::new(&path);
		storage.sync_error = kind == 0;
		storage.read_error = kind == 1;
		storage.write_error = kind == 2;
		let storage = Arc::new(storage);
		assert!(
			injected(
				&temp,
				&server,
				storage.clone(),
				&[Range {
					start: 0,
					length: 512
				}],
				8192
			)
			.is_err()
		);
		assert_eq!(storage.writes.load(Ordering::Relaxed), 1);
		assert!(
			std::fs::read_to_string(temp.p("verified"))
				.unwrap()
				.is_empty()
		);
	}
}
#[test]
fn global_retry_limit_stops_repeatedly_unreliable_storage() {
	let temp = Temp::new();
	let path = temp.target(32768);
	let server = Server::new(&temp, source(32768), Behavior::default());
	let mut storage = FaultTarget::new(&path);
	storage.bad_once = true;
	let ranges: Vec<_> = (0..4)
		.map(|i| Range {
			start: i * 4096,
			length: 512,
		})
		.collect();
	assert!(injected(&temp, &server, Arc::new(storage), &ranges, 32768).is_err());
	let trace = std::fs::read_to_string(temp.p("verified.retries.jsonl")).unwrap();
	assert_eq!(
		trace
			.lines()
			.filter(|line| line.contains("\"retry\""))
			.count(),
		4
	);
}
#[test]
fn malformed_truncated_or_timed_out_nbd_never_completes_transfer() {
	for behavior in [
		Behavior {
			bad_handle: true,
			..Behavior::default()
		},
		Behavior {
			truncate: true,
			..Behavior::default()
		},
		Behavior {
			silent: true,
			..Behavior::default()
		},
	] {
		let temp = Temp::new();
		let path = temp.target(8192);
		let server = Server::new(&temp, source(8192), behavior);
		let options = TransferOptions {
			read_timeout: Duration::from_millis(40),
			..TransferOptions::default()
		};
		assert!(
			copy(
				&server.socket,
				&path,
				&[Range {
					start: 0,
					length: 512
				}],
				8192,
				&temp.p("verified"),
				&options,
				|_| Ok(())
			)
			.is_err()
		);
		assert!(std::fs::read(&path).unwrap().iter().all(|b| *b == 0));
	}
}
#[test]
fn pipelining_hides_simulated_source_latency_without_changing_data() {
	let mut durations = Vec::new();
	for depth in [1, 2] {
		let temp = Temp::new();
		let size = 8 * 1024 * 1024;
		let path = temp.target(size);
		let data = source(size as usize);
		let server = Server::new(
			&temp,
			data.clone(),
			Behavior {
				delay_ms: 15,
				..Behavior::default()
			},
		);
		let ranges: Vec<_> = (0..64)
			.map(|i| Range {
				start: i * 131072,
				length: 65536,
			})
			.collect();
		let report = copy(
			&server.socket,
			&path,
			&ranges,
			size,
			&temp.p("verified"),
			&TransferOptions {
				max_depth: depth,
				..TransferOptions::default()
			},
			|_| Ok(()),
		)
		.unwrap();
		assert_eq!(report.verified_bytes, 4 * 1024 * 1024);
		assert_eq!(report.peak_in_flight, depth);
		let actual = std::fs::read(&path).unwrap();
		for range in &ranges {
			assert_eq!(
				&actual[range.start as usize..(range.start + range.length) as usize],
				&data[range.start as usize..(range.start + range.length) as usize]
			);
		}
		durations.push(report.elapsed_seconds);
	}
	// Zaman karşılaştırması ortam yüküne göre değişebilir; nicel garanti verilmez.
	eprintln!(
		"latency benchmark: depth=1 {:.3}s; depth=2 {:.3}s",
		durations[0], durations[1]
	);
}

#[test]
fn small_buffer_budget_handles_multiple_persistence_groups() {
	let temp = Temp::new();
	let size = 68 * 1024 * 1024;
	let data = source(size);
	let expected = sha256::digest(&data);
	let path = temp.target(size as u64);
	let server = Server::new(&temp, data, Behavior::default());
	let target = Arc::new(FaultTarget::new(&path));
	let report = copy_inner(
		&server.socket,
		target.clone(),
		Probe::new(&path),
		&[Range {
			start: 0,
			length: size as u64,
		}],
		size as u64,
		&temp.p("verified"),
		&TransferOptions {
			memory_bytes: 16 * 1024 * 1024,
			verification: Verification::Buffered,
			..TransferOptions::default()
		},
		|_| Ok(()),
	)
	.unwrap();
	assert_eq!(report.verified_bytes, size as u64);
	assert_eq!(report.verified_chunks, 17);
	assert!(report.peak_buffer_bytes <= 16 * 1024 * 1024);
	assert!(target.syncs.load(Ordering::Relaxed) >= 2);
	assert_eq!(sha256::digest(&std::fs::read(&path).unwrap()), expected);
	assert_eq!(
		std::fs::read_to_string(temp.p("verified"))
			.unwrap()
			.lines()
			.count(),
		17
	);
}
#[test]
fn worker_errors_follow_the_callers_explicit_language_scope() {
	for (language, expected) in [
		(
			crate::i18n::Language::Turkish,
			"İkinci disk doğrulaması başarısız",
		),
		(
			crate::i18n::Language::English,
			"Second disk verification failed",
		),
	] {
		let temp = Temp::new();
		let data = source(512);
		let path = temp.target(512);
		let server = Server::new(&temp, data, Behavior::default());
		let mut target = FaultTarget::new(&path);
		target.bad_always = true;
		let error = crate::i18n::with_language(language, || {
			injected(
				&temp,
				&server,
				Arc::new(target),
				&[Range {
					start: 0,
					length: 512,
				}],
				512,
			)
		})
		.unwrap_err();
		assert!(error.to_string().contains(expected), "{error}");
	}
}
