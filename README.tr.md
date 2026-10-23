# esxi2pve

[Türkçe](README.tr.md) | [English](README.md)

VMware ESXi sanal makinelerini Proxmox VE ortamına aktaran komut satırı aracı (CLI). Dört aktarım yöntemi, kalıcı günlükler, doğrulamalı disk aktarımı ve Türkçe/İngilizce arayüz sunar. Rust ile yazılmıştır ve `libesxi2pve` kütüphanesini kullanır.

## Dil seçimi

Türkçe ve İngilizce desteklenir. İlk boş olmayan değer şu sırayla kullanılır:

1. `ESXI2PVE_LANG` — yalnızca bu uygulama için açık dil seçimi.
2. `LC_ALL`
3. `LC_MESSAGES`
4. `LANG`

`tr`, `tr_TR.UTF-8`, `tr-TR` gibi değerler Türkçe seçer. İngilizce değerler, `C`/`POSIX`, desteklenmeyen diller veya hiçbir değişkenin tanımlanmamış olması İngilizce seçer. Boş değerler atlanır. Yüklü bir sistem yerel ayarı veya ek çeviri paketi gerekmez.

```console
ESXI2PVE_LANG=tr esxi2pve --help
ESXI2PVE_LANG=en esxi2pve --help
LANG=tr_TR.UTF-8 esxi2pve --help
```

Son örnekte daha yüksek öncelikli değişkenler ayarlıysa onlar geçerlidir. Etkileşimli onay Türkçe için `EVET`, İngilizce için `YES` ister; `--yes` her iki dilde aynı çalışır. Yardım, uygulama hataları, açıklayıcı günlükler ve `RECOVERY.txt` seçilen dilde yazılır. Komutlar, seçenekler, JSON alan adları, aşama kimlikleri ve dosya adları değişmez. `state.json` ve olay günlüklerinde `language` alanı `tr`/`en` değerini kaydeder. Mevcut kayıtlar sonradan başka dile çevrilmez. SSH/QEMU/PVE ve işletim sistemi kaynaklı ham çıktı/hatalar değiştirilmez.

## Kaynaktan derleme

Depo, tek Cargo çalışma alanında (workspace) iki paket içerir:

```text
esxi2pve/
├── Cargo.toml
├── Cargo.lock
├── crates/
│   ├── libesxi2pve/   aktarım kütüphanesi
│   └── esxi2pve/      komut satırı uygulaması
└── README.md
```

```console
cd esxi2pve
cargo build --offline --release
./target/release/esxi2pve --help
```

amd64 (`x86_64-unknown-linux-gnu`) için `.cargo/config.toml`, `-C target-feature=+crt-static` ayarıyla glibc tabanlı, konumdan bağımsız sabit bağlı çalıştırılabilir dosya (static PIE) üretir. Rust 1.89 veya yenisi, bağlayıcı (linker) ve glibc sabit arşivleri gerekir. Debian üzerinde bu arşivler normalde `libc6-dev` paketindedir. Diğer hedefler kendi derleme yapılandırmasını gerektirir.

Cargo komutunu depo kökünde çalıştırın; böylece `.cargo/config.toml` okunur. Başka bir dizinden `--manifest-path` kullanırsanız depo yapılandırmasını `--config` ile de belirtin. Sabit bağlamayı `file` ve `readelf` ile denetleyebilirsiniz: ikili `static-pie linked` göstermeli; dinamik yükleyici (`INTERP`) ve paylaşımlı kütüphane bağımlılığı (`NEEDED`) içermemelidir.

Oluşan `esxi2pve` ikilisini hedef PVE düğümüne taşıyın. Rust çalışma zamanı veya ayrı Rust kütüphane dosyası gerekmez. Harici SSH/QEMU/PVE araçları hedefte bulunmalıdır. İki paket kökteki kilit dosyasını, derleme yapılandırmasını ve sürüm profilini paylaşır. Varsayılan paket CLI'dir; ikisi için `--workspace`, yalnızca kütüphane için `-p libesxi2pve` kullanın.

## Gereksinimler

Hedef PVE üzerinde root, kullanılmayan VMID, etkin depolama ve Linux ağ köprüsü gerekir.

Yerel araçlar: `ssh`, `sshfs`, `qemu-img`, `qemu-nbd`, `qm`, `pvesm`, `fusermount3`, `findmnt`, `kill`. PVE'nin QEMU araçları `pve-qemu-kvm` paketindendir; PVE üzerinde farklı QEMU paketleriyle değiştirmeyin. SSHFS için `sshfs`, istemci için `openssh-client` gerekir. Eksikler hata ile listelenir; kendiliğinden kurulmaz.

ESXi üzerinde SSH, `vim-cmd`, `vmkfstools`, `esxcli`, `readlink`, `stat`, `dd`, `sha256sum` gerekir. Live Clone'un blok hash işlemi ESXi'nin yerleşik kabuğundaki `dd | sha256sum` boru hattını kullanır. `pipefail` desteklenmiyorsa işlem durur.

SSH sunucu anahtarı önceden doğrulanmış `known_hosts` içinde bulunmalıdır. Araç `StrictHostKeyChecking=yes` kullanır; sunucu anahtarını kendiliğinden kabul etmez. Önce mevcut anahtar/ssh-agent denenir. Başarısızsa gerçek terminalde OpenSSH parola sorar. Parola argüman, ortam değişkeni veya günlüğe yazılmaz. `--known-hosts` ve `--ssh-key` mutlak yol alır.

## Önce plan

Aşağıdaki IP, kimlik ve depolama adları örnektir:

```console
esxi2pve plan --method snapshot-iterative \
	--esxi 192.0.2.10 --esxi-vmid 12 --vmid 212 \
	--storage local-lvm --bridge vmbr0 --iterations 0
```

Plan, gerçek ESXi/PVE keşfi yapar; günlük ve SSH bağlantısı oluşturur. Salt okunur SSHFS/SFTP bağlantısını da kurup doğrular ve plan sonunda kaldırır. Kaynak VM'yi kapatmaz; snapshot, klon veya hedef VM oluşturmaz. `--method` yerine `--mode`, `snapshot-iterative` yerine `snapshot-delta` takma adı kabul edilir.

## Ana kopya ve yalnızca son fark

```console
esxi2pve migrate --method snapshot-iterative \
	--esxi 192.0.2.10 --esxi-vmid 12 --vmid 212 \
	--storage local-lvm --bridge vmbr0 --iterations 0 \
	--start --health-tcp 192.0.2.30:443
```

Akış:

1. Hedef VMID yerel kilidi alınır, kaynak incelenir ve salt okunur SSHFS/SFTP bağlantısı doğrulanır. Bu denetim başarısızsa kaynak kilidi veya hedef VM oluşturulmaz. Uygulama onayından sonra kaynak kilidi alınır ve kaynak donanımı yeniden okunur.
2. Hedef VM kapalı ve `migrate` kilitli oluşturulur; yeni ham (raw) diskler ayrılır.
3. `vim-cmd vmsvc/snapshot.create VMID AD AÇIKLAMA 0 1` ile belleksiz, konuk dosya sistemi dondurma (quiesce) istenen snapshot alınır. Her diskin yeni descriptor'a geçtiği ve snapshot kimliği doğrulanır.
4. Donmuş ana disk `vmkfstools -i ... -d thin` ile klonlanır. `qemu-img convert -n` ile hedefe aktarılır; `qemu-img compare` ve yerel SHA-256 kayıtları alınır. Doğrulanmış geçici klon kaldırılır. Kaynak VM bu sırada çalışır.
5. Geçiş onayından sonra `vim-cmd vmsvc/power.shutdown VMID` ile düzenli kapanış istenir. Güç durumu sorgulanır. Zorla kapatma/reset yoktur.
6. Son snapshot descriptor'ının parent/CID zinciri doğrulanır. `qemu-img map` ile yalnızca üst katmanın veri veya sıfır içeren mantıksal bölgeleri seçilir.
7. `qemu-nbd --read-only` çözümlediği zinciri sunar. Rust NBD istemcisi bu mantıksal bölgeleri okuyup hedefte aynı mantıksal konumlara yazar. Her 64MiB grupta `fsync` ve SHA-256 ile geri okuma denetimi yapılır. Ayrı doğrulama günlüğü diske eşitlenir.
8. Diskler bağlanır; kaynak kapalı olduğu yeniden doğrulanır. `--start` varsa hedef açılır ve hizmet denetlenir.

Son fark aşamasında bütün disk taranmaz. Veri tutarlılığı, ilk tam karşılaştırma + sıralı parent/CID zinciri + uygulanmış farkların geri okunmasına dayanır; son aşamada bağımsız tam disk karşılaştırması yapılmış sayılmaz. Uygulamanın düzenli kapanışının doğruluğu da gereklidir.

`--iterations 0` ön fark turunu kapatır. Varsayılan 6 tur, kaynak çalışırken ek snapshotlar alıp donmuş önceki katmanı sırasıyla uygular; tur farkı varsayılan 64MiB eşiğinin altına inince son geçişe gider. Yakınsama sağlanmazsa son fark büyük olabilir.

## Diğer yöntemler

| Yöntem | Akış | Kesinti ve doğrulama |
|---|---|---|
| `offline` | Düzenli kapat → tam kopyala → tam karşılaştır → hedefi hazırla | Bütün aktarım süresince kesinti; en sade akış |
| `live-clone` | Snapshot → donmuş klonu aktar → düzenli kapat → son zinciri yeniden klonla → 64MiB SHA-256 bloklarını karşılaştır → farklı blokları NBD ile uygula | Son klonlama ve iki uçtaki tam hash taraması kesinti süresine dahildir |
| `snapshot-iterative` | Donmuş ana klon → isteğe bağlı fark turları → düzenli kapat → son mantıksal fark | Küçük son farkla kısa kesinti hedefler; sabit süre garantisi yoktur |
| `live-mirror` | Düzenli kapat → uzak salt okunur disk + yerel qcow2 yazma katmanıyla hedefi aç → hizmeti doğrula → `qm move-disk` ile yerel diske canlı taşı → QMP ve kalıcı yapılandırmayı doğrula | Açılış uzak disk/ağ hızına bağlıdır; ölçülen ilk hizmet hazır olma süresi raporlanır |

Offline veya Live Clone için aynı örnekte `--method` değerini değiştirin, `--iterations` seçeneğini kaldırın.

Snapshot yönteminde `--delta-engine clone` açık seçimi de kullanılabilir: harita farkı yerine her donmuş zincir ESXi'de tam klonlanır ve hash blokları karşılaştırılır. Bu seçenek daha fazla alan, tarama ve süre gerektirir. Hatalarda kendiliğinden bu yönteme geçilmez.

## Canlı yansıtma (live mirror)

```console
esxi2pve migrate --method live-mirror \
	--esxi 192.0.2.10 --esxi-vmid 12 --vmid 212 \
	--storage local-lvm --staging-storage local \
	--start --health-tcp 192.0.2.30:443 --mirror-mib 64
```

Staging, hedef depolamadan farklı `dir` türünde, gerçek yerel ext2/3/4, XFS, Btrfs veya ZFS üzerinde olmalıdır. Bütün sanal disk kapasitesi +2GiB boş alan istenir; overlay en kötü durumda tamamen dolabilir. Her uzak disk için 64 rastgele 4KiB ve 16MiB ardışık örnek okuma yapılır. Varsayılan p95 sınırı 25ms, ardışık alt sınır 20MiB/s'dir. Bu örnek test uzun süreli performans garantisi değildir. Eşik aşılırsa hedef açılmaz.

Konuk yazmaları yerel qcow2 katmana gider. Önce hizmet denetimi geçer; sonra PVE'nin `qm move-disk --format raw --delete 0 --bwlimit ...` akışı QMP aynalama/pivot ve kalıcı yapılandırmayı yönetir. Hız üst sınırı, istenen hız ile ölçülen ardışık hızın yarısının küçüğüdür. MiB/s değeri PVE için KiB/s'ye çevrilir. Hata halinde canlı `dd` veya `qemu-img convert -U` geri dönüşü yoktur.

PVE kalıcı disk yolu ve QMP etkin disk kimliği, raw biçimi, backing bulunmaması ve işlerin bitmesi doğrulanır. Hizmet ikinci kez sınanır. Her ikisi geçtikten sonra `mirror-local.verified` yazılır. Kullanılmayan staging overlay `--delete 0` nedeniyle tutulur; incelemeden silmeyin.

**Live Mirror yarım kaldığında uzak NBD/SSHFS süreçleri ve yerel overlay çalışan VM'nin disk bağımlılığı olabilir. Kaynağı aynı anda açmayın, süreçleri veya PVE sunucusunu yeniden başlatmayın. `RECOVERY.txt` ve QMP/PVE disk yapılandırmasını inceleyin.**

## Hizmet ve donanım

`--start` kullanılmazsa doğrulanmış hedef kapalı bırakılır ve kesinti sona erdi sayılmaz. Live Mirror için `--start` zorunludur. CLI'nin `--health-tcp IP:PORT` denetimi yalnızca TCP bağlantısını sınar; hedef VM'ye ait adres/port verin. Veritabanı sorgusu gibi gerçek uygulama denetimi, kütüphanenin `with_health_check` Rust geri çağrısıyla (callback) eklenebilir. İlk hazır olma zamanı korunur; aynalama boyunca sürekli erişilebilirlik iddiası yoktur.

Kaynak PVSCSI/LSI, uygun PVE SCSI modeline eşlenir; diğerlerinde `--disk-bus`/`--scsihw` açık seçimi gerekir. BIOS/EFI ve Secure Boot etkinliği eşlenir. Kaynak NVRAM, özel Secure Boot anahtarları veya TPM durumu aktarılmaz; özel anahtar/önyükleme girdisi kullanan konuklar ek hazırlık gerektirebilir. Konuk sürücüsü yüklenmez, önyükleyici onarılmaz. İlk sıralı veri diski önyükleme diski kabul edilir. VMX snapshot konumunun veri diski dizininden farklı olduğu yapılandırmalar bu sürümde zincir doğrulamasında durabilir.

Sınırlar: 1..6 disk, en fazla 1 NIC; mevcut snapshot zinciri, RDM, independent/paylaşımlı disk/SCSI veri yolu, şifreleme, vTPM ve PCI geçişi reddedilir. Ham yazma hedefi yerel dosya veya blok aygıtı olmalıdır. Ceph'te ham yöntemler için krbd eşlemesi gerekir; librbd URI'ye kendiliğinden yazılmaz. PVE'nin canlı taşıma yolu farklı depolamaları destekleyebilir, fakat Ceph uçtan uca sınanmamıştır.

Snapshot öncesi datastore başına 16GiB rezerv, her klondan önce tam sanal disk kapasitesi +16GiB boş alan denetlenir. Bu sabit rezerv yazma hızına göre tükenebilir; aktarım sırasında datastore doluluğunu izleyin. Alan denetimi sürekli bir rezervasyon yapmaz.

## Kalıcı günlükler ve kurtarma

```text
/var/log/esxi2pve/İŞ.jsonl
/var/lib/esxi2pve/İŞ/
  state.json                  atomik yazılan aşama ve durum
  RECOVERY.txt                kurtarma bilgisi
  resources.jsonl             snapshot/klon/disk/NBD ve QMP envanteri
  commands/000001.argv.json   tam argüman dizisi
  commands/000001.stdout      yürütme sırasında yazılan çıktı
  commands/000001.stderr      yürütme sırasında yazılan hata/ilerleme
  commands/000001.result.json çıkış kodu ve zaman aşımı
  target-0.sha256.pending     hash taraması sürerken blok ilerlemesi
  final-ranges-0.verified.jsonl fsync sonrası doğrulanan fark blokları
```

Olay zamanları Unix zamanı (epoch) olarak kaydedilir. `source_connection`, `source_inspection`, `source_mount`, `source_lock` ve `target_creation` ayrı aşamalardır. Başarısız komutun stderr sonundaki en çok 8 satır / 8KiB bölüm hata mesajına eklenir; tam dosya korunur. `RECOVERY.txt` oluşturma, başlatma, kapatma ve bağlama girişimlerinin gerçek durumunu gösterir; Live Mirror bağımlılık uyarısı yalnızca o yöntem başlatılmış ve henüz bağımsızlık doğrulanmamışsa yazılır. Dosyalar 0600, yeni özel dizinler 0700 izinleriyle oluşturulur. SSH parola istemi özel olarak günlüğe yönlendirilmez. Hedef oluşturma girişiminden önce `target_create_attempted`, başlatılmadan önce `target_start_attempted` kalıcı yazılır; süreç kesilse bile kurtarma kaydı önceki kopyaya körlemesine dönülmemesini söyler.

```console
esxi2pve status /var/lib/esxi2pve/İŞ
```

Bu komut kalıcı `state.json` dosyasını okur; canlı VM veya işin hâlâ çalıştığını sorgulamaz. Uzun QEMU işlemlerinde ilgili `.stderr`, yerel taramada `.sha256.pending` ve olay günlüğü büyür. Fark aktarımının doğrulama dosyası her grupta diske eşitlenir. Günlük rotasyonu/otomatik saklama süresi uygulanmaz; dosyaları yalnızca işlem tamamlandıktan sonra arşivleyin.

Hata halinde kaynak VM/snapshotlar, yarım klonlar, hedef diskler ve gerekli SSHFS/NBD korunur. Otomatik silme, kaynak VM'yi geri açma ve kaldığı yerden sürdürme (resume) yoktur. SIGTERM/SIGKILL veya elektrik kesintisi sonrasında son durum/komut niyeti incelenmelidir. Komut zaman aşımında uzak ESXi işi sürebilir. Bu aracı systemd üzerinden çalıştıracaksanız hata halinde bağımlı NBD/SSH süreçlerinin birimin süreç grubuyla öldürülmeyeceğini ayrıca tasarlayın.

Kaynak `.esxi2pve.lock` kilidi işbirliğine dayalıdır; vSphere/PVE operatörlerinin veya başka yazılımların kaynak yapılandırmayı değiştirmesini engellemez. Aktarım boyunca başka snapshot/disk/donanım işlemi yapmayın. Başarıdan sonra kaynak VM ve snapshotlar saklanır; doğrulama ve geri dönüş süresi bitince yalnızca ilgili envantere göre elle temizleyin.

## Uyumluluk

Disk düzeni, konuk sürücüleri, aygıt yazılımı (firmware) ve depolama türü aktarım uyumluluğunu etkiler. VMFS/SEsparse, VMware Tools ile dosya sistemi dondurma (quiesce), konuk açılışı, PVE canlı disk taşıması ve Ceph kullanılan kaynak/hedef yapılandırmasında doğrulanmalıdır. Üretim kullanımından önce aktarımın tamamını ve uygulama sağlık denetimlerini bir sınama VM'siyle doğrulayın. Otomatik disk ve akış sınamaları her ESXi/PVE ortamında uçtan uca uyumluluk anlamına gelmez.

## Sorun giderme

`read: Connection reset by peer` bir SSH/SFTP bağlantı sorununu gösterebilir. ESXi üzerinde SFTP bulunduğunu, SSH sunucu anahtarının doğrulandığını ve bağlantı hesabının VM dizinini okuyabildiğini kontrol edin. Başarısız komutun `.stderr` dosyasını ve `RECOVERY.txt` içeriğini inceleyin.

Yarım kalan işin hedef VM'si ve kaynak `.esxi2pve.lock` kilidi korunur. Temizlikten önce işin bittiğini doğrulayın; `qm status VMID`, `qm config VMID`, `state.json` ve `resources.jsonl` dosyalarını inceleyin. Kullanılmayan boş kaynak kilidini yalnızca hiçbir aktarımın sahiplenmediğini doğruladıktan sonra `rmdir` ile kaldırın. Hedefi silmeye karar vermeden önce yarım diskleri ve VM durumunu değerlendirin.

## Paketler ve crates.io

Bu depodan iki ayrı paket yayımlanır: `libesxi2pve` kütüphane, `esxi2pve` komut satırı aracıdır. CLI bağımlılığında yerel yol ve sürüm gereksinimi birlikte bulunur; yayımlanan paket kütüphaneyi crates.io üzerinden çözümler.

Depo kökünde sınamaları çalıştırıp yerel paket arşivlerini oluşturabilirsiniz:

```console
cargo test --offline --workspace --all-targets
cargo clippy --offline --workspace --all-targets -- -D warnings
cargo package --workspace --allow-dirty
```

Önce kütüphaneyi yayımlayın; erişilebilir olmasını bekledikten sonra CLI'yi yayımlayın:

```console
cargo publish -p libesxi2pve --dry-run
cargo publish -p libesxi2pve
cargo publish -p esxi2pve --dry-run
cargo publish -p esxi2pve
```

Yayımlama için crates.io hesabı, kimlik doğrulaması ve paket adlarının sahipliği gerekir. Git gönderimi (push) crates.io yayını yapmaz. Yayından önce her arşivi inceleyin; kaynaklar, iki dilde benioku, lisans ve CLI çeviri kaynakları pakete dahil edilir.

Yayımlandıktan sonra `cargo install esxi2pve --locked` ile kurulabilir. Sabit bağlı amd64 GNU derlemesi için `RUSTFLAGS="-C target-feature=+crt-static" cargo install esxi2pve --locked --target x86_64-unknown-linux-gnu` kullanın; Cargo kurulumu bu deponun `.cargo/config.toml` dosyasını devralmaz. Harici PVE/SSH/QEMU araçları yine gereklidir.

## Lisans

GNU GPL sürüm 3 veya daha yeni bir sürüm (`GPL-3.0-or-later`) ile lisanslanmıştır. Tam metin: [LICENSE](LICENSE).
