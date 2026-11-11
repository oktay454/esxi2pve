# Yapay zekâ ile geliştirme kılavuzu

Bu dosya deponun tamamı için geçerlidir. Bir pakette çalışırken o paketin `AGENTS.md` dosyasını da oku. Paket yönergeleri bu dosyayı tamamlar. Kullanıcının açık talimatları bu yönergelerden önce gelir.

## Başlangıç ve çalışma düzeni

- Önce `git status --short` ile çalışma ağacını kontrol et. Mevcut kullanıcı değişikliklerini koru; istenmedikçe geri alma, sıfırlama veya temizleme yapma.
- [Türkçe kullanım belgesini](README.tr.md), [İngilizce kullanım belgesini](README.md) ve değiştirilecek paketin beniokusunu oku. Disk aktarımına dokunacaksan [aktarım mimarisini](docs/architecture.tr.md) de oku.
- İlgili kaynakları ve mevcut testleri okuyup sorunun nedenini belirle; sonra gerekli en küçük değişikliği yap. İlgisiz yeniden düzenleme (refactoring), yeni çalışma zamanı veya yeni geliştirme altyapısı ekleme.
- Dosya/metin aramada `rg --files` ve `rg` kullan. Üretilmiş `target/`, `.t/` ve `.r/` dizinlerini kaynak gibi inceleme.
- Bakımcıyla Türkçe, kısa ve somut iletişim kur. Gerektiğinde teknik terimin İngilizcesini parantez içinde belirt. Sonuçta ne değiştiğini, nasıl doğrulandığını ve varsa çalıştırılamayan denetimleri açıkla.
- Belirsizlik doğruluğu veya kapsamı etkiliyorsa sor. Yetkilendirilmiş olağan depo düzenlemeleri ve testler için tekrar tekrar onay isteme.
- Eksik paket veya bileşen gerekiyorsa önce kullanıcıya bildir; kendiliğinden kurma. Gerçek ESXi/PVE sisteminde işlem yapma ve yayımlama, ayrıca kullanıcı yetkilendirmesi gerektirir.
- Kullanıcı istemedikçe commit, push, sürüm etiketi veya crates.io yayını yapma. Kişisel yolları, kimlik bilgilerini ve çalışma çıktısını kaynaklara ekleme.

## Ürün ve paket sınırları

esxi2pve, ESXi sanal makinelerini Proxmox VE'ye aktaran bağımsız bir uygulamadır. Depo tek Cargo çalışma alanı (workspace) ve iki paket içerir:

| Konum | Sorumluluk |
|---|---|
| `crates/libesxi2pve/` | Aktarım motoru, yapılandırma, protokoller, doğrulama ve kalıcı kayıtlar |
| `crates/esxi2pve/` | Kütüphaneyi kullanan ince komut satırı arayüzü (CLI) |
| `docs/architecture.md`, `docs/architecture.tr.md` | Aktarım motorunun iki dilde ürün belgesi |
| `.cargo/config.toml` | GNU amd64 statik derleme ayarı |
| `rustfmt.toml` | Sekmeli Rust biçimlendirmesi |

Yeni aktarım mantığı kütüphaneye aittir. CLI içinde ikinci bir motor veya aynı doğrulamanın farklı kurallara sahip kopyasını oluşturma. Ayrıntılar: [kütüphane yönergeleri](crates/libesxi2pve/AGENTS.md), [CLI yönergeleri](crates/esxi2pve/AGENTS.md).

## Dil, bağımlılıklar ve biçim

- Uygulama Rust ile yazılır. Rust 2024 sürümünü ve kök `Cargo.toml` içindeki asgari Rust sürümünü (MSRV) koru; daha yeni derleyicide çalışan bir API'nin MSRV'de bulunduğunu varsayma.
- Kütüphanedeki `#![forbid(unsafe_code)]` kuralını koru. Mevcut uygulama standart kütüphaneyi kullanır; yeni Cargo bağımlılığı eklemeden önce gerekliliğini kullanıcıya bildir.
- Uygulama akışını Bash/Python yardımcılarına taşıma. Mevcut harici OpenSSH/SSHFS/QEMU/PVE araçları ürünün tanımlı çalışma sınırıdır.
- Girintileme sekme (tab), satır genişliği `rustfmt.toml` ile belirlenir. Elle ayrı bir stil uygulama; `cargo fmt --all` kullan. Mevcut modül düzenine ve Rust adlandırmasına uy.
- Hata yollarında mevcut `Result`, `Error`, `ensure` ve `tr!` düzenini kullan. Dış girdide `unwrap`/panic ile ilerleme; iş parçacıklarında hataların çağırana ulaşmasını sağla.
- İşletim sistemine özgü kararları Debian/Linux ve hedef PVE üzerinden değerlendir. Paketleme istenirse Debian standartlarını gözet; istenmeyen paketleme altyapısı ekleme.
- Sürüm ve ortak paket bilgileri kök `Cargo.toml` içindedir. Sürüm değişikliği gerektiğinde çalışma alanı bağımlılığını, `Cargo.lock` ve belgelerdeki sürüm örneklerini birlikte kontrol et. Yalnız belge/test düzenlemesi için kendiliğinden sürüm artırma.

## Korunacak davranışlar

- Dört yöntem `offline`, `live-clone`, `snapshot-iterative`, `live-mirror` olarak korunur. Paylaşılan bir işlevi değiştirirken diğer yöntemlerin başarı ve hata yollarını da incele.
- `plan` gerçek bağlantı/keşif ve SSHFS kontrolü yapar; çevrimdışı bir deneme komutu değildir. Test için gerçek sunucuya karşı `plan` veya `migrate` çalıştırma. Mevcut sahte yürütücüyü (mock executor) ve yerel disk örneklerini kullan.
- Kaynakta düzenli kapanış, güç durumu doğrulaması, kaynak/hedef kilitleri ve hedefin başlatılma sırası korunur. Belirsiz güç durumunu kapalı kabul etme; otomatik zorla kapatma/reset ekleme.
- Kaynak snapshotları ve hata halinde yarım hedefler korunur. Mevcut snapshot zincirine destek, otomatik snapshot silme, otomatik geri dönüş veya kaldığı yerden sürdürme (resume) mevcut özellikmiş gibi davranma; bunlar kapsamı belirlenmiş ayrı değişikliklerdir.
- Live Mirror tamamlanmadan SSHFS/NBD ve staging overlay etkin hedefin bağımlılığı olabilir. Temizleme veya hata yolu değişikliğinde bu bağımlılığı kesme. Diskin yerel olması, VM'nin açılması ve hizmetin sağlıklı olması ayrı sonuçlardır.
- Veri doğrulamasını performans için kaldırma veya sessizce zayıflatma. Kaynak zinciri, aralık kapsamı, `fsync`, geri okuma, ilk kaynak hash'i ve kalıcı tamamlanma kaydı arasındaki sıralamayı koru.
- Yerel komutlar argüman dizisiyle çalışır; uzak kabuk sözcükleri ve SSH/FUSE seçenekleri ilgili mevcut kaçış işlevlerinden geçer. Sunucu anahtarı doğrulamasını veya parola gizliliğini zayıflatma.
- Kaynaklar ve hata bilgisi kurtarma için kaydedilir. Başarısız işlemi başarılı gösterme; gerçek ortamda sınanmamış davranışı doğrulanmış sayma.

## Dil ve çıktı uyumluluğu

- Türkçe ve İngilizce kullanıcı metinlerini birlikte güncelle. Çeviri kataloğu `crates/libesxi2pve/src/messages.rs`, dil seçimi `i18n.rs`, CLI yardımları `crates/esxi2pve/locales/{tr,en}/help.txt` içindedir.
- Dil önceliği `ESXI2PVE_LANG > LC_ALL > LC_MESSAGES > LANG` şeklindedir. İş parçacığı yerel dil kapsamını koru; eşzamanlı testlerde/süreçte genel ortam değişkenlerini değiştirme.
- Protokol anahtarları, komut/seçenek adları, JSON alanları ve aşama kimlikleri dilden bağımsızdır. ASCII protokol karşılaştırmalarını kültüre bağlı büyük/küçük harf dönüşümüne çevirme. Kullanıcı verilerindeki `i`, `İ`, `ı`, `I` ve birleşik Unicode yazımları aynen korunmalıdır.
- Terminal sunumu ile kalıcı/makine tarafından işlenen çıktı ayrıdır. JSONL kayıtlarının, komut çıktı dosyalarının ve eski `status` girdilerinin uyumluluğunu koru. Terminal ilerlemesini seyrekleştirmek kalıcı kaydı eksiltmemelidir.
- İş adı tek sabit biçimdedir: `YYYY-MM-DDTHHMMSS.nnnnnnnnnZ_esxi-SOURCE_pve-TARGET_pPID`. Nanosaniye/PID yalnız çakışmada eklenen ikinci bir biçime dönüşmemelidir.

## Doğrulama

Komutları depo kökünde çalıştır. Varsayılan Cargo paketi CLI olduğundan bütün ürün için `--workspace` kullan.

```console
cargo fmt --all --check
cargo test --offline --workspace --all-targets
cargo clippy --offline --workspace --all-targets -- -D warnings
```

Değişiklikle ilgili dar testleri önce çalıştır; Rust davranışı değiştiyse tamamlamadan önce yukarıdaki çalışma alanı denetimlerini de yap. Yalnız belge değişikliğinde bağlantı/içerik ve `git diff --check` kontrolü yeterlidir; gereksiz tam derleme başlatma. Aynı değişiklik için geçmiş kontrolleri sebep olmadan tekrarlama.

| Değişiklik | İlgili test/denetim |
|---|---|
| Disk biçimi, NBD/QMP, aralıklar | `cargo test --offline -p libesxi2pve --test qemu` |
| Aktarım hattı ve tekrar | `cargo test --offline -p libesxi2pve transfer::` |
| Aktarım durum makinesi | `cargo test --offline -p libesxi2pve migration::tests` |
| Türkçe karakterler | `cargo test --offline -p libesxi2pve --test turkey` |
| CLI dili ve Unicode | `cargo test --offline -p esxi2pve --test locale` |
| CLI durum/JSON uyumluluğu | `cargo test --offline -p esxi2pve --test status` |
| Genel API ve belge örnekleri | `cargo test --offline --workspace --doc` ve `cargo doc --offline --workspace --no-deps` |

Dil değişikliklerinde Türkiye testlerini ve CLI dil testlerini `C`, `en_US.UTF-8`, `tr_TR.UTF-8` altında çocuk süreç ortamıyla doğrula. Testler geçici dosyalarını depo içindeki mevcut test dizinlerine yazmalı; gerçek VM, disk aygıtı veya uzak kimlik kullanmamalıdır.

Yerel QEMU/OpenSSH/SSHFS araçları bazı testlerde gerçekten çalışır. SSHFS ayrıştırıcı testleri sahte SSH kullansa da FUSE aygıtına erişim gerektirebilir. Böyle bir test yalnız kısıtlı çalıştırıcıda başarısızsa ortam/aygıt erişimini kontrol et; testi kaldırarak, atlayarak veya doğrulamayı azaltarak sonucu başarılı gösterme. Normal geliştirme/test düzenini koru; istenmedikçe ayrı kapsayıcı veya denetim süreci kurma.

Statik derleme veya paket kapsamı değiştiğinde ayrıca:

```console
cargo build --offline --release -p esxi2pve
file target/release/esxi2pve
readelf -l target/release/esxi2pve
readelf -d target/release/esxi2pve
cargo package --offline --workspace --allow-dirty
```

GNU amd64 ikilisi `static-pie linked` olmalı; `INTERP` ve `NEEDED` içermemelidir. Bu, harici SSH/QEMU/PVE araçlarını ikiliye dahil etmez. Dağıtılacak ikili değiştiğinde `target/release/esxi2pve.sha256` dosyasını da güncelle. Yerel paket doğrulaması crates.io yayını değildir.

## Belge ve teslim

README ve ürün belgeleri Türkçe/İngilizce birlikte tutulur. Belgeler mevcut davranışı açıklamalı; konuşma geçmişi, taslak parçaları, kişisel sunucu bilgileri veya doğrulanmamış özellik iddiaları içermemelidir. Davranış değiştiğinde yardım, örnekler ve ilgili testleri aynı değişiklikte güncelle.

Tamamlamadan önce `git diff --check`, değişiklik kapsamı ve ilgili test sonuçlarını kontrol et. Test/derleme çıktıları ve geçici dosyalar sürüm kontrolüne eklenmez. Son yanıtında değişen dosyaları, gerekçeyi, geçen/çalıştırılamayan testleri ve gerçek ESXi/PVE denemesi yapılıp yapılmadığını gerektiği ölçüde belirt. Mimari veya geliştirme komutları değişirse bu kılavuzu ve paket yönergelerini de güncelle.
