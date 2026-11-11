//! İnsan okunur sunum. Kalıcı JSON kayıtlarının alanları ve komut dosyaları değişmez.
use crate::{i18n::Language, json::Value, process::CommandSpec};
use std::{
	io::{self, Write},
	process::Command,
	time::{Duration, Instant},
};

fn calendar(epoch: u64, offset: i32) -> String {
	let seconds = i128::from(epoch) + i128::from(offset);
	let days = seconds.div_euclid(86400);
	let daytime = seconds.rem_euclid(86400);
	let z = days + 719468;
	let era = z.div_euclid(146097);
	let day_of_era = z - era * 146097;
	let year_of_era =
		(day_of_era - day_of_era / 1460 + day_of_era / 36524 - day_of_era / 146096) / 365;
	let mut year = year_of_era + era * 400;
	let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
	let month_part = (5 * day_of_year + 2) / 153;
	let day = day_of_year - (153 * month_part + 2) / 5 + 1;
	let month = month_part + if month_part < 10 { 3 } else { -9 };
	year += i128::from(month <= 2);
	format!(
		"{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}",
		daytime / 3600,
		daytime / 60 % 60,
		daytime % 60
	)
}

/// Her işte aynı UTC/nanosaniye, kaynak, hedef ve PID düzeni kullanılır.
pub(crate) fn job_name(epoch: u64, nanos: u128, esxi: u32, pve: u32, pid: u32) -> String {
	let stamp = calendar(epoch, 0).replace(':', "");
	format!(
		"{stamp}.{:09}Z_esxi-{esxi}_pve-{pve}_p{pid}",
		nanos % 1_000_000_000
	)
}

fn parse_offset(value: &str) -> Option<i32> {
	let value = value.trim().as_bytes();
	if value.len() != 6
		|| value[3] != b':'
		|| !matches!(value[0], b'+' | b'-')
		|| ![1, 2, 4, 5].iter().all(|i| value[*i].is_ascii_digit())
	{
		return None;
	}
	let hours = i32::from(value[1] - b'0') * 10 + i32::from(value[2] - b'0');
	let minutes = i32::from(value[4] - b'0') * 10 + i32::from(value[5] - b'0');
	if hours > 23 || minutes > 59 {
		return None;
	}
	Some((hours * 3600 + minutes * 60) * if value[0] == b'-' { -1 } else { 1 })
}

#[derive(Default)]
struct Clock {
	minute: Option<u64>,
	offset: Option<i32>,
}
impl Clock {
	fn stamp(&mut self, epoch: u64) -> String {
		// Debian coreutils: TZ ve sistem saat dilimini date çözer. Her satırda
		// süreç başlatılmaz; ofset dakika başında yenilenerek DST geçişi izlenir.
		if self.minute != Some(epoch / 60) {
			self.minute = Some(epoch / 60);
			self.offset = Command::new("date")
				.args(["--date", &format!("@{epoch}"), "+%:z"])
				.output()
				.ok()
				.filter(|out| out.status.success())
				.and_then(|out| String::from_utf8(out.stdout).ok())
				.and_then(|text| parse_offset(&text));
		}
		let offset = self.offset.unwrap_or(0);
		let stamp = calendar(epoch, offset).replace('T', " ");
		if self.offset.is_none() {
			return format!("{stamp}Z");
		}
		format!(
			"{stamp}{}{:02}:{:02}",
			if offset < 0 { '-' } else { '+' },
			offset.abs() / 3600,
			offset.abs() / 60 % 60
		)
	}
}

/// Makineye yönelik aşama kimliğini seçilen dilde görüntüler.
pub fn phase_label(phase: &str) -> String {
	match phase {
		"initializing" => tr!("Hazırlanıyor"),
		"preflight" => tr!("Ön kontrol"),
		"source_connection" => tr!("ESXi bağlantısı"),
		"source_inspection" => tr!("Kaynak inceleme"),
		"source_mount" => tr!("Kaynak disk erişimi"),
		"source_lock" => tr!("Kaynak kilidi"),
		"target_creation" => tr!("Hedef oluşturma"),
		"online_precopy" => tr!("Çevrimiçi ön kopya"),
		"source_shutdown" => tr!("Kaynak kapanışı"),
		"snapshot_final_replay" => tr!("Son fark aktarımı"),
		"live_clone_final_replay" => tr!("Son klon farkları"),
		"target_configuration" => tr!("Hedef yapılandırması"),
		"service_readiness" => tr!("Hizmet denetimi"),
		"offline_copy" => tr!("Kapalı kaynak aktarımı"),
		"mirror_remote_boot" => tr!("Uzak diskten açılış"),
		"mirror_storage_move" => tr!("Yerel diske aynalama"),
		"cleanup" => tr!("Temizlik"),
		"completed" => tr!("Tamamlandı"),
		"planned" => tr!("Plan hazır"),
		name if name.starts_with("snapshot_iter") => tr!("Snapshot turu {}", &name[13..]),
		_ => phase.to_string(),
	}
}
fn level_label(level: &str) -> String {
	match level {
		"INFO" => tr!("BİLGİ"),
		"CMD" => tr!("KOMUT"),
		"PROGRESS" => tr!("İLERLEME"),
		"WARN" => tr!("UYARI"),
		"ERROR" => tr!("HATA"),
		_ => level.to_string(),
	}
}
fn duration_text(duration: Duration) -> String {
	let seconds = duration.as_secs();
	format!(
		"{:02}:{:02}:{:02}",
		seconds / 3600,
		seconds / 60 % 60,
		seconds % 60
	)
}
fn major_command(spec: &CommandSpec) -> bool {
	matches!(spec.program.as_str(), "qemu-img" | "qemu-nbd")
		|| (spec.program == "ssh"
			&& spec.args.last().is_some_and(|s| {
				s.starts_with("'vmkfstools' '-i'")
					|| s.starts_with("'vim-cmd' 'vmsvc/snapshot.create'")
					|| s.starts_with("'vim-cmd' 'vmsvc/power.shutdown'")
			}))
}
fn command_label(spec: &CommandSpec) -> String {
	match (spec.program.as_str(), spec.args.first().map(String::as_str)) {
		("qemu-img", Some("convert")) => tr!("Tam disk aktarımı"),
		("qemu-img", Some("compare")) => tr!("Tam disk karşılaştırması"),
		("qemu-img", Some("map")) => tr!("Fark haritası çıkarılıyor"),
		("qemu-img", Some("info")) => tr!("Disk bilgileri okunuyor"),
		("qemu-nbd", _) => tr!("Salt okunur disk erişimi açılıyor"),
		("ssh", _)
			if spec
				.args
				.last()
				.is_some_and(|s| s.starts_with("'vmkfstools' '-i'")) =>
		{
			tr!("ESXi üzerinde donmuş disk klonlanıyor")
		}
		("ssh", _)
			if spec
				.args
				.last()
				.is_some_and(|s| s.starts_with("'vim-cmd' 'vmsvc/snapshot.create'")) =>
		{
			tr!("Kaynak snapshotı oluşturuluyor")
		}
		("ssh", _)
			if spec
				.args
				.last()
				.is_some_and(|s| s.starts_with("'vim-cmd' 'vmsvc/power.shutdown'")) =>
		{
			tr!("Düzenli kapanış isteniyor")
		}
		_ => spec.program.clone(),
	}
}

#[derive(Default)]
pub(crate) struct Console {
	pub verbose: bool,
	clock: Clock,
	last_progress: Option<Instant>,
}
impl Console {
	fn render_line(&mut self, epoch: u64, level: &str, phase: &str, message: &str) -> String {
		format!(
			"{} [{}] [{}] {}",
			self.clock.stamp(epoch),
			level_label(level),
			phase_label(phase),
			message
		)
	}
	fn line(&mut self, epoch: u64, level: &str, phase: &str, message: &str) {
		// Terminal kapalı veya tüketici ayrılmış olsa da kalıcı günlükler sürer.
		let line = self.render_line(epoch, level, phase, message);
		let _ = writeln!(io::stderr().lock(), "{line}");
	}
	pub fn event(&mut self, epoch: u64, level: &str, phase: &str, message: &str) {
		if level == "CMD" && !self.verbose {
			return;
		}
		if level == "PROGRESS" && !self.verbose {
			// Yapılandırılmış ilerleme Journal::progress üzerinden gösterilir.
			return;
		}
		let phase_message;
		let message = if message == tr!("Aşama: {name}", name = phase) {
			self.last_progress = None;
			phase_message = tr!("Aşama başladı");
			&phase_message
		} else {
			message
		};
		self.line(epoch, level, phase, message);
	}
	pub fn progress(&mut self, epoch: u64, phase: &str, current: u64, total: u64, message: &str) {
		if self.verbose {
			return;
		}
		if current != 1
			&& current != total
			&& self
				.last_progress
				.is_some_and(|at| at.elapsed() < Duration::from_secs(2))
		{
			return;
		}
		self.last_progress = Some(Instant::now());
		let percent = if total == 0 {
			0.0
		} else {
			current as f64 * 100.0 / total as f64
		};
		let percent = match crate::i18n::current_language() {
			Language::Turkish => format!("%{percent:.1}").replace('.', ","),
			Language::English => format!("{percent:.1}%"),
		};
		self.line(epoch, "PROGRESS", phase, &format!("{percent} | {message}"));
	}
	pub fn command_start(&mut self, epoch: u64, phase: &str, index: u64, spec: &CommandSpec) {
		if !self.verbose && major_command(spec) {
			let label = command_label(spec);
			self.line(
				epoch,
				"CMD",
				phase,
				&tr!("{label} — komut #{index:06}", label = label, index = index),
			);
		}
	}
	pub fn command_end(
		&mut self,
		epoch: u64,
		phase: &str,
		index: u64,
		spec: &CommandSpec,
		code: i32,
		elapsed: Duration,
	) {
		if self.verbose {
			return;
		}
		if code != 0 {
			self.line(
				epoch,
				"WARN",
				phase,
				&tr!(
					"Komut #{index:06}: {program}, çıkış kodu {code}",
					index = index,
					program = spec.program,
					code = code
				),
			);
		} else if major_command(spec) {
			self.line(
				epoch,
				"CMD",
				phase,
				&tr!(
					"Komut #{index:06} tamamlandı; süre {duration}",
					index = index,
					duration = duration_text(elapsed)
				),
			);
		}
	}
	pub fn location(&mut self, epoch: u64, state: &std::path::Path, log: &std::path::Path) {
		self.line(
			epoch,
			"INFO",
			"initializing",
			&tr!("İş dizini: {}", state.display()),
		);
		self.line(
			epoch,
			"INFO",
			"initializing",
			&tr!("Olay günlüğü: {}", log.display()),
		);
	}
}

/// Aynı JSON nesnesinin terminale yönelik özeti. JSON çıktı için nesne doğrudan kodlanır.
pub fn status_summary(state: &Value) -> String {
	let text = |key: &str| state.optional(key).and_then(|v| v.string().ok());
	let number = |key: &str| state.optional(key).and_then(|v| v.integer().ok());
	let mut lines = Vec::new();
	if let Some(job) = text("job") {
		lines.push(tr!("İş: {}", job));
	}
	if let Some(phase) = text("phase") {
		lines.push(tr!("Durum: {}", phase_label(phase)));
	}
	if let Some(host) = text("esxi") {
		lines.push(tr!("Kaydedilmiş durum; canlı VM sorgulanmadı."));
		lines.push(tr!(
			"Kaynak: {host} / ESXi VMID {vmid}",
			host = host,
			vmid = number("esxi_vmid")
				.map(|n| n.to_string())
				.unwrap_or_else(|| "?".into())
		));
	}
	if let Some(vmid) = number("vmid") {
		lines.push(tr!("Hedef PVE VMID: {}", vmid));
	}
	if let Some(method) = text("method") {
		lines.push(tr!("Yöntem: {}", method));
	}
	if let Some(path) = text("state_dir") {
		lines.push(tr!("İş dizini: {}", path));
	}
	if let Some(path) = text("log_file") {
		lines.push(tr!("Olay günlüğü: {}", path));
	}
	if let Some(seconds) = number("downtime_seconds") {
		lines.push(tr!(
			"İlk hizmet denetimine kadar kesinti: {}",
			duration_text(Duration::from_secs(seconds))
		));
	}
	lines.join("\n")
}

#[cfg(test)]
mod tests {
	use super::*;
	#[test]
	fn readable_utc_names_keep_identity_and_sort_by_start_time() {
		let name = job_name(1792768847, 123456789, 72, 108281, 3804955);
		assert_eq!(
			name,
			"2026-10-23T152047.123456789Z_esxi-72_pve-108281_p3804955"
		);
		assert!(
			name.bytes()
				.all(|c| c.is_ascii_alphanumeric() || b"-_.".contains(&c))
		);
		assert!(name < job_name(1792768848, 123456789, 72, 108281, 3804955));
		assert_ne!(name, job_name(1792768847, 123456789, 73, 108281, 3804955));
	}
	#[test]
	fn calendar_and_offsets_cover_day_rollovers_and_leap_years() {
		assert_eq!(calendar(0, 0), "1970-01-01T00:00:00");
		assert_eq!(calendar(0, -3600), "1969-12-31T23:00:00");
		assert_eq!(calendar(951782400, 0), "2000-02-29T00:00:00");
		assert_eq!(calendar(1792780297, 10800), "2026-10-23T21:31:37");
		assert_eq!(parse_offset("+03:00"), Some(10800));
		assert_eq!(parse_offset("-03:30\n"), Some(-12600));
		assert_eq!(parse_offset("+05:45"), Some(20700));
		for value in ["x03:00", "+24:00", "+03:60", "+0x:00", "", "+0300"] {
			assert_eq!(parse_offset(value), None);
		}
	}
	#[test]
	fn labels_and_summaries_are_localized_without_changing_state() {
		let state = crate::json::object([
			("job", crate::json::text("legacy-job")),
			("phase", crate::json::text("snapshot_iter2")),
		]);
		let original = state.encode();
		crate::i18n::with_language(Language::Turkish, || {
			assert_eq!(phase_label("snapshot_iter2"), "Snapshot turu 2");
			assert!(status_summary(&state).contains("İş: legacy-job"));
		});
		crate::i18n::with_language(Language::English, || {
			assert_eq!(phase_label("snapshot_iter2"), "Snapshot round 2");
			assert!(status_summary(&state).contains("Job: legacy-job"));
		});
		assert_eq!(state.encode(), original);
		assert_eq!(phase_label("future_phase"), "future_phase");
	}
}

#[cfg(test)]
mod console_tests {
	use super::*;
	#[test]
	fn human_output_uses_explicit_offsets_and_localized_phase_labels() {
		let epoch = 1792780297;
		let mut console = Console {
			clock: Clock {
				minute: Some(epoch / 60),
				offset: Some(10800),
			},
			..Console::default()
		};
		crate::i18n::with_language(Language::Turkish, || {
			let line = console.render_line(epoch, "INFO", "snapshot_final_replay", "test");
			assert_eq!(
				line,
				"2026-10-23 21:31:37+03:00 [BİLGİ] [Son fark aktarımı] test"
			);
		});
		console.clock.offset = None;
		crate::i18n::with_language(Language::English, || {
			let line = console.render_line(epoch, "INFO", "snapshot_final_replay", "test");
			assert_eq!(
				line,
				"2026-10-23 18:31:37Z [INFO] [Final delta transfer] test"
			);
		});
	}
	#[test]
	fn only_long_running_commands_are_expanded_by_default() {
		let spec =
			|program, args: Vec<&str>| CommandSpec::new(program, args, Duration::from_secs(1));
		assert!(!major_command(&spec(
			"ssh",
			vec!["host", "'vim-cmd' 'vmsvc/power.getstate' '72'"]
		)));
		assert!(major_command(&spec(
			"ssh",
			vec!["host", "'vim-cmd' 'vmsvc/power.shutdown' '72'"]
		)));
		assert!(major_command(&spec("qemu-img", vec!["convert"])));
		assert!(!major_command(&spec("qm", vec!["list"])));
	}
}
