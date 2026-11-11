//! Sınırlı, uyarlanabilir ve kalıcı doğrulamalı fark aktarımı.
mod controller;
mod nbd;
use crate::{
	Error, Result,
	disk::{self, Range},
	ensure, json, sha256,
};
use controller::{Controller, Probe, Samples};
use nbd::{Reader, Reply};
use std::{
	collections::VecDeque,
	fs::{File, OpenOptions},
	io::Write,
	os::unix::fs::{FileExt, OpenOptionsExt},
	path::Path,
	sync::{
		Arc,
		atomic::{AtomicBool, Ordering},
		mpsc::{self, Receiver, RecvTimeoutError, SyncSender, TryRecvError, TrySendError},
	},
	thread,
	time::{Duration, Instant},
};
pub const CHUNK_BYTES: usize = 4 * 1024 * 1024;
const GROUP_BYTES: usize = 64 * 1024 * 1024;
const GROUP_ITEMS: usize = 4096;
const MAX_RETRIES: u64 = 3;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verification {
	Direct,
	Buffered,
}
impl Verification {
	pub fn name(self) -> &'static str {
		match self {
			Self::Direct => "direct",
			Self::Buffered => "buffered",
		}
	}
}
#[derive(Clone, Debug)]
pub struct TransferOptions {
	pub max_depth: usize,
	pub memory_bytes: usize,
	pub verification: Verification,
	pub read_timeout: Duration,
}
impl Default for TransferOptions {
	fn default() -> Self {
		Self {
			max_depth: 16,
			memory_bytes: 128 * 1024 * 1024,
			verification: Verification::Direct,
			read_timeout: Duration::from_secs(120),
		}
	}
}
impl TransferOptions {
	pub fn validate(&self) -> Result<()> {
		ensure(
			(1..=64).contains(&self.max_depth)
				&& (16 * 1024 * 1024..=1024 * 1024 * 1024).contains(&self.memory_bytes)
				&& !self.read_timeout.is_zero(),
			tr!("G/Ç derinliği 1..64, bellek 16..1024 MiB ve zaman aşımı pozitif olmalı"),
		)
	}
}
#[derive(Clone, Debug, Default)]
pub struct TransferReport {
	pub planned_bytes: u64,
	pub verified_bytes: u64,
	pub verified_chunks: u64,
	pub retries: u64,
	pub read_requests: u64,
	pub peak_in_flight: usize,
	pub peak_buffer_bytes: usize,
	pub queue_depth: usize,
	pub read_ms: f64,
	pub hash_ms: f64,
	pub write_ms: f64,
	pub fsync_ms: f64,
	pub verify_ms: f64,
	pub journal_ms: f64,
	pub elapsed_seconds: f64,
}
impl TransferReport {
	pub fn json(&self, mode: Verification) -> json::Value {
		let micros = |ms: f64| json::number((ms.max(0.0) * 1000.0) as u64);
		json::object([
			("planned_bytes", json::number(self.planned_bytes)),
			("verified_bytes", json::number(self.verified_bytes)),
			("verified_chunks", json::number(self.verified_chunks)),
			("retries", json::number(self.retries)),
			("read_requests", json::number(self.read_requests)),
			("peak_in_flight", json::number(self.peak_in_flight as u64)),
			(
				"peak_buffer_bytes",
				json::number(self.peak_buffer_bytes as u64),
			),
			("queue_depth", json::number(self.queue_depth as u64)),
			("read_us", micros(self.read_ms)),
			("hash_us", micros(self.hash_ms)),
			("write_us", micros(self.write_ms)),
			("fsync_us", micros(self.fsync_ms)),
			("verify_us", micros(self.verify_ms)),
			("journal_us", micros(self.journal_ms)),
			(
				"elapsed_ms",
				json::number((self.elapsed_seconds * 1000.0) as u64),
			),
			("verification", json::text(mode.name())),
		])
	}
}
#[derive(Clone)]
struct Chunk {
	offset: u64,
	length: usize,
	retry_reply: Option<SyncSender<Result<Vec<u8>>>>,
}
struct Written {
	offset: u64,
	length: usize,
	hash: String,
}
enum Input {
	Data(Reply),
	End,
}
enum Event {
	Written {
		bytes: usize,
		hash_ms: f64,
		write_ms: f64,
	},
	Committed {
		bytes: u64,
		chunks: u64,
		hash_ms: f64,
		write_ms: f64,
		flush_ms: f64,
		verify_ms: f64,
		journal_ms: f64,
	},
	Retry {
		chunk: Chunk,
		count: u64,
	},
	Fatal(String),
	Done,
}
struct Plan<'a> {
	ranges: &'a [Range],
	index: usize,
	position: u64,
}
impl<'a> Plan<'a> {
	fn new(ranges: &'a [Range]) -> Self {
		Self {
			ranges,
			index: 0,
			position: ranges.first().map_or(0, |r| r.start),
		}
	}
	fn next(&mut self) -> Option<Chunk> {
		let range = self.ranges.get(self.index)?;
		let length =
			((range.start + range.length - self.position).min(CHUNK_BYTES as u64)) as usize;
		let chunk = Chunk {
			offset: self.position,
			length,
			retry_reply: None,
		};
		self.position += length as u64;
		if self.position == range.start + range.length {
			self.index += 1;
			if let Some(range) = self.ranges.get(self.index) {
				self.position = range.start;
			}
		}
		Some(chunk)
	}
}
trait Target: Send + Sync {
	fn write(&self, offset: u64, bytes: &[u8]) -> Result<()>;
	fn sync(&self) -> Result<()>;
	fn read(&self, offset: u64, length: usize) -> Result<Vec<u8>>;
}
struct LocalTarget {
	writer: File,
	reader: File,
	mode: Verification,
}
impl LocalTarget {
	fn open(path: &Path, mode: Verification) -> Result<Self> {
		// Linux asm-generic/x86 O_DIRECT. Uygulamanın desteklenen GNU amd64 hedefi.
		#[cfg(not(any(target_arch = "x86_64", target_arch = "x86", target_arch = "aarch64")))]
		ensure(
			mode != Verification::Direct,
			tr!("Bu mimaride doğrudan G/Ç doğrulaması desteklenmiyor"),
		)?;
		let mut reader = OpenOptions::new();
		reader.read(true);
		if mode == Verification::Direct {
			reader.custom_flags(0x4000);
		}
		Ok(Self {
			writer: OpenOptions::new().read(true).write(true).open(path)?,
			reader: reader.open(path).map_err(|error| {
				Error(tr!("Doğrulama okuyucusu açılamadı: {error}", error = error))
			})?,
			mode,
		})
	}
}
impl Target for LocalTarget {
	fn write(&self, offset: u64, bytes: &[u8]) -> Result<()> {
		self.writer.write_all_at(bytes, offset)?;
		Ok(())
	}
	fn sync(&self) -> Result<()> {
		self.writer.sync_all()?;
		Ok(())
	}
	fn read(&self, offset: u64, length: usize) -> Result<Vec<u8>> {
		if self.mode == Verification::Buffered {
			let mut bytes = vec![0; length];
			self.reader.read_exact_at(&mut bytes, offset)?;
			return Ok(bytes);
		}
		// Güvenli Rust ile hizalı alt dilim; yeniden yazma yok, unsafe yok.
		const ALIGN: usize = 4096;
		let base = offset / ALIGN as u64 * ALIGN as u64;
		let prefix = (offset - base) as usize;
		let wanted = prefix + length;
		let count = wanted.div_ceil(ALIGN) * ALIGN;
		let mut allocation = vec![0; count + ALIGN];
		let shift = (ALIGN - allocation.as_ptr() as usize % ALIGN) % ALIGN;
		let bytes = &mut allocation[shift..shift + count];
		let mut read = 0;
		while read < wanted {
			match self.reader.read_at(&mut bytes[read..], base + read as u64) {
				Ok(0) => return Err(Error(tr!("Doğrudan geri okuma eksik kaldı"))),
				Ok(n) => read += n,
				Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
				Err(e) => {
					return Err(Error(tr!(
						"Doğrudan geri okuma başarısız: {error}",
						error = e
					)));
				}
			}
		}
		Ok(bytes[prefix..wanted].to_vec())
	}
}
/// Kaynağı kapatmadan önce yerel hedefin doğrulama yolunu denetler.
pub fn probe_target(path: &Path, size: u64, mode: Verification) -> Result<()> {
	disk::target_size(path, size)?;
	if size > 0 {
		LocalTarget::open(path, mode)?.read(0, size.min(4096) as usize)?;
	}
	Ok(())
}
fn sync_parent(path: &Path) -> Result<()> {
	let parent = path
		.parent()
		.filter(|p| !p.as_os_str().is_empty())
		.unwrap_or_else(|| Path::new("."));
	File::open(parent)?.sync_all()?;
	Ok(())
}
fn milliseconds(start: Instant) -> f64 {
	start.elapsed().as_secs_f64() * 1000.0
}
fn send_event(events: &mpsc::Sender<Event>, event: Event) -> Result<()> {
	events
		.send(event)
		.map_err(|_| Error(tr!("Aktarım olay kanalı kapandı")))
}
fn write_stage(
	target: &dyn Target,
	input: Receiver<Input>,
	groups: SyncSender<Vec<Written>>,
	events: mpsc::Sender<Event>,
	stop: &AtomicBool,
) -> Result<()> {
	let mut group = Vec::new();
	let mut bytes = 0;
	loop {
		if stop.load(Ordering::Acquire) {
			return Ok(());
		}
		let packet = match input.recv_timeout(Duration::from_millis(100)) {
			Ok(Input::Data(packet)) => Some(packet),
			Ok(Input::End) | Err(RecvTimeoutError::Disconnected) => {
				if !group.is_empty() {
					send_group(&groups, group, stop)?;
				}
				return Ok(());
			}
			Err(RecvTimeoutError::Timeout) => {
				if !group.is_empty() {
					send_group(&groups, std::mem::take(&mut group), stop)?;
					bytes = 0;
				}
				continue;
			}
		};
		if let Some(packet) = packet {
			let started = Instant::now();
			let hash = sha256::digest(&packet.bytes);
			let hash_ms = milliseconds(started);
			let started = Instant::now();
			target
				.write(packet.chunk.offset, &packet.bytes)
				.map_err(|error| {
					Error(tr!(
						"Hedef yazma başarısız; adres={offset}: {error}",
						offset = packet.chunk.offset,
						error = error
					))
				})?;
			let write_ms = milliseconds(started);
			let length = packet.bytes.len();
			group.push(Written {
				offset: packet.chunk.offset,
				length,
				hash,
			});
			drop(packet);
			send_event(
				&events,
				Event::Written {
					bytes: length,
					hash_ms,
					write_ms,
				},
			)?;
			bytes += length;
			if bytes >= GROUP_BYTES || group.len() >= GROUP_ITEMS {
				send_group(&groups, std::mem::take(&mut group), stop)?;
				bytes = 0;
			}
		}
	}
}
fn send_group(
	groups: &SyncSender<Vec<Written>>,
	mut group: Vec<Written>,
	stop: &AtomicBool,
) -> Result<()> {
	loop {
		if stop.load(Ordering::Acquire) {
			return Ok(());
		}
		match groups.try_send(group) {
			Ok(()) => return Ok(()),
			Err(TrySendError::Full(value)) => {
				group = value;
				thread::sleep(Duration::from_millis(1));
			}
			Err(TrySendError::Disconnected(_)) => {
				return Err(Error(tr!("Kalıcılaştırma kanalı kapandı")));
			}
		}
	}
}
fn commit_stage(
	target: &dyn Target,
	groups: Receiver<Vec<Written>>,
	events: mpsc::Sender<Event>,
	stop: &AtomicBool,
	journal: &Path,
	mode: Verification,
) -> Result<()> {
	let mut records = OpenOptions::new()
		.create(true)
		.append(true)
		.mode(0o600)
		.open(journal)?;
	let mut retries = OpenOptions::new()
		.create(true)
		.append(true)
		.mode(0o600)
		.open(journal.with_extension("retries.jsonl"))?;
	sync_parent(journal)?;
	let mut retry_count = 0;
	loop {
		if stop.load(Ordering::Acquire) {
			return Ok(());
		}
		let group = match groups.recv_timeout(Duration::from_millis(50)) {
			Ok(group) => group,
			Err(RecvTimeoutError::Timeout) => continue,
			Err(RecvTimeoutError::Disconnected) => {
				send_event(&events, Event::Done)?;
				return Ok(());
			}
		};
		let started = Instant::now();
		target
			.sync()
			.map_err(|error| Error(tr!("Hedef fsync başarısız: {error}", error = error)))?;
		let mut flush_ms = milliseconds(started);
		let mut verify_ms = 0.0;
		let mut hash_ms = 0.0;
		let mut write_ms = 0.0;
		let mut journal_ms = 0.0;
		let mut bytes = 0;
		let mut verified = Vec::with_capacity(group.len());
		for chunk in &group {
			if stop.load(Ordering::Acquire) {
				return Ok(());
			}
			let started = Instant::now();
			let actual = sha256::digest(&target.read(chunk.offset, chunk.length)?);
			verify_ms += milliseconds(started);
			let mut attempt = 0;
			if actual != chunk.hash {
				retry_count += 1;
				let record = json::object([
					("epoch", json::number(crate::process::epoch())),
					("offset", json::number(chunk.offset)),
					("length", json::number(chunk.length as u64)),
					("expected_sha256", json::text(&chunk.hash)),
					("actual_sha256", json::text(&actual)),
					("retry", json::number(retry_count)),
				]);
				writeln!(retries, "{}", record.encode())?;
				retries.sync_all()?;
				ensure(
					retry_count <= MAX_RETRIES,
					tr!("Doğrulama tekrar sınırı aşıldı; hedef korunuyor"),
				)?;
				let (reply, receiver) = mpsc::sync_channel(1);
				send_event(
					&events,
					Event::Retry {
						count: retry_count,
						chunk: Chunk {
							offset: chunk.offset,
							length: chunk.length,
							retry_reply: Some(reply),
						},
					},
				)?;
				let source = loop {
					if stop.load(Ordering::Acquire) {
						return Ok(());
					}
					match receiver.recv_timeout(Duration::from_millis(50)) {
						Ok(bytes) => break bytes?,
						Err(RecvTimeoutError::Timeout) => continue,
						Err(RecvTimeoutError::Disconnected) => {
							return Err(Error(tr!("Tekrar okuma kanalı kapandı")));
						}
					}
				};
				let started = Instant::now();
				let reread_hash = sha256::digest(&source);
				hash_ms += milliseconds(started);
				if reread_hash != chunk.hash {
					writeln!(
						retries,
						"{}",
						json::object([
							("epoch", json::number(crate::process::epoch())),
							("offset", json::number(chunk.offset)),
							("status", json::text("source_changed")),
							("expected_sha256", json::text(&chunk.hash)),
							("actual_sha256", json::text(&reread_hash)),
						])
						.encode()
					)?;
					retries.sync_all()?;
					return Err(Error(tr!(
						"Tekrar okumasında kaynak verisi değişti; beklenen özet korunuyor"
					)));
				}
				let started = Instant::now();
				target.write(chunk.offset, &source)?;
				write_ms += milliseconds(started);
				drop(source);
				let started = Instant::now();
				target
					.sync()
					.map_err(|error| Error(tr!("Hedef fsync başarısız: {error}", error = error)))?;
				flush_ms += milliseconds(started);
				let started = Instant::now();
				let second_hash = sha256::digest(&target.read(chunk.offset, chunk.length)?);
				verify_ms += milliseconds(started);
				writeln!(
					retries,
					"{}",
					json::object([
						("epoch", json::number(crate::process::epoch())),
						("offset", json::number(chunk.offset)),
						(
							"status",
							json::text(if second_hash == chunk.hash {
								"recovered"
							} else {
								"failed"
							})
						),
						("expected_sha256", json::text(&chunk.hash)),
						("actual_sha256", json::text(&second_hash)),
					])
					.encode()
				)?;
				retries.sync_all()?;
				ensure(
					second_hash == chunk.hash,
					tr!("İkinci disk doğrulaması başarısız; hedef korunuyor"),
				)?;
				attempt = 1;
			}
			bytes += chunk.length as u64;
			verified.push(json::object([
				("offset", json::number(chunk.offset)),
				("length", json::number(chunk.length as u64)),
				("sha256", json::text(&chunk.hash)),
				("attempt", json::number(attempt)),
				("verification", json::text(mode.name())),
			]));
		}
		let started = Instant::now();
		for record in &verified {
			writeln!(records, "{}", record.encode())?;
		}
		records.sync_all()?;
		journal_ms += milliseconds(started);
		send_event(
			&events,
			Event::Committed {
				bytes,
				chunks: group.len() as u64,
				hash_ms,
				write_ms,
				flush_ms,
				verify_ms,
				journal_ms,
			},
		)?;
	}
}
fn worker<F>(events: mpsc::Sender<Event>, action: F)
where
	F: FnOnce() -> Result<()>,
{
	let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(action));
	let message = match result {
		Ok(Ok(())) => return,
		Ok(Err(error)) => error.to_string(),
		Err(_) => tr!("Aktarım G/Ç işçisi beklenmedik şekilde durdu"),
	};
	let _ = events.send(Event::Fatal(message));
}
/// Kaynak doğrulanmış ve donmuş olmalıdır. Hata dönüşünden önce bütün yerel
/// işçiler toplanır; hiçbir arka plan yazması sahipsiz bırakılmaz.
#[allow(clippy::too_many_arguments)]
pub fn copy(
	socket: &Path,
	target: &Path,
	ranges: &[Range],
	size: u64,
	journal: &Path,
	options: &TransferOptions,
	observe: impl FnMut(&TransferReport) -> Result<()>,
) -> Result<TransferReport> {
	options.validate()?;
	disk::validate_ranges(ranges, size)?;
	probe_target(target, size, options.verification)?;
	let probe = Probe::new(target);
	let storage: Arc<dyn Target> = Arc::new(LocalTarget::open(target, options.verification)?);
	copy_inner(
		socket, storage, probe, ranges, size, journal, options, observe,
	)
}

#[allow(clippy::too_many_arguments)]
fn copy_inner(
	socket: &Path,
	target: Arc<dyn Target>,
	mut probe: Probe,
	ranges: &[Range],
	size: u64,
	journal: &Path,
	options: &TransferOptions,
	mut observe: impl FnMut(&TransferReport) -> Result<()>,
) -> Result<TransferReport> {
	options.validate()?;
	disk::validate_ranges(ranges, size)?;
	let planned = ranges
		.iter()
		.try_fold(0u64, |sum, r| sum.checked_add(r.length))
		.ok_or_else(|| Error(tr!("Aktarım byte sayısı taştı")))?;
	let chunks = ranges
		.iter()
		.map(|r| r.length.div_ceil(CHUNK_BYTES as u64))
		.sum::<u64>();
	let mut report = TransferReport {
		planned_bytes: planned,
		..TransferReport::default()
	};
	let mut metrics = OpenOptions::new()
		.create(true)
		.append(true)
		.mode(0o600)
		.open(journal.with_extension("metrics.jsonl"))?;
	sync_parent(journal)?;
	if planned == 0 {
		writeln!(metrics, "{}", report.json(options.verification).encode())?;
		metrics.sync_all()?;
		observe(&report)?;
		return Ok(report);
	}
	let mut reader = Reader::connect(socket, size, options.read_timeout)?;
	let stop = AtomicBool::new(false);
	let (input, inputs) = mpsc::sync_channel(8);
	let (groups, batches) = mpsc::sync_channel(1);
	let (events, receiver) = mpsc::channel();
	thread::scope(|scope| {
		let target_write = target.clone();
		let target_commit = target.clone();
		let write_events = events.clone();
		let commit_events = events.clone();
		let stop_ref = &stop;
		let language = crate::i18n::current_language();
		let writer = scope.spawn(move || {
			let _language = crate::i18n::enter_language(language);
			worker(write_events.clone(), || {
				write_stage(
					target_write.as_ref(),
					inputs,
					groups,
					write_events,
					stop_ref,
				)
			})
		});
		let committer = scope.spawn(move || {
			let _language = crate::i18n::enter_language(language);
			worker(commit_events.clone(), || {
				commit_stage(
					target_commit.as_ref(),
					batches,
					commit_events,
					stop_ref,
					journal,
					options.verification,
				)
			})
		});
		let result = (|| {
			let started = Instant::now();
			let mut sample_time = Instant::now();
			let mut sample_received = 0u64;
			let mut received = 0u64;
			let mut reserved = 0usize;
			let budget = options.memory_bytes - 3 * CHUNK_BYTES;
			let mut plan = Plan::new(ranges);
			let mut next = plan.next();
			let mut ready = VecDeque::<Reply>::new();
			let mut retry_queue = VecDeque::new();
			let mut input = Some(input);
			let mut control = Controller::new(options.max_depth);
			let mut writes = Samples::default();
			let mut flushes = Samples::default();
			let mut source_saturated = false;
			let mut next_issue = Instant::now();
			loop {
				let mut busy = false;
				loop {
					match receiver.try_recv() {
						Ok(Event::Written {
							bytes,
							hash_ms,
							write_ms,
						}) => {
							reserved = reserved
								.checked_sub(bytes)
								.ok_or_else(|| Error(tr!("Aktarım tampon sayacı tutarsız")))?;
							report.hash_ms += hash_ms;
							report.write_ms += write_ms;
							writes.push(write_ms);
							busy = true;
						}
						Ok(Event::Committed {
							bytes,
							chunks,
							hash_ms,
							write_ms,
							flush_ms,
							verify_ms,
							journal_ms,
						}) => {
							report.verified_bytes += bytes;
							report.verified_chunks += chunks;
							report.hash_ms += hash_ms;
							report.write_ms += write_ms;
							report.fsync_ms += flush_ms;
							report.verify_ms += verify_ms;
							report.journal_ms += journal_ms;
							flushes.push(flush_ms);
							writes.push(verify_ms / chunks.max(1) as f64);
							ensure(
								report.verified_bytes <= planned,
								tr!("Doğrulanan byte sayısı planı aştı"),
							)?;
							busy = true;
						}
						Ok(Event::Retry { chunk, count }) => {
							report.retries = count;
							retry_queue.push_back(chunk);
							busy = true;
						}
						Ok(Event::Fatal(message)) => return Err(Error(message)),
						Ok(Event::Done) => {
							ensure(
								next.is_none()
									&& ready.is_empty() && reader.len() == 0
									&& retry_queue.is_empty() && reserved == 0
									&& report.verified_bytes == planned
									&& report.verified_chunks == chunks,
								tr!("Aktarım kapsamı tamamlanmadı; hedef korunuyor"),
							)?;
							report.elapsed_seconds = started.elapsed().as_secs_f64();
							report.peak_in_flight = reader.peak_pending;
							report.queue_depth = control.depth;
							writeln!(metrics, "{}", report.json(options.verification).encode())?;
							metrics.sync_all()?;
							observe(&report)?;
							return Ok(report);
						}
						Err(TryRecvError::Empty) => break,
						Err(TryRecvError::Disconnected) => {
							return Err(Error(tr!("Aktarım olay kanalı kapandı")));
						}
					}
				}
				while reader.len() < control.depth {
					if let Some(chunk) = retry_queue.pop_front() {
						reader.submit(chunk)?;
						report.read_requests += 1;
						busy = true;
						continue;
					}
					if Instant::now() < next_issue {
						break;
					}
					let Some(chunk) = next.as_ref() else { break };
					if reserved + chunk.length > budget {
						break;
					}
					let chunk = next.take().unwrap();
					reserved += chunk.length;
					report.peak_buffer_bytes =
						report.peak_buffer_bytes.max(reserved + 3 * CHUNK_BYTES);
					reader.submit(chunk)?;
					report.read_requests += 1;
					next = plan.next();
					next_issue = Instant::now() + control.delay;
					busy = true;
				}
				source_saturated |= reader.len() >= control.depth;
				if let Some(packet) = reader.poll()? {
					report.read_ms += packet.read_ms;
					received += packet.bytes.len() as u64;
					if let Some(reply) = &packet.chunk.retry_reply {
						reply
							.send(Ok(packet.bytes))
							.map_err(|_| Error(tr!("Tekrar okuma kanalı kapandı")))?;
					} else {
						ready.push_back(packet);
					}
					busy = true;
				}
				while let Some(packet) = ready.pop_front() {
					let sender = input
						.as_ref()
						.ok_or_else(|| Error(tr!("Yazma kanalı erken kapandı")))?;
					match sender.try_send(Input::Data(packet)) {
						Ok(()) => busy = true,
						Err(TrySendError::Full(Input::Data(packet))) => {
							ready.push_front(packet);
							break;
						}
						Err(_) => return Err(Error(tr!("Yazma kanalı erken kapandı"))),
					}
				}
				if next.is_none()
					&& reader.len() == 0
					&& ready.is_empty()
					&& let Some(sender) = input.take()
				{
					match sender.try_send(Input::End) {
						Ok(()) => {}
						Err(TrySendError::Full(_)) => input = Some(sender),
						Err(TrySendError::Disconnected(_)) => {
							return Err(Error(tr!("Yazma kanalı erken kapandı")));
						}
					}
				}
				if sample_time.elapsed() >= Duration::from_secs(1) {
					let seconds = sample_time.elapsed().as_secs_f64();
					let pressure = probe.sample(seconds);
					let rate = (received - sample_received) as f64 / 1048576.0 / seconds;
					control.update(
						rate,
						writes.p95(),
						flushes.p95(),
						pressure,
						reserved > budget * 3 / 4 || ready.len() >= 4,
						source_saturated,
					);
					report.queue_depth = control.depth;
					report.elapsed_seconds = started.elapsed().as_secs_f64();
					report.peak_in_flight = reader.peak_pending;
					let mut row = report.json(options.verification);
					if let json::Value::Object(fields) = &mut row {
						fields.insert("epoch".into(), json::number(crate::process::epoch()));
						fields.insert("source_bytes".into(), json::number(received));
						fields.insert(
							"issue_delay_us".into(),
							json::number(control.delay.as_micros() as u64),
						);
						fields.insert(
							"target_queue".into(),
							pressure
								.queue
								.map(json::number)
								.unwrap_or(json::Value::Null),
						);
						fields.insert(
							"io_stall_ppm".into(),
							pressure
								.stall
								.map(|s| json::number((s * 1_000_000.0) as u64))
								.unwrap_or(json::Value::Null),
						);
					}
					writeln!(metrics, "{}", row.encode())?;
					metrics.sync_data()?;
					observe(&report)?;
					sample_time = Instant::now();
					sample_received = received;
					writes.clear();
					flushes.clear();
					source_saturated = false;
				}
				if !busy {
					thread::sleep(Duration::from_millis(1));
				}
			}
		})();
		stop.store(true, Ordering::Release);
		// Bloke edebilen yerel işler gerçekten bitmeden dönülmez.
		let writer_result = writer.join();
		let committer_result = committer.join();
		if writer_result.is_err() || committer_result.is_err() {
			return Err(Error(tr!("Aktarım G/Ç işçisi beklenmedik şekilde durdu")));
		}
		if let Err(error) = &result {
			let _ = writeln!(
				metrics,
				"{}",
				json::object([
					("epoch", json::number(crate::process::epoch())),
					("status", json::text("failed")),
					("error", json::text(error.to_string())),
				])
				.encode()
			);
			let _ = metrics.sync_all();
		}
		result
	})
}

#[cfg(test)]
mod tests;
