//! Turkish dotted/dotless I: ASCII protocol tokens remain stable, Unicode data is preserved.
use libesxi2pve::{Config, Language, Method, i18n, json, vmware};
const LETTERS: [&str; 8] = [
	"i",
	"İ",
	"ı",
	"I",
	"I\u{307}",
	"İstanbul",
	"Isparta",
	"ışık",
];
#[test]
fn language_ids_are_ascii_and_locale_independent() {
	for name in ["tr", "TR", "tr_TR.UTF-8", "TR_tr.utf8", "tr-TR"] {
		assert_eq!(Language::from_locale(name), Language::Turkish, "{name}");
	}
	for name in ["en_US.UTF-8", "C", "POSIX", "ı", "İ"] {
		assert_eq!(Language::from_locale(name), Language::English, "{name}");
	}
}
#[test]
fn json_preserves_raw_and_escaped_dotted_and_dotless_i() {
	for name in LETTERS {
		let wire = json::text(name).encode();
		assert_eq!(json::parse(&wire).unwrap().string().unwrap(), name);
	}
	for (escaped, expected) in [
		(r#""\u0069""#, "i"),
		(r#""\u0130""#, "İ"),
		(r#""\u0131""#, "ı"),
		(r#""\u0049""#, "I"),
		(r#""I\u0307""#, "I\u{307}"),
	] {
		assert_eq!(json::parse(escaped).unwrap().string().unwrap(), expected);
	}
}
#[test]
fn json_object_keys_remain_distinct() {
	let value = json::object(LETTERS.into_iter().map(|s| (s, json::text(s))));
	let decoded = json::parse(&value.encode()).unwrap();
	for key in LETTERS {
		assert_eq!(decoded.get(key).unwrap().string().unwrap(), key);
	}
}
#[test]
fn vmx_ascii_keys_are_case_insensitive_without_modifying_unicode_filenames() {
	for name in LETTERS {
		let filename = format!("{name}-disk.VMDK");
		let vmx = format!(
			"MEMSIZE=\"1024\"\nFIRMWARE=\"efi\"\nUEFI.SECUREBOOT.ENABLED=\"TRUE\"\nSCSI0.VIRTUALDEV=\"LSILOGIC\"\nSCSI0:0.FILENAME=\"{filename}\"\n"
		);
		let parsed = vmware::parse_vmx(&vmx, "/vmfs/volumes/DS-I/VM/g.vmx").unwrap();
		assert!(parsed.secure_boot);
		assert_eq!(
			parsed.disks[0].remote,
			format!("/vmfs/volumes/DS-I/VM/{filename}")
		);
	}
}
#[test]
fn escaped_vmx_utf8_bytes_survive_without_ascii_folding() {
	for (escaped, expected) in [("|c4|b0", "İ"), ("|c4|b1", "ı")] {
		let vmx = format!(
			"memsize=\"1024\"\nscsi0.virtualDev=\"lsilogic\"\nscsi0:0.fileName=\"{escaped}.vmdk\"\n"
		);
		let parsed = vmware::parse_vmx(&vmx, "/vmfs/volumes/ds/vm/g.vmx").unwrap();
		assert!(
			parsed.disks[0]
				.remote
				.ends_with(&format!("/{expected}.vmdk"))
		);
	}
}
#[test]
fn descriptor_and_remote_paths_preserve_all_i_variants() {
	for name in LETTERS {
		let basename = format!("{name}-flat.vmdk");
		let descriptor = vmware::parse_descriptor(&format!(
			"CID=1\nparentCID=ffffffff\nRW 16 VMFS \"{basename}\"\n"
		))
		.unwrap();
		assert_eq!(descriptor.extents[0].1, basename);
		let remote = format!("/vmfs/volumes/{name}/vm/{name}.vmdk");
		assert_eq!(vmware::safe_remote(&remote).unwrap(), remote);
	}
}
#[test]
fn ascii_machine_identifiers_do_not_accept_unicode_lookalikes() {
	let config = Config {
		esxi: "192.0.2.10".into(),
		esxi_vmid: 1,
		vmid: 100,
		storage: "ISCSI".into(),
		..Config::default()
	};
	assert!(config.validate().is_ok());
	for spelling in ["İSCSI", "ıscsi", "I\u{307}SCSI"] {
		let mut changed = config.clone();
		changed.storage = spelling.into();
		assert!(changed.validate().is_err());
	}
	for spelling in ["lİve-clone", "lıve-clone", "LIVE-CLONE"] {
		assert!(Method::parse(spelling).is_err());
	}
}
#[test]
fn language_scope_restores_and_preserves_turkish_text() {
	i18n::with_language(Language::English, || {
		i18n::with_language(Language::Turkish, || {
			assert_eq!(i18n::current_language(), Language::Turkish);
			assert_eq!(
				json::parse(&json::text("iİıI ŞĞÜÖÇ").encode())
					.unwrap()
					.string()
					.unwrap(),
				"iİıI ŞĞÜÖÇ"
			);
		});
		assert_eq!(i18n::current_language(), Language::English);
	});
}
