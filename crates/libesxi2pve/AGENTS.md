# libesxi2pve yönergeleri

Bu paket için önce [depo kılavuzunu](../../AGENTS.md) oku. Aşağıdaki kurallar kütüphane kaynakları ve testleri için ek ayrıntılardır.

## Kaynak haritası

| Dosya/modül | Değişiklik yeri |
|---|---|
| `src/config.rs` | Ortak seçenekler, varsayılanlar ve doğrulama |
| `src/migration.rs` | Dört aktarım akışı, kilitler, güç durumu, snapshot/clone, kurtarma ve sağlık denetimi |
| `src/process.rs` | `CommandExecutor`, süreç yürütme, kalıcı komut çıktıları ve günlükler |
| `src/transport.rs` | OpenSSH yapılandırma yolları ve SSHFS/FUSE seçenek aktarımı |
| `src/vmware.rs` | VMX/VMDK/CSV/PVE ayrıştırma, yol ve parent/CID zinciri doğrulaması |
| `src/disk.rs` | NBD/QMP yardımcıları, aralıklar, hash taraması |
| `src/transfer.rs`, `src/transfer/` | Eşzamansız (async) okuma, sınırlı tamponlar, yazma/fsync/doğrulama ve G/Ç denetimi |
| `src/i18n.rs`, `src/messages.rs` | Dil kapsamı ve iki dilde mesaj kataloğu |
| `src/presentation.rs` | İnsan okunur terminal sunumu ve iş adı |
| `src/json.rs`, `src/sha256.rs` | Paylaşılan JSON ve SHA-256 uygulaması |

## API ve yürütme sınırı

- Ortak davranış `Config`, `Migration` ve ilgili kütüphane işlevlerinde uygulanır. Yeni seçenek kütüphane kullanıcısı için de doğrulanmalı; yalnız CLI kontrolüne güvenme.
- `Migration::with_executor` ve mevcut sahte ESXi/PVE yürütücüsü dış komut sınırını test etmek içindir. Harici işlem eklerken niyet/sonuç günlüğünü ve hata durumundaki kaynak envanterini koru.
- CLI'ye özgü çıktı yazdırmayı aktarım mantığına ekleme. Mevcut ilerleme/gözlemci (observer) ve günlük arayüzlerini kullan.
- Genel API değişikliğinde tüketici örneklerini ve CLI çağırıcısını birlikte kontrol et. Uyumluluk etkisini belirt; gereksiz API kırılması yapma.

## Disk aktarımında değişmezler (invariants)

- Aralıkları yazmaya başlamadan önce taşma, sınır, çakışma ve kapasite bakımından doğrula. Byte/sektör/KiB/MiB birimlerini açık tut; tamsayı dönüşümlerini sınır kontrolünden sonra yap.
- Bir turda bir aralığın tek yazma sahibi bulunur. Aynı hedef adresine yazan snapshot nesilleri sırayla uygulanır; sıfır bölgelerini atlama.
- Bekleyen istekleri ve bütün veri tamponlarını mevcut bütçe hesabına dahil et. Geri okuma/tekrar payını koru; sınırsız veri kuyruğu ekleme.
- Gruba ait yazmalar bitmeden `fsync`; eşitleme bitmeden doğrulama; doğrulama ve günlük eşitlemesi bitmeden tamamlanma bildirimi yapılamaz.
- Özet uyuşmazlığında yalnız sorunlu parçanın kontrollü tek tekrarı vardır. Kaynak yeniden okuması ilk hash ile eşleşmelidir. Değişen kaynağı yeni beklenen hash kabul ederek devam etme. Tur başına toplam tekrar sınırını koru.
- Aygıt, zincir, izin, kapasite ve eşitleme hatalarını genel bir tekrar döngüsüne alma. Hata dönüşünden önce bütün yerel hedef G/Ç işçilerinin bitmesini bekle; timeout'u iptal garantisi sayma.
- Doğrudan geri okuma varsayılanını, hizalama/kapasite denetimini ve kaynak kapanışından önce hedef okuma desteği denetimini koru. Önbellekli okumaya geçiş açık yapılandırma ve uyarı gerektirir.
- NBD yanıtlarını istek kimliğiyle bağla; QMP olaylarını komut yanıtı sayma. Yanlış/eksik/tekrarlı yanıtta hata yolunu izle.
- Descriptor parent/CID ve kanonik yol kontrolleri korunmalıdır. Bir dosyayı incelemek, başka araçların onu değiştiremeyeceğini garanti etmez; yeni akışta kaynak nesli varsayımını açık tut.

## Mesajlar ve testler

Yeni kullanıcı metnini `tr!` kataloğunun iki diline ekle. Yeni iş parçacığına çağıranın dilini açıkça aktar; thread-local kapsamın kendiliğinden taşındığını varsayma. Ham araç çıktısını veya kullanıcı dosya adını çevirmeye çalışma.

`src/migration_tests.rs` sahte kontrol düzlemi ile gerçek yerel QEMU'yu, `tests/qemu.rs` disk/protokol tümleşimini (integration), `src/transfer/tests.rs` hata/tekrar/eşzamanlılığı, `tests/turkey.rs` Unicode ve ASCII ayrımını sınar. Değişikliğin bozabileceği davranış için anlamlı bir regresyon testi ekle; uygulamanın satırlarını tekrar eden test yazma.

Kaynak/hedef başlatma, snapshot silme veya gerçek depolama aygıtı test düzeneğinin parçası olamaz. Aktarım hızındaki yapay gecikme testinin sonucunu gerçek ESXi/PVE hız garantisi olarak sunma. Kütüphane/paket beniokularını iki dilde tut.
