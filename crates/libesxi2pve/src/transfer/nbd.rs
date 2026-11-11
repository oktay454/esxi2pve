//! Tek NBD bağlantısında bloke etmeyen istek/yanıt çoğullama.
use super::{CHUNK_BYTES, Chunk};
use crate::{Result, disk::NbdReader, ensure};
use std::{
	collections::{BTreeMap, VecDeque},
	io::{ErrorKind, Read, Write},
	net::Shutdown,
	os::unix::net::UnixStream,
	path::Path,
	time::{Duration, Instant},
};
struct Request {
	header: [u8; 28],
	position: usize,
}
struct Pending {
	chunk: Chunk,
	started: Instant,
}
struct Body {
	pending: Pending,
	bytes: Vec<u8>,
	position: usize,
}
pub(super) struct Reply {
	pub chunk: Chunk,
	pub bytes: Vec<u8>,
	pub read_ms: f64,
}
pub(super) struct Reader {
	stream: UnixStream,
	pending: BTreeMap<u64, Pending>,
	outgoing: VecDeque<Request>,
	header: [u8; 16],
	header_position: usize,
	body: Option<Body>,
	next: u64,
	timeout: Duration,
	pub peak_pending: usize,
}
impl Reader {
	pub fn connect(path: &Path, size: u64, timeout: Duration) -> Result<Self> {
		let reader = NbdReader::connect(path)?;
		ensure(reader.size == size, tr!("NBD kapasitesi değişti"))?;
		reader.stream.set_nonblocking(true)?;
		Ok(Self {
			stream: reader.stream,
			pending: BTreeMap::new(),
			outgoing: VecDeque::new(),
			header: [0; 16],
			header_position: 0,
			body: None,
			next: 0,
			timeout,
			peak_pending: 0,
		})
	}
	pub fn len(&self) -> usize {
		self.pending.len() + usize::from(self.body.is_some())
	}
	pub fn submit(&mut self, chunk: Chunk) -> Result<()> {
		ensure(
			chunk.length > 0 && chunk.length <= CHUNK_BYTES,
			tr!("NBD okuma sınırı"),
		)?;
		self.next = self
			.next
			.checked_add(1)
			.ok_or_else(|| crate::Error(tr!("NBD istek kimliği taştı")))?;
		let mut header = [0; 28];
		header[0..4].copy_from_slice(&0x25609513u32.to_be_bytes());
		header[8..16].copy_from_slice(&self.next.to_be_bytes());
		header[16..24].copy_from_slice(&chunk.offset.to_be_bytes());
		header[24..28].copy_from_slice(&(chunk.length as u32).to_be_bytes());
		self.pending.insert(
			self.next,
			Pending {
				chunk,
				started: Instant::now(),
			},
		);
		self.outgoing.push_back(Request {
			header,
			position: 0,
		});
		self.peak_pending = self.peak_pending.max(self.len());
		Ok(())
	}
	pub fn poll(&mut self) -> Result<Option<Reply>> {
		ensure(
			!self
				.pending
				.values()
				.any(|p| p.started.elapsed() > self.timeout)
				&& self
					.body
					.as_ref()
					.is_none_or(|b| b.pending.started.elapsed() <= self.timeout),
			tr!("NBD okuma zaman aşımı; bekleyen kaynak istekleri kapatıldı"),
		)?;
		while let Some(request) = self.outgoing.front_mut() {
			match self.stream.write(&request.header[request.position..]) {
				Ok(0) => {
					return Err(crate::Error(tr!(
						"NBD bağlantısı beklenmedik şekilde kapandı"
					)));
				}
				Ok(n) => {
					request.position += n;
					if request.position == request.header.len() {
						self.outgoing.pop_front();
					}
				}
				Err(e) if e.kind() == ErrorKind::WouldBlock => break,
				Err(e) if e.kind() == ErrorKind::Interrupted => continue,
				Err(e) => return Err(e.into()),
			}
		}
		if self.body.is_none() && !self.pending.is_empty() {
			while self.header_position < self.header.len() {
				match self.stream.read(&mut self.header[self.header_position..]) {
					Ok(0) => {
						return Err(crate::Error(tr!(
							"NBD bağlantısı beklenmedik şekilde kapandı"
						)));
					}
					Ok(n) => self.header_position += n,
					Err(e) if e.kind() == ErrorKind::WouldBlock => return Ok(None),
					Err(e) if e.kind() == ErrorKind::Interrupted => continue,
					Err(e) => return Err(e.into()),
				}
			}
			ensure(
				u32::from_be_bytes(self.header[..4].try_into().unwrap()) == 0x67446698
					&& u32::from_be_bytes(self.header[4..8].try_into().unwrap()) == 0,
				tr!("NBD READ yanıtı başarısız"),
			)?;
			let handle = u64::from_be_bytes(self.header[8..].try_into().unwrap());
			let pending = self.pending.remove(&handle).ok_or_else(|| {
				crate::Error(tr!("NBD yanıtında bilinmeyen veya yinelenen istek kimliği"))
			})?;
			let length = pending.chunk.length;
			self.body = Some(Body {
				pending,
				bytes: vec![0; length],
				position: 0,
			});
			self.header_position = 0;
		}
		if let Some(body) = self.body.as_mut() {
			while body.position < body.bytes.len() {
				match self.stream.read(&mut body.bytes[body.position..]) {
					Ok(0) => {
						return Err(crate::Error(tr!(
							"NBD bağlantısı beklenmedik şekilde kapandı"
						)));
					}
					Ok(n) => body.position += n,
					Err(e) if e.kind() == ErrorKind::WouldBlock => return Ok(None),
					Err(e) if e.kind() == ErrorKind::Interrupted => continue,
					Err(e) => return Err(e.into()),
				}
			}
			let body = self.body.take().unwrap();
			return Ok(Some(Reply {
				chunk: body.pending.chunk,
				bytes: body.bytes,
				read_ms: body.pending.started.elapsed().as_secs_f64() * 1000.0,
			}));
		}
		Ok(None)
	}
}
impl Drop for Reader {
	fn drop(&mut self) {
		let _ = self.stream.shutdown(Shutdown::Both);
	}
}
