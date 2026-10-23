//! QEMU JSON/QMP ile sınırlı, katı ayrıştırıcı; yinelenen anahtarları reddeder.
use crate::{Error, Result, ensure};
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq)]
pub enum Value {
	Null,
	Bool(bool),
	Number(String),
	String(String),
	Array(Vec<Value>),
	Object(BTreeMap<String, Value>),
}
impl Value {
	pub fn get(&self, key: &str) -> Result<&Self> {
		if let Self::Object(map) = self {
			map.get(key)
				.ok_or_else(|| Error(tr!("JSON alanı eksik: {key}", key = key)))
		} else {
			Err(Error(tr!("JSON nesnesi bekleniyor")))
		}
	}
	pub fn optional(&self, key: &str) -> Option<&Self> {
		if let Self::Object(map) = self {
			map.get(key)
		} else {
			None
		}
	}
	pub fn string(&self) -> Result<&str> {
		if let Self::String(s) = self {
			Ok(s)
		} else {
			Err(Error(tr!("JSON metni bekleniyor")))
		}
	}
	pub fn integer(&self) -> Result<u64> {
		if let Self::Number(n) = self {
			n.parse()
				.map_err(|_| Error(tr!("JSON pozitif tamsayı bekleniyor")))
		} else {
			Err(Error(tr!("JSON sayısı bekleniyor")))
		}
	}
	pub fn array(&self) -> Result<&[Value]> {
		if let Self::Array(v) = self {
			Ok(v)
		} else {
			Err(Error(tr!("JSON dizisi bekleniyor")))
		}
	}
	pub fn is_true(&self) -> bool {
		matches!(self, Self::Bool(true))
	}
	pub fn encode(&self) -> String {
		match self {
			Self::Null => "null".into(),
			Self::Bool(v) => v.to_string(),
			Self::Number(v) => v.clone(),
			Self::String(v) => quote(v),
			Self::Array(v) => format!(
				"[{}]",
				v.iter().map(Self::encode).collect::<Vec<_>>().join(",")
			),
			Self::Object(v) => format!(
				"{{{}}}",
				v.iter()
					.map(|(k, v)| format!("{}:{}", quote(k), v.encode()))
					.collect::<Vec<_>>()
					.join(",")
			),
		}
	}
}
pub fn text(value: impl Into<String>) -> Value {
	Value::String(value.into())
}
pub fn number(value: u64) -> Value {
	Value::Number(value.to_string())
}
pub fn object(items: impl IntoIterator<Item = (impl Into<String>, Value)>) -> Value {
	Value::Object(items.into_iter().map(|(k, v)| (k.into(), v)).collect())
}
pub fn quote(s: &str) -> String {
	let mut out = String::from("\"");
	for c in s.chars() {
		match c {
			'"' => out.push_str("\\\""),
			'\\' => out.push_str("\\\\"),
			'\n' => out.push_str("\\n"),
			'\r' => out.push_str("\\r"),
			'\t' => out.push_str("\\t"),
			c if c < ' ' => out.push_str(&format!("\\u{:04x}", c as u32)),
			c => out.push(c),
		}
	}
	out.push('"');
	out
}
pub fn parse(s: &str) -> Result<Value> {
	let mut p = Parser {
		bytes: s.as_bytes(),
		pos: 0,
	};
	let value = p.value(0)?;
	p.space();
	ensure(p.pos == p.bytes.len(), tr!("JSON sonunda fazla veri"))?;
	Ok(value)
}
struct Parser<'a> {
	bytes: &'a [u8],
	pos: usize,
}
impl Parser<'_> {
	fn space(&mut self) {
		while self.pos < self.bytes.len()
			&& matches!(self.bytes[self.pos], b' ' | b'\n' | b'\r' | b'\t')
		{
			self.pos += 1;
		}
	}
	fn take(&mut self, byte: u8) -> bool {
		self.space();
		if self.bytes.get(self.pos) == Some(&byte) {
			self.pos += 1;
			true
		} else {
			false
		}
	}
	fn need(&mut self, byte: u8) -> Result<()> {
		ensure(
			self.take(byte),
			tr!("JSON karakteri bekleniyor: {}", byte as char),
		)
	}
	fn string(&mut self) -> Result<String> {
		self.need(b'"')?;
		let mut out = Vec::new();
		loop {
			let c = *self
				.bytes
				.get(self.pos)
				.ok_or_else(|| Error(tr!("Eksik JSON metni")))?;
			self.pos += 1;
			match c {
				b'"' => return String::from_utf8(out).map_err(|e| Error(e.to_string())),
				b'\\' => {
					let e = *self
						.bytes
						.get(self.pos)
						.ok_or_else(|| Error(tr!("Eksik JSON escape")))?;
					self.pos += 1;
					match e {
						b'"' | b'\\' | b'/' => out.push(e),
						b'b' => out.push(8),
						b'f' => out.push(12),
						b'n' => out.push(10),
						b'r' => out.push(13),
						b't' => out.push(9),
						b'u' => {
							let mut cp = self.hex()?;
							if (0xd800..=0xdbff).contains(&cp) {
								ensure(
									self.bytes.get(self.pos..self.pos + 2) == Some(b"\\u"),
									tr!("Eksik Unicode surrogate"),
								)?;
								self.pos += 2;
								let low = self.hex()?;
								ensure(
									(0xdc00..=0xdfff).contains(&low),
									tr!("Geçersiz Unicode surrogate"),
								)?;
								cp = 0x10000 + ((cp - 0xd800) << 10) + low - 0xdc00;
							}
							let ch =
								char::from_u32(cp).ok_or_else(|| Error(tr!("Geçersiz Unicode")))?;
							let mut buf = [0; 4];
							out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
						}
						_ => return Err(Error(tr!("Geçersiz JSON escape"))),
					}
				}
				0..=31 => return Err(Error(tr!("JSON kontrol karakteri"))),
				_ => out.push(c),
			}
		}
	}
	fn hex(&mut self) -> Result<u32> {
		let b = self
			.bytes
			.get(self.pos..self.pos + 4)
			.ok_or_else(|| Error(tr!("Eksik Unicode hex")))?;
		let s = std::str::from_utf8(b).map_err(|e| Error(e.to_string()))?;
		let n = u32::from_str_radix(s, 16).map_err(|e| Error(e.to_string()))?;
		self.pos += 4;
		Ok(n)
	}
	fn value(&mut self, depth: usize) -> Result<Value> {
		ensure(depth < 128, tr!("JSON iç içe geçme sınırı"))?;
		self.space();
		match self.bytes.get(self.pos).copied() {
			Some(b'"') => Ok(Value::String(self.string()?)),
			Some(b'{') => {
				self.pos += 1;
				let mut v = BTreeMap::new();
				if self.take(b'}') {
					return Ok(Value::Object(v));
				}
				loop {
					let k = self.string()?;
					self.need(b':')?;
					let value = self.value(depth + 1)?;
					ensure(v.insert(k, value).is_none(), tr!("Yinelenen JSON anahtarı"))?;
					if self.take(b'}') {
						break;
					}
					self.need(b',')?;
				}
				Ok(Value::Object(v))
			}
			Some(b'[') => {
				self.pos += 1;
				let mut v = Vec::new();
				if self.take(b']') {
					return Ok(Value::Array(v));
				}
				loop {
					v.push(self.value(depth + 1)?);
					if self.take(b']') {
						break;
					}
					self.need(b',')?;
				}
				Ok(Value::Array(v))
			}
			Some(b't') => {
				self.literal(b"true")?;
				Ok(Value::Bool(true))
			}
			Some(b'f') => {
				self.literal(b"false")?;
				Ok(Value::Bool(false))
			}
			Some(b'n') => {
				self.literal(b"null")?;
				Ok(Value::Null)
			}
			Some(b'-' | b'0'..=b'9') => {
				let start = self.pos;
				if self.bytes[self.pos] == b'-' {
					self.pos += 1;
				}
				match self.bytes.get(self.pos) {
					Some(b'0') => self.pos += 1,
					Some(b'1'..=b'9') => self.digits(),
					_ => return Err(Error(tr!("JSON sayısı geçersiz"))),
				}
				if self.bytes.get(self.pos) == Some(&b'.') {
					self.pos += 1;
					self.required_digits()?;
				}
				if matches!(self.bytes.get(self.pos), Some(b'e' | b'E')) {
					self.pos += 1;
					if matches!(self.bytes.get(self.pos), Some(b'+' | b'-')) {
						self.pos += 1;
					}
					self.required_digits()?;
				}
				Ok(Value::Number(
					String::from_utf8(self.bytes[start..self.pos].to_vec()).unwrap(),
				))
			}
			_ => Err(Error(tr!("JSON verisi geçersiz: konum {}", self.pos))),
		}
	}
	fn digits(&mut self) {
		while matches!(self.bytes.get(self.pos), Some(b'0'..=b'9')) {
			self.pos += 1;
		}
	}
	fn required_digits(&mut self) -> Result<()> {
		let before = self.pos;
		self.digits();
		ensure(self.pos > before, tr!("JSON rakamı eksik"))
	}
	fn literal(&mut self, bytes: &[u8]) -> Result<()> {
		ensure(
			self.bytes.get(self.pos..self.pos + bytes.len()) == Some(bytes),
			tr!("JSON sabiti geçersiz"),
		)?;
		self.pos += bytes.len();
		Ok(())
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	#[test]
	fn roundtrip() {
		let s = r#"{"x":[1,true,null,"\ud83d\ude80","Türkçe\n\t"],"n":1.5e-2}"#;
		let v = parse(s).unwrap();
		assert_eq!(v, parse(&v.encode()).unwrap());
	}
	#[test]
	fn rejects_malformed() {
		for s in [
			"01",
			"[1,]",
			"{\"a\":1,\"a\":2}",
			"\"\\ud800\"",
			"1.",
			"true garbage",
			"\"\n\"",
		] {
			assert!(parse(s).is_err(), "{s}");
		}
	}
}
