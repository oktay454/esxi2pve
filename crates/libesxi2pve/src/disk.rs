//! Salt okunur NBD, mantıksal fark uygulama ve QMP doğrulaması.
use crate::{
	Error, Result, ensure,
	json::{self, Value},
	process::{read_text, write_private},
	sha256::{self, Sha256},
};
use std::{
	fs::{File, OpenOptions},
	io::{BufRead, BufReader, Read, Seek, SeekFrom, Write},
	os::unix::{fs::FileExt, net::UnixStream},
	path::Path,
	time::{Duration, Instant},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Range {
	pub start: u64,
	pub length: u64,
}
pub fn map_ranges(text: &str, size: u64) -> Result<Vec<Range>> {
	let data = json::parse(text)?;
	let entries = data.array()?;
	ensure(!entries.is_empty(), tr!("QEMU haritası boş"))?;
	let mut end = 0u64;
	let mut ranges: Vec<Range> = Vec::new();
	for entry in entries {
		let start = entry.get("start")?.integer()?;
		let length = entry.get("length")?.integer()?;
		ensure(
			start == end && length > 0,
			tr!("QEMU haritası sıralı ve tam kapsamlı değil"),
		)?;
		end = start
			.checked_add(length)
			.ok_or_else(|| Error(tr!("Harita taşması")))?;
		let depth = entry.get("depth")?.integer()?;
		ensure(
			depth != 0
				|| entry.optional("data").is_some_and(Value::is_true)
				|| entry.optional("zero").is_some_and(Value::is_true),
			tr!("Üst snapshot bölgesi veri/sıfır olarak çözümlenemedi; atlanmadı"),
		)?;
		if depth == 0
			&& (entry.optional("data").is_some_and(Value::is_true)
				|| entry.optional("zero").is_some_and(Value::is_true))
		{
			if let Some(last) = ranges.last_mut().filter(|r| r.start + r.length == start) {
				last.length += length;
			} else {
				ranges.push(Range { start, length });
			}
		}
	}
	ensure(end == size, tr!("Harita/disk kapasitesi uyuşmuyor"))?;
	Ok(ranges)
}
pub fn encode_ranges(ranges: &[Range]) -> String {
	Value::Array(
		ranges
			.iter()
			.map(|r| Value::Array(vec![json::number(r.start), json::number(r.length)]))
			.collect(),
	)
	.encode()
}
pub fn validate_ranges(ranges: &[Range], size: u64) -> Result<()> {
	let mut end = 0;
	for r in ranges {
		ensure(
			r.start >= end && r.length > 0,
			tr!("Örtüşen veya boş disk bölgesi"),
		)?;
		end = r
			.start
			.checked_add(r.length)
			.ok_or_else(|| Error(tr!("Bölge taşması")))?;
		ensure(end <= size, tr!("Bölge disk sınırını aşıyor"))?;
	}
	Ok(())
}
/// Sabit yeni biçim (fixed-newstyle), basit READ yanıtları. Yazma komutu içermez.
pub struct NbdReader {
	stream: UnixStream,
	pub size: u64,
	handle: u64,
}
impl NbdReader {
	pub fn connect(path: &Path) -> Result<Self> {
		let mut stream = UnixStream::connect(path)?;
		stream.set_read_timeout(Some(Duration::from_secs(120)))?;
		stream.set_write_timeout(Some(Duration::from_secs(120)))?;
		let mut greeting = [0; 18];
		stream.read_exact(&mut greeting)?;
		ensure(
			&greeting[..8] == b"NBDMAGIC"
				&& u64::from_be_bytes(greeting[8..16].try_into().unwrap()) == 0x49484156454f5054,
			tr!("NBD karşılama verisi geçersiz"),
		)?;
		let flags = u16::from_be_bytes(greeting[16..].try_into().unwrap());
		ensure(flags & 1 != 0, tr!("NBD fixed-newstyle gerekli"))?;
		let client = 1u32 | if flags & 2 != 0 { 2 } else { 0 };
		stream.write_all(&client.to_be_bytes())?;
		stream.write_all(&0x49484156454f5054u64.to_be_bytes())?;
		stream.write_all(&1u32.to_be_bytes())?;
		stream.write_all(&0u32.to_be_bytes())?;
		let mut export = [0; 10];
		stream.read_exact(&mut export)?;
		let size = u64::from_be_bytes(export[..8].try_into().unwrap());
		let transmission = u16::from_be_bytes(export[8..].try_into().unwrap());
		if client & 2 == 0 {
			let mut padding = [0; 124];
			stream.read_exact(&mut padding)?;
		}
		ensure(
			transmission & 2 != 0 && size > 0,
			tr!("NBD dışa aktarımı salt okunur ve dolu olmalı"),
		)?;
		Ok(Self {
			stream,
			size,
			handle: 0,
		})
	}
	pub fn read(&mut self, offset: u64, length: usize) -> Result<Vec<u8>> {
		ensure(
			length > 0
				&& length <= 4 * 1024 * 1024
				&& offset
					.checked_add(length as u64)
					.is_some_and(|n| n <= self.size),
			tr!("NBD okuma sınırı"),
		)?;
		self.handle += 1;
		let mut request = Vec::with_capacity(28);
		request.extend(0x25609513u32.to_be_bytes());
		request.extend(0u16.to_be_bytes());
		request.extend(0u16.to_be_bytes());
		request.extend(self.handle.to_be_bytes());
		request.extend(offset.to_be_bytes());
		request.extend((length as u32).to_be_bytes());
		self.stream.write_all(&request)?;
		let mut reply = [0; 16];
		self.stream.read_exact(&mut reply)?;
		ensure(
			u32::from_be_bytes(reply[..4].try_into().unwrap()) == 0x67446698
				&& u32::from_be_bytes(reply[4..8].try_into().unwrap()) == 0
				&& u64::from_be_bytes(reply[8..].try_into().unwrap()) == self.handle,
			tr!("NBD READ yanıtı başarısız"),
		)?;
		let mut data = vec![0; length];
		self.stream.read_exact(&mut data)?;
		Ok(data)
	}
}
pub fn target_size(path: &Path, expected: u64) -> Result<()> {
	use std::os::unix::fs::FileTypeExt;
	let m = std::fs::metadata(path)?;
	ensure(
		m.is_file() || m.file_type().is_block_device(),
		tr!("Hedef yerel dosya/blok aygıtı olmalı (Ceph için krbd)"),
	)?;
	let mut f = File::open(path)?;
	let size = f.seek(SeekFrom::End(0))?;
	ensure(size >= expected, tr!("Hedef disk kaynaktan küçük"))
}
pub fn logical_copy(
	socket: &Path,
	target: &Path,
	ranges: &[Range],
	size: u64,
	journal: &Path,
) -> Result<u64> {
	validate_ranges(ranges, size)?;
	target_size(target, size)?;
	let mut reader = NbdReader::connect(socket)?;
	ensure(reader.size == size, tr!("NBD kapasitesi değişti"))?;
	let file = OpenOptions::new().read(true).write(true).open(target)?;
	use std::os::unix::fs::OpenOptionsExt;
	let mut journal = OpenOptions::new()
		.create(true)
		.append(true)
		.mode(0o600)
		.open(journal)?;
	let mut pending: Vec<(u64, usize, String)> = Vec::new();
	let mut batch_bytes = 0;
	let mut total = 0;
	fn verify(
		file: &File,
		journal: &mut File,
		batch: &mut Vec<(u64, usize, String)>,
	) -> Result<()> {
		file.sync_all()?;
		for (offset, length, hash) in batch.drain(..) {
			let mut data = vec![0; length];
			file.read_exact_at(&mut data, offset)?;
			ensure(
				sha256::digest(&data) == hash,
				tr!("fsync sonrası disk geri okuma doğrulaması başarısız"),
			)?;
			writeln!(
				journal,
				"{}",
				json::object([
					("offset", json::number(offset)),
					("length", json::number(length as u64)),
					("sha256", json::text(hash))
				])
				.encode()
			)?;
		}
		journal.sync_all()?;
		Ok(())
	}
	for r in ranges {
		let end = r.start + r.length;
		let mut offset = r.start;
		while offset < end {
			let data = reader.read(offset, (end - offset).min(4 * 1024 * 1024) as usize)?;
			file.write_all_at(&data, offset)?;
			pending.push((offset, data.len(), sha256::digest(&data)));
			offset += data.len() as u64;
			total += data.len() as u64;
			batch_bytes += data.len();
			if batch_bytes >= 64 * 1024 * 1024 {
				verify(&file, &mut journal, &mut pending)?;
				batch_bytes = 0;
			}
		}
	}
	verify(&file, &mut journal, &mut pending)?;
	Ok(total)
}
pub fn checksums(path: &Path, size: u64, block: u64) -> Result<Vec<String>> {
	checksums_with_progress(path, size, block, |_, _| Ok(()))
}
pub fn checksums_with_progress(
	path: &Path,
	size: u64,
	block: u64,
	mut progress: impl FnMut(u64, &str) -> Result<()>,
) -> Result<Vec<String>> {
	ensure(
		block > 0 && block <= 1024 * 1024 * 1024,
		tr!("Sağlama toplamı blok sınırı"),
	)?;
	let mut f = File::open(path)?;
	let mut offset = 0;
	let mut rows = Vec::new();
	let mut buffer = vec![0; 4 * 1024 * 1024];
	while offset < size {
		let mut remaining = block.min(size - offset);
		let mut sha = Sha256::new();
		while remaining > 0 {
			let n = remaining.min(buffer.len() as u64) as usize;
			f.read_exact(&mut buffer[..n])?;
			sha.update(&buffer[..n]);
			remaining -= n as u64;
		}
		let hash = sha.hex();
		progress(rows.len() as u64, &hash)?;
		rows.push(hash);
		offset += block.min(size - offset);
	}
	Ok(rows)
}
pub fn hash_rows(text: &str, count: u64) -> Result<Vec<String>> {
	let mut rows = Vec::new();
	for (i, line) in text.lines().enumerate() {
		let f: Vec<_> = line.split_whitespace().collect();
		ensure(
			f.len() == 2
				&& f[0].parse::<usize>().ok() == Some(i)
				&& f[1].len() == 64
				&& f[1]
					.bytes()
					.all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
			tr!("SHA-256 kayıtları eksik/sırasız/geçersiz"),
		)?;
		rows.push(f[1].to_string());
	}
	ensure(
		rows.len() as u64 == count,
		tr!("SHA-256 blok sayısı uyuşmuyor"),
	)?;
	Ok(rows)
}
pub fn hash_diffs(
	source: &[String],
	target: &[String],
	size: u64,
	block: u64,
) -> Result<Vec<Range>> {
	ensure(
		block > 0 && source.len() == target.len() && source.len() as u64 == size.div_ceil(block),
		tr!("SHA-256 blok sayısı uyuşmuyor"),
	)?;
	let mut out: Vec<Range> = Vec::new();
	for (i, (a, b)) in source.iter().zip(target).enumerate() {
		if a != b {
			let start = i as u64 * block;
			let length = block.min(size - start);
			if let Some(last) = out.last_mut().filter(|r| r.start + r.length == start) {
				last.length += length;
			} else {
				out.push(Range { start, length });
			}
		}
	}
	Ok(out)
}
pub fn save_hashes(path: &Path, hashes: &[String]) -> Result<()> {
	write_private(
		path,
		hashes
			.iter()
			.enumerate()
			.map(|(i, h)| format!("{i} {h}\n"))
			.collect::<String>(),
	)
}
#[derive(Debug)]
pub struct Benchmark {
	pub p95_ms: f64,
	pub sequential_mib: f64,
	pub mirror_mib: u64,
}
pub fn benchmark(
	socket: &Path,
	max_latency: u64,
	min_mib: u64,
	mirror_mib: u64,
) -> Result<Benchmark> {
	let mut r = NbdReader::connect(socket)?;
	ensure(r.size >= 4096, tr!("Disk I/O testi için küçük"))?;
	let mut seed = 473u64;
	let mut latencies = Vec::new();
	for _ in 0..64 {
		seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
		let offset = (seed % (r.size / 4096)) * 4096;
		let now = Instant::now();
		r.read(offset, 4096)?;
		latencies.push(now.elapsed().as_secs_f64() * 1000.0);
	}
	latencies.sort_by(f64::total_cmp);
	let p95 = latencies[60];
	let count = r.size.min(16 * 1024 * 1024);
	let now = Instant::now();
	let mut offset = 0;
	while offset < count {
		let n = (count - offset).min(4 * 1024 * 1024) as usize;
		r.read(offset, n)?;
		offset += n as u64;
	}
	let speed = count as f64 / 1048576.0 / now.elapsed().as_secs_f64().max(0.000001);
	ensure(
		p95 <= max_latency as f64 && speed >= min_mib as f64,
		tr!(
			"Uzak disk yavaş: p95={p95:.2}ms, ardışık={speed:.2}MiB/s; hedef başlatılmadı",
			p95 = p95,
			speed = speed
		),
	)?;
	Ok(Benchmark {
		p95_ms: p95,
		sequential_mib: speed,
		mirror_mib: mirror_mib.min((speed / 2.0) as u64).max(1),
	})
}

pub struct Qmp {
	stream: BufReader<UnixStream>,
	id: u64,
	pub events: Vec<Value>,
	pub transcript: Vec<Value>,
}
impl Qmp {
	pub fn connect(path: &Path) -> Result<Self> {
		let stream = UnixStream::connect(path)?;
		stream.set_read_timeout(Some(Duration::from_secs(30)))?;
		stream.set_write_timeout(Some(Duration::from_secs(30)))?;
		let mut q = Self {
			stream: BufReader::new(stream),
			id: 0,
			events: Vec::new(),
			transcript: Vec::new(),
		};
		ensure(
			q.receive()?.optional("QMP").is_some(),
			tr!("QMP karşılama verisi yok"),
		)?;
		q.call("qmp_capabilities")?;
		Ok(q)
	}
	fn receive(&mut self) -> Result<Value> {
		let mut line = String::new();
		self.stream
			.by_ref()
			.take(8 * 1024 * 1024)
			.read_line(&mut line)?;
		ensure(
			!line.is_empty() && line.ends_with('\n'),
			tr!("QMP yanıtı eksik/aşırı büyük"),
		)?;
		let v = json::parse(&line)?;
		if v.optional("event").is_some() {
			self.events.push(v.clone());
		}
		self.transcript.push(v.clone());
		Ok(v)
	}
	pub fn call(&mut self, command: &str) -> Result<Value> {
		self.id += 1;
		let req = json::object([
			("execute", json::text(command)),
			("id", json::number(self.id)),
		]);
		self.transcript.push(req.clone());
		self.stream
			.get_mut()
			.write_all(format!("{}\r\n", req.encode()).as_bytes())?;
		loop {
			let response = self.receive()?;
			if response.optional("id").and_then(|v| v.integer().ok()) != Some(self.id) {
				continue;
			}
			if let Some(e) = response.optional("error") {
				return Err(Error(format!("QMP {command}: {}", e.encode())));
			}
			return Ok(response.get("return")?.clone());
		}
	}
}
fn image_key(s: &str) -> String {
	if let Some(v) = s.strip_prefix("rbd:") {
		format!("rbd:{}", v.split(':').next().unwrap_or(""))
	} else {
		std::fs::canonicalize(s)
			.unwrap_or_else(|_| s.into())
			.to_string_lossy()
			.into_owned()
	}
}
pub fn active_device(blocks: &Value, path: &str) -> Result<String> {
	let found: Vec<_> = blocks
		.array()?
		.iter()
		.filter(|b| {
			b.optional("inserted")
				.and_then(|v| v.optional("image"))
				.and_then(|v| v.optional("filename"))
				.and_then(|v| v.string().ok())
				.is_some_and(|s| image_key(s) == image_key(path))
		})
		.collect();
	ensure(
		found.len() == 1,
		tr!("QMP etkin kaynak disk tekil bulunamadı"),
	)?;
	let id = found[0].get("device")?.string()?;
	ensure(!id.is_empty(), tr!("Boş QMP disk kimliği"))?;
	Ok(id.into())
}
pub fn verify_active(blocks: &Value, jobs: &Value, id: &str, target: &str) -> Result<()> {
	let found: Vec<_> = blocks
		.array()?
		.iter()
		.filter(|b| b.optional("device").and_then(|v| v.string().ok()) == Some(id))
		.collect();
	ensure(found.len() == 1, tr!("QMP disk kimliği kayboldu"))?;
	let image = found[0].get("inserted")?.get("image")?;
	ensure(
		image_key(image.get("filename")?.string()?) == image_key(target)
			&& image.get("format")?.string()? == "raw"
			&& image.optional("backing-image").is_none()
			&& image.optional("backing-filename").is_none(),
		tr!("Etkin disk bağımsız ham hedefe geçmemiş"),
	)?;
	ensure(jobs.array()?.is_empty(), tr!("QMP disk işi hâlâ sürüyor"))
}
pub fn load_hashes(path: &Path, size: u64, block: u64) -> Result<Vec<String>> {
	hash_rows(&read_text(path)?, size.div_ceil(block))
}
#[cfg(test)]
mod tests {
	use super::*;
	#[test]
	fn logical_not_physical_and_zero() {
		let s = r#"[{"start":0,"length":512,"depth":1,"data":true},{"start":512,"length":512,"depth":0,"data":true,"offset":8192},{"start":1024,"length":512,"depth":0,"data":false,"zero":true}]"#;
		assert_eq!(
			map_ranges(s, 1536).unwrap(),
			vec![Range {
				start: 512,
				length: 1024
			}]
		);
	}
	#[test]
	fn rejects_map_gaps_missing_depth() {
		assert!(
			map_ranges(
				r#"[{"start":512,"length":512,"depth":0,"data":true}]"#,
				1024
			)
			.is_err()
		);
		assert!(map_ranges(r#"[{"start":0,"length":512,"data":true}]"#, 512).is_err());
	}
	#[test]
	fn ranges_overflow() {
		assert!(
			validate_ranges(
				&[Range {
					start: u64::MAX,
					length: 2
				}],
				u64::MAX
			)
			.is_err()
		);
		assert!(
			validate_ranges(
				&[
					Range {
						start: 0,
						length: 10
					},
					Range {
						start: 9,
						length: 1
					}
				],
				10
			)
			.is_err()
		);
	}
	#[test]
	fn hash_count_order() {
		assert!(hash_rows(&format!("1 {}\n", "0".repeat(64)), 1).is_err());
		assert!(hash_rows("", 1).is_err());
	}
	#[test]
	fn qmp_reject_backing_jobs_wrong_path() {
		let blocks=json::parse(r#"[{"device":"drive-scsi0","inserted":{"image":{"filename":"/disk/local.raw","format":"raw"}}}]"#).unwrap();
		let jobs = Value::Array(vec![]);
		assert!(verify_active(&blocks, &jobs, "drive-scsi0", "/disk/local.raw").is_ok());
		assert!(verify_active(&blocks, &jobs, "drive-scsi0", "/disk/wrong.raw").is_err());
		assert!(
			verify_active(
				&blocks,
				&Value::Array(vec![Value::Null]),
				"drive-scsi0",
				"/disk/local.raw"
			)
			.is_err()
		);
	}
}

#[cfg(test)]
mod ambiguous_map_tests {
	#[test]
	fn undecoded_upper_layer_fails_closed() {
		assert!(
			super::map_ranges(
				r#"[{"start":0,"length":512,"depth":0,"data":false,"zero":false}]"#,
				512
			)
			.is_err()
		);
	}
}
