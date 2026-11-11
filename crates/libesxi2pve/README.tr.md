# libesxi2pve

[Türkçe](README.tr.md) | [English](README.md)

VMware ESXi sanal makinelerini Proxmox VE ortamına aktaran Rust kütüphanesi (library). Dört aktarım akışı, doğrulamalı disk aktarımı, kalıcı günlükler ve kurtarma durumu sağlar. `esxi2pve` komut satırı uygulaması bu kütüphaneyi kullanır.

Linux üzerinde Rust 1.89 veya yenisi gerekir. Üretim işlemleri hedef PVE düğümünde root olarak yürütülür. Harici SSH/QEMU/PVE araçları yine gereklidir; QEMU disk biçimlerini çözümlemek, PVE ise VM/depolama işlemlerini kendi araçlarıyla yürütmek için kullanılır.

## Dil seçimi

`Config::default().language`, `ESXI2PVE_LANG > LC_ALL > LC_MESSAGES > LANG` sırasındaki ilk boş olmayan değerden seçilir. Türkçe `tr`/`tr_TR.UTF-8`/`tr-TR` ile etkinleşir; diğer diller, `C`/`POSIX` ve boş ortam İngilizceye döner. Özel seçim diğer ortam değerlerinden önceliklidir.

```rust,no_run
use libesxi2pve::{Config, Language};
let config = Config {
	language: Language::English,
	..Config::default()
};
```

Doğrulama ve aktarım `Config.language` kullanır. Bağımsız ayrıştırıcı/hata/günlük işlevleri için `libesxi2pve::i18n::with_language(Language::Turkish, || { /* işlem */ })` kullanılabilir. Dil kapsamı iş parçacığına özeldir; iç içe çağrılardan veya panikten sonra önceki değer geri yüklenir. Alt iş parçacıkları için dili açıkça seçin. Ortam değişkenleri yalnızca okunur, süreç genelinde değiştirilmez.

Yardım, hatalar, onaylar, açıklayıcı günlük ve kurtarma metni çevrilir. JSON alanları ve protokol/komut sabitleri değişmez; harici araç/işletim sistemi metinleri olduğu gibi korunur. Türkçe onay `EVET`, İngilizce onay `YES` bekler. Katalog `src/messages.rs`, dil seçimi `src/i18n.rs`, CLI yardım kaynakları CLI paketinde `locales/tr/help.txt` ve `locales/en/help.txt` içindedir. Ek Cargo bağımlılığı veya yerel ayar paketi gerekmez.

## Kullanım

```toml
[dependencies]
libesxi2pve = "0.1.4"
```

```rust,no_run
use libesxi2pve::{Config, Method, Migration};

fn main() -> libesxi2pve::Result<()> {
	let config = Config {
		method: Method::SnapshotIterative,
		esxi: "192.0.2.10".into(),
		esxi_vmid: 12,
		vmid: 212,
		storage: "local-lvm".into(),
		iterations: 0,
		execute: false, // keşif ve plan; kaynak/hedef VM değiştirilmez
		..Config::default()
	};
	let report = Migration::new(config).run()?;
	println!("{}", report.state_dir.display());
	Ok(())
}
```

`execute=true` değişiklikleri uygular. `yes=false` başlangıç ve geçiş için terminalde onay ister. `start=false` hedefi kapalı bırakır. `start=true`, `Config.health` veya aşağıdaki özel Rust denetimini gerektirir.

```rust,ignore
let migration = Migration::new(config).with_health_check(
	std::time::Duration::from_secs(600),
	move |vmid| {
		// Burada uygulamaya özgü sorguyu yapın.
		// true: hazır; false: henüz hazır değil; Err: işlemi durdur.
		// Her çağrı kendi zaman aşımıyla sınırlı olmalıdır.
		Ok(my_service_is_ready(vmid))
	},
);
let report = migration.run()?;
```

Bu ikinci parça uygulamaya özgü `my_service_is_ready` işlevini gösteren şematik örnektir. CLI'nin hazır denetimi Rust `TcpStream::connect_timeout` kullanır; açık TCP portu, veritabanının veya uygulamanın doğru çalıştığı anlamına gelmez.

## Modüller

| Modül | İş |
|---|---|
| `i18n` | Dil önceliği, iş parçacığı kapsamı ve derleme sırasında denetlenen metin kataloğu |
| `config` | Yöntemler ve seçenek doğrulaması |
| `migration` | Dört aktarım akışı, kaynak/VMID kilitleri, kurtarma durumu |
| `transport` | SSHFS yerel seçenekleri ve FUSE kaçışları |
| `process` | Argüman dizileriyle komut yürütme, kalıcı stdout/stderr, zaman aşımı |
| `vmware` | VMX/VMDK ayrıştırma, CID zinciri, PVE/ESXi metaverisi |
| `transfer` | Eşzamansız NBD boru hattı, dengeli G/Ç, fsync, doğrudan geri okuma ve kontrollü tekrar |
| `disk` | NBD/QMP yardımcıları, mantıksal bölge doğrulaması ve kopyalama uyumluluk girişi |
| `json` | QEMU/QMP JSON ayrıştırma ve kodlama |
| `sha256` | Akışlı SHA-256; disk tutarlılığı karşılaştırması |

`CommandExecutor` arayüzü komut sınırını değiştirmeye olanak verir. Üretimde `ProcessExecutor` kullanılır. Sınamalarda ESXi/PVE yanıtları sahte yürütücüyle üretilirken QEMU disk işlemleri gerçekten çalıştırılır.

## Derleme ve sınama

```console
cargo test --offline --all-targets
cargo clippy --offline --all-targets -- -D warnings
cargo fmt --check
cargo doc --offline --no-deps
```

Sınamalar `sshfs`, `qemu-img`, `qemu-nbd`, `qemu-io`, `sha256sum` ve Debian'ın `ls`/`sleep` araçlarını kullanır. Yalnızca proje içindeki geçici disk dosyalarında çalışırlar; PVE VM oluşturmaz veya ESXi'ye bağlanmazlar. Eksik bileşen kurulmaz.

İşletim adımları, örnekler ve sınırlar için CLI'nin [Türkçe benioku](https://github.com/oktay454/esxi2pve/blob/main/README.tr.md) veya [İngilizce benioku](https://github.com/oktay454/esxi2pve/blob/main/README.md) belgesini okuyun. Üretim kullanımından önce kullanılan ESXi/PVE yapılandırmasında aktarım uyumluluğunu ve uygulama sağlık denetimlerini doğrulayın.

Türkiye sınamaları, `i/İ` ve `ı/I` karakterlerini, birleşik Unicode yazımlarını, JSON/VMX/dosya yollarını ve ASCII protokol kimliklerini kapsar. CLI sınamaları `C`, `en_US.UTF-8` ve `tr_TR.UTF-8` altında çocuk süreçlerle çalışır; süreç genelinin ortam değişkenleri değiştirilmez. SSHFS ayrıştırıcısını çalıştıran iki sınama, gerçek bağlantı kurmasa da FUSE aygıtına erişim gerektirebilir.

## Fark aktarım ayarları

`Config.transfer` alanı `TransferOptions` taşır. `max_depth` 1..64 (varsayılan 16), `memory_bytes` 16..1024 MiB (varsayılan 128 MiB), `verification` varsayılan `Verification::Direct` ve `read_timeout` varsayılan 120 saniyedir. `transfer::copy` donmuş kaynağın ayrık bölgelerini uygular ve `TransferReport` döndürür; `disk::logical_copy` varsayılan ayarlarla uyumluluk girişidir. Çağıran taraf kaynak neslini ve tek yazıcı sahipliğini sağlamalıdır.

Akış ve kayıt güvenceleri: [Aktarım motoru](https://github.com/oktay454/esxi2pve/blob/main/docs/architecture.tr.md).

## Lisans

GNU GPL sürüm 3 veya daha yeni bir sürüm (`GPL-3.0-or-later`). Tam metin: [LICENSE](LICENSE).

## Terminal sunumu

Config.verbose terminalde tam komut ayrıntılarını açar; kalıcı günlükler bu ayardan bağımsız olarak eksiksiz tutulur. presentation modülü aşama adlarını çevirir ve insan okunur durum özetleri üretir. Her iş kimliği aynı biçimi kullanır: dokuz basamaklı nanosaniye içeren UTC zamanı, kaynak/hedef VMID ve PID. Sunum, aktarım akışını, snapshot saklama veya doğrulama politikasını değiştirmez.
