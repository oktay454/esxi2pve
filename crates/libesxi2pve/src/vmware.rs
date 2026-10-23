//! VMX/VMDK ve PVE metaverisi; tahmin ederek ilerlemek yerine doğrulama hatası verir.
use crate::{Error, Result, ensure};
use std::{
	collections::{BTreeMap, BTreeSet},
	path::{Path, PathBuf},
};
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Disk {
	pub slot: String,
	pub remote: String,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Nic {
	pub model: String,
	pub mac: String,
}
#[derive(Clone, Debug)]
pub struct Manifest {
	pub vmx: String,
	pub cores: u32,
	pub memory: u64,
	pub firmware: String,
	pub secure_boot: bool,
	pub controllers: Vec<String>,
	pub disks: Vec<Disk>,
	pub nics: Vec<Nic>,
}
pub fn safe_remote(s: &str) -> Result<String> {
	ensure(
		s.starts_with("/vmfs/volumes/")
			&& s.split('/').count() >= 5
			&& !s.chars().any(|c| c.is_control())
			&& !s.split('/').any(|p| matches!(p, ".." | "."))
			&& !s.contains('\\'),
		tr!("Güvensiz ESXi yolu"),
	)?;
	Ok(s.into())
}
pub fn datastore_path(value: &str, parent: Option<&str>) -> Result<String> {
	let path = if let Some(v) = value.strip_prefix('[') {
		let (ds, tail) = v
			.split_once(']')
			.ok_or_else(|| Error(tr!("Datastore yolu geçersiz")))?;
		ensure(
			!ds.contains('/') && !ds.is_empty(),
			tr!("Geçersiz datastore adı"),
		)?;
		format!("/vmfs/volumes/{ds}/{}", tail.trim_start())
	} else if value.starts_with('/') {
		value.into()
	} else {
		format!(
			"{}/{}",
			parent
				.and_then(|s| s.rsplit_once('/').map(|v| v.0))
				.ok_or_else(|| Error(tr!("VMX üst dizini gerekli")))?,
			value
		)
	};
	safe_remote(&path)
}
pub fn assignment(line: &str) -> Option<(&str, &str)> {
	let (k, v) = line.split_once('=')?;
	Some((k.trim(), v.trim()))
}
pub fn unquoted(s: &str) -> Result<&str> {
	s.strip_prefix('"')
		.and_then(|s| s.strip_suffix('"'))
		.ok_or_else(|| Error(tr!("Tırnaklı değer bekleniyor")))
}
fn vmx_decode(s: &str) -> Result<String> {
	let bytes = s.as_bytes();
	let mut out = Vec::new();
	let mut i = 0;
	while i < bytes.len() {
		if bytes[i] == b'|'
			&& i + 2 < bytes.len()
			&& let Ok(hex) = std::str::from_utf8(&bytes[i + 1..i + 3])
			&& let Ok(v) = u8::from_str_radix(hex, 16)
		{
			out.push(v);
			i += 3;
			continue;
		}
		out.push(bytes[i]);
		i += 1;
	}
	String::from_utf8(out).map_err(|e| Error(e.to_string()))
}
fn slot_key(s: &str) -> (String, u32, u32) {
	let split = s.find(|c: char| c.is_ascii_digit()).unwrap_or(s.len());
	let (a, b) = s[split..].split_once(':').unwrap_or(("0", "0"));
	(
		s[..split].into(),
		a.parse().unwrap_or(u32::MAX),
		b.parse().unwrap_or(u32::MAX),
	)
}
pub fn parse_vmx(text: &str, remote: &str) -> Result<Manifest> {
	let mut c = BTreeMap::new();
	for line in text.lines() {
		if let Some((key, value)) = assignment(line)
			&& value.starts_with('"')
		{
			ensure(
				c.insert(key.to_ascii_lowercase(), vmx_decode(unquoted(value)?)?)
					.is_none(),
				tr!("VMX yinelenen anahtar"),
			)?;
		}
	}
	let get = |k: &str, default: &str| c.get(k).cloned().unwrap_or_else(|| default.into());
	for (k, v) in &c {
		if k.contains("encryption") || k.starts_with("pcipassthru") || k.starts_with("vtpm") {
			ensure(
				v.is_empty() || v.eq_ignore_ascii_case("false"),
				tr!("Desteklenmeyen VM özelliği: {k}", k = k),
			)?;
		}
	}
	let mut disks = Vec::new();
	let mut controllers = BTreeSet::new();
	for (k, v) in &c {
		let Some(slot) = k.strip_suffix(".filename") else {
			continue;
		};
		if !["scsi", "sata", "ide", "nvme"]
			.iter()
			.any(|p| slot.starts_with(p))
			|| !slot.contains(':')
			|| !v.to_ascii_lowercase().ends_with(".vmdk")
		{
			continue;
		}
		if !get(&format!("{slot}.present"), "true").eq_ignore_ascii_case("true") {
			continue;
		}
		ensure(
			!get(&format!("{slot}.mode"), "").starts_with("independent"),
			tr!("Independent disk desteklenmez"),
		)?;
		ensure(
			get(&format!("{slot}.sharing"), "noSharing").eq_ignore_ascii_case("nosharing"),
			tr!("Paylaşımlı disk desteklenmez"),
		)?;
		if slot.starts_with("scsi") {
			let controller = slot.split(':').next().unwrap();
			controllers
				.insert(get(&format!("{controller}.virtualdev"), "lsilogic").to_ascii_lowercase());
			ensure(
				get(&format!("{controller}.sharedbus"), "none").eq_ignore_ascii_case("none"),
				tr!("Paylaşımlı SCSI veri yolu desteklenmez"),
			)?;
		}
		disks.push(Disk {
			slot: slot.into(),
			remote: datastore_path(v, Some(remote))?,
		});
	}
	disks.sort_by_key(|d| slot_key(&d.slot));
	ensure(
		!disks.is_empty() && disks.len() <= 6,
		tr!("1..6 disk gerekli"),
	)?;
	ensure(
		disks
			.iter()
			.map(|d| &d.remote)
			.collect::<BTreeSet<_>>()
			.len() == disks.len(),
		tr!("Disk birden çok yuvaya bağlı"),
	)?;
	let mut nics = Vec::new();
	for i in 0..10 {
		let prefix = format!("ethernet{i}.");
		if get(&(prefix.clone() + "present"), "false").eq_ignore_ascii_case("true") {
			let mac = get(
				&(prefix.clone() + "address"),
				&get(&(prefix.clone() + "generatedaddress"), ""),
			);
			let model = get(&(prefix + "virtualdev"), "e1000").to_ascii_lowercase();
			ensure(
				matches!(model.as_str(), "vmxnet3" | "e1000" | "e1000e"),
				tr!("Desteklenmeyen ağ kartı"),
			)?;
			ensure(
				mac.len() == 17
					&& mac.split(':').count() == 6
					&& mac
						.split(':')
						.all(|p| p.len() == 2 && p.bytes().all(|b| b.is_ascii_hexdigit())),
				tr!("MAC adresi geçersiz"),
			)?;
			nics.push(Nic { model, mac });
		}
	}
	ensure(
		nics.len() <= 1,
		tr!("Birden çok ağ kartı için ağ eşlemesi gerekli; bu sürüm tek kart destekler"),
	)?;
	let cores = get("numvcpus", "1")
		.parse()
		.map_err(|_| Error(tr!("CPU değeri geçersiz")))?;
	let memory = get("memsize", "")
		.parse()
		.map_err(|_| Error(tr!("Bellek değeri geçersiz")))?;
	ensure(cores > 0 && memory >= 16, tr!("CPU/bellek yetersiz"))?;
	let firmware = get("firmware", "bios");
	let secure = get("uefi.secureboot.enabled", "false");
	ensure(
		secure.eq_ignore_ascii_case("true") || secure.eq_ignore_ascii_case("false"),
		tr!("Secure Boot değeri belirsiz"),
	)?;
	let secure_boot = secure.eq_ignore_ascii_case("true");
	ensure(
		!secure_boot || firmware == "efi",
		tr!("Secure Boot için EFI gerekli"),
	)?;
	ensure(
		matches!(firmware.as_str(), "bios" | "efi"),
		tr!("Bilinmeyen firmware"),
	)?;
	Ok(Manifest {
		vmx: remote.into(),
		cores,
		memory,
		firmware,
		secure_boot,
		controllers: controllers.into_iter().collect(),
		disks,
		nics,
	})
}
pub fn config_path(text: &str) -> Result<String> {
	let found: Vec<_> = text
		.lines()
		.filter_map(assignment)
		.filter(|(k, _)| *k == "vmPathName")
		.collect();
	ensure(found.len() == 1, tr!("get.config VMX yolu tekil değil"))?;
	let s = found[0].1.trim_end_matches(',').trim();
	datastore_path(unquoted(s)?, None)
}
#[derive(Clone, Debug)]
pub struct Descriptor {
	pub size: u64,
	pub extents: Vec<(String, String)>,
	pub parent: Option<String>,
	pub cid: Option<u32>,
	pub parent_cid: Option<u32>,
}
fn plain_filename(s: &str) -> Result<()> {
	ensure(
		!s.is_empty()
			&& s != "."
			&& s != ".."
			&& !s.contains(['/', '\\', ':'])
			&& !s.chars().any(|c| c.is_control()),
		tr!("Descriptor dosya adı güvenli değil"),
	)
}
pub fn parse_descriptor(text: &str) -> Result<Descriptor> {
	for line in text.lines() {
		if let Some((key, value)) = assignment(line)
			&& key.eq_ignore_ascii_case("createType")
		{
			let kind = unquoted(value)?.to_ascii_lowercase();
			ensure(
				!kind.contains("rawdevice") && !kind.contains("rdm"),
				tr!("RDM/raw-device desteklenmez"),
			)?;
		}
	}
	let mut d = Descriptor {
		size: 0,
		extents: Vec::new(),
		parent: None,
		cid: None,
		parent_cid: None,
	};
	for line in text.lines() {
		let line = line.trim();
		let fields: Vec<_> = line.split_whitespace().collect();
		if matches!(fields.first(), Some(&"RW") | Some(&"RDONLY")) {
			ensure(fields.len() >= 4, tr!("Eksik extent tanımı"))?;
			let sectors = fields[1]
				.parse::<u64>()
				.map_err(|_| Error(tr!("Extent kapasitesi geçersiz")))?;
			ensure(sectors > 0, tr!("Sıfır extent"))?;
			d.size = d
				.size
				.checked_add(
					sectors
						.checked_mul(512)
						.ok_or_else(|| Error(tr!("Kapasite taşması")))?,
				)
				.ok_or_else(|| Error(tr!("Kapasite taşması")))?;
			ensure(
				matches!(
					fields[2],
					"VMFS" | "VMFSSPARSE" | "SESPARSE" | "FLAT" | "SPARSE"
				),
				tr!("Bilinmeyen extent biçimi"),
			)?;
			let start = line
				.find('"')
				.ok_or_else(|| Error(tr!("Extent dosya adı yok")))?;
			let rest = &line[start + 1..];
			let end = rest
				.find('"')
				.ok_or_else(|| Error(tr!("Extent tırnağı yok")))?;
			let name = &rest[..end];
			plain_filename(name)?;
			d.extents.push((fields[2].into(), name.into()));
		} else if let Some((key, value)) = assignment(line) {
			match key {
				"parentFileNameHint" => {
					ensure(d.parent.is_none(), tr!("Yinelenen parent"))?;
					let name = unquoted(value)?;
					plain_filename(name)?;
					d.parent = Some(name.into());
				}
				"CID" | "parentCID" => {
					ensure(!value.is_empty() && value.len() <= 8, tr!("CID uzunluğu"))?;
					let cid =
						u32::from_str_radix(value, 16).map_err(|_| Error(tr!("CID geçersiz")))?;
					if key == "CID" {
						d.cid = Some(cid)
					} else {
						d.parent_cid = Some(cid)
					}
				}
				_ => {}
			}
		}
	}
	ensure(d.size > 0 && !d.extents.is_empty(), tr!("Disk extent yok"))?;
	Ok(d)
}
pub fn validate_descriptor(path: &Path, mount: &Path, depth: u32) -> Result<Descriptor> {
	ensure(depth < 20, tr!("Descriptor zinciri çok uzun"))?;
	let canonical = path.canonicalize()?;
	ensure(
		canonical.starts_with(mount.canonicalize()?) && canonical.is_file(),
		tr!("Descriptor SSHFS alanı dışında"),
	)?;
	let d = parse_descriptor(&crate::process::read_text(path)?)?;
	let parent = path
		.parent()
		.ok_or_else(|| Error(tr!("Disk üst dizini yok")))?;
	for (_, name) in &d.extents {
		let p = parent.join(name).canonicalize()?;
		ensure(
			p.starts_with(mount.canonicalize()?) && p.is_file(),
			tr!("Extent eksik/SSHFS dışında"),
		)?;
	}
	if let Some(name) = &d.parent {
		let p = validate_descriptor(&parent.join(name), mount, depth + 1)?;
		ensure(
			d.parent_cid.is_some() && d.parent_cid == p.cid && d.size == p.size,
			tr!("VMDK parentCID/CID veya kapasite uyuşmuyor"),
		)?;
	}
	Ok(d)
}
pub fn parent_link(child: &Path, expected: &Path) -> Result<()> {
	let c = parse_descriptor(&crate::process::read_text(child)?)?;
	let p = parse_descriptor(&crate::process::read_text(expected)?)?;
	let name = c
		.parent
		.as_ref()
		.ok_or_else(|| Error(tr!("Snapshot parent yok")))?;
	ensure(
		child
			.parent()
			.unwrap_or(Path::new("/"))
			.join(name)
			.canonicalize()?
			== expected.canonicalize()?
			&& c.parent_cid.is_some()
			&& c.parent_cid == p.cid
			&& c.size == p.size,
		tr!("Son uygulanan parent/CID ile snapshot zinciri uyuşmuyor"),
	)
}
pub fn snapshot_id(text: &str, wanted: &str) -> Result<String> {
	let mut current = "";
	let mut found = Vec::new();
	for line in text.lines() {
		if let Some((_, v)) = line.split_once("Snapshot Name") {
			current = v.trim().trim_start_matches(':').trim();
		}
		if let Some((_, v)) = line.split_once("Snapshot Id")
			&& current == wanted
		{
			let v = v.trim().trim_start_matches(':').trim();
			ensure(
				v.bytes().all(|b| b.is_ascii_digit()) && !v.is_empty(),
				tr!("Snapshot kimliği geçersiz"),
			)?;
			found.push(v.to_string());
		}
	}
	ensure(found.len() == 1, tr!("Snapshot tekil doğrulanamadı"))?;
	Ok(found.remove(0))
}
pub fn storage_status(text: &str, name: &str) -> Result<(String, u64)> {
	let rows: Vec<Vec<_>> = text
		.lines()
		.map(|l| l.split_whitespace().collect::<Vec<_>>())
		.filter(|r| r.first() == Some(&name))
		.collect();
	ensure(
		rows.len() == 1 && rows[0].len() >= 6 && rows[0][2] == "active",
		tr!("Depolama etkin/tekil değil"),
	)?;
	let free = rows[0][5]
		.parse::<u64>()
		.map_err(|_| Error(tr!("Boş alan geçersiz")))?
		.checked_mul(1024)
		.ok_or_else(|| Error(tr!("Alan taşması")))?;
	Ok((rows[0][1].into(), free))
}
pub fn allocated_volume(text: &str, storage: &str) -> Result<String> {
	let found: Vec<_> = text
		.lines()
		.filter_map(|l| l.split_once("successfully created ").map(|(_, r)| r.trim()))
		.collect();
	ensure(
		found.len() == 1,
		tr!("Tahsis edilen birim kimliği tekil değil"),
	)?;
	let s = found[0];
	let quote = s.chars().next().unwrap_or(' ');
	ensure(matches!(quote, '\'' | '"'), tr!("Birim kimliği tırnaksız"))?;
	let v = s[1..].split(quote).next().unwrap_or("");
	ensure(
		v.starts_with(&format!("{storage}:"))
			&& !v.chars().any(|c| c.is_control() || c.is_whitespace()),
		tr!("Birim yanlış depolamada"),
	)?;
	Ok(v.into())
}
pub fn config_volume(text: &str, slot: &str) -> Result<String> {
	ensure(
		!text.lines().any(|l| l.starts_with('[')),
		tr!("Hedef yapılandırmada snapshot/pending bölümü var"),
	)?;
	let prefix = format!("{slot}:");
	let values: Vec<_> = text
		.lines()
		.filter_map(|l| l.strip_prefix(&prefix))
		.collect();
	ensure(values.len() == 1, tr!("Hedef disk yuvası tekil değil"))?;
	Ok(values[0].trim().split(',').next().unwrap_or("").into())
}
pub fn local_path(mount: &Path, remote: &str) -> Result<PathBuf> {
	safe_remote(remote)?;
	Ok(mount.join(remote.strip_prefix("/vmfs/volumes/").unwrap()))
}
/// ESXi CSV hem tırnaklı virgülleri hem çift tırnak kaçışlarını destekler.
pub fn csv(text: &str) -> Result<Vec<Vec<String>>> {
	let mut rows = Vec::new();
	let mut row = Vec::new();
	let mut field = String::new();
	let mut quoted = false;
	let mut chars = text.chars().peekable();
	while let Some(c) = chars.next() {
		match c {
			'"' if quoted && chars.peek() == Some(&'"') => {
				field.push('"');
				chars.next();
			}
			'"' => quoted = !quoted,
			',' if !quoted => {
				row.push(std::mem::take(&mut field));
			}
			'\n' if !quoted => {
				row.push(std::mem::take(&mut field));
				rows.push(std::mem::take(&mut row));
			}
			'\r' if !quoted => {}
			_ => field.push(c),
		}
	}
	ensure(!quoted, tr!("CSV tırnağı kapanmamış"))?;
	if !field.is_empty() || !row.is_empty() {
		row.push(field);
		rows.push(row);
	}
	Ok(rows)
}
pub fn source_free(text: &str, source: &str) -> Result<u64> {
	let source = safe_remote(source)?;
	let ds = source.split('/').nth(3).unwrap();
	let rows = csv(text)?;
	ensure(!rows.is_empty(), tr!("ESXi CSV boş"))?;
	let header: Vec<_> = rows[0]
		.iter()
		.map(|s| s.trim().to_ascii_lowercase())
		.collect();
	let mut found = Vec::new();
	for row in &rows[1..] {
		let get = |key: &str| {
			header
				.iter()
				.position(|k| k == key)
				.and_then(|i| row.get(i))
				.map(|s| s.trim())
				.unwrap_or("")
		};
		if [
			get("volumename"),
			get("uuid"),
			get("mountpoint").rsplit('/').next().unwrap_or(""),
		]
		.contains(&ds)
		{
			found.push(
				get("free")
					.parse::<u64>()
					.map_err(|_| Error(tr!("ESXi boş alan geçersiz")))?,
			);
		}
	}
	ensure(found.len() == 1, tr!("ESXi datastore tekil bulunamadı"))?;
	Ok(found[0])
}
#[cfg(test)]
mod tests {
	use super::*;
	#[test]
	fn rejects_paths() {
		for s in ["/tmp/a", "/vmfs/volumes/a/../b", "/vmfs/volumes/a/b\n"] {
			assert!(safe_remote(s).is_err());
		}
	}
	#[test]
	fn extent_spaces_cid() {
		let d = parse_descriptor("CID=1\nparentCID=ffffffff\nRW 16 VMFS \"a disk-flat.vmdk\"\n")
			.unwrap();
		assert_eq!(d.size, 8192);
		assert_eq!(d.cid, Some(1));
		assert_eq!(d.extents[0].1, "a disk-flat.vmdk");
	}
	#[test]
	fn reject_external_extent() {
		assert!(parse_descriptor("RW 16 VMFS \"../other\"").is_err());
	}
	#[test]
	fn csv_datastore() {
		assert_eq!(
			source_free(
				"MountPoint,VolumeName,UUID,Free\n/vmfs/volumes/uuid,\"ds,1\",uuid,1234\n",
				"/vmfs/volumes/uuid/a.vmdk"
			)
			.unwrap(),
			1234
		);
	}
}

#[cfg(test)]
mod vmx_tests {
	use super::*;
	#[test]
	fn escaped_unicode_and_boot_state() {
		let m=parse_vmx("memsize=\"1024\"\nfirmware=\"efi\"\nuefi.secureBoot.enabled=\"TRUE\"\nscsi0.virtualDev=\"pvscsi\"\nscsi0:0.fileName=\"Türk|20disk.vmdk\"\n", "/vmfs/volumes/ds/vm/g.vmx").unwrap();
		assert!(m.secure_boot);
		assert!(m.disks[0].remote.ends_with("Türk disk.vmdk"));
	}
	#[test]
	fn rejects_encrypted_shared_and_independent() {
		for extra in [
			"encryption.keySafe=\"secret\"",
			"scsi0.sharedBus=\"virtual\"",
			"scsi0:0.mode=\"independent-persistent\"",
		] {
			assert!(
				parse_vmx(
					&format!("memsize=\"1024\"\nscsi0:0.fileName=\"disk.vmdk\"\n{extra}\n"),
					"/vmfs/volumes/ds/vm/g.vmx"
				)
				.is_err()
			);
		}
	}
}

#[cfg(test)]
mod rdm_tests {
	#[test]
	fn filename_is_not_a_disk_type() {
		assert!(
			super::parse_descriptor("createType=\"vmfs\"\nRW 16 VMFS \"rdm-server-flat.vmdk\"\n")
				.is_ok()
		);
		assert!(
			super::parse_descriptor(
				"createType=\"vmfsRawDeviceMap\"\nRW 16 VMFS \"mapping.vmdk\"\n"
			)
			.is_err()
		);
		assert!(super::parse_descriptor("RW 16 VMFSRDM \"mapping.vmdk\"\n").is_err());
	}
}
