# Aktarım motoru

[English](architecture.md) | [Türkçe](architecture.tr.md)

Fark aktarım motoru, donmuş bir kaynak disk zincirinin seçilmiş mantıksal bölgelerini yerel ham dosyaya veya blok aygıtına uygular. QEMU disk biçimini çözümler; kaynak okumalarını, hedef yazmalarını, SHA-256 doğrulamasını ve kalıcı doğrulama kayıtlarını Rust yönetir. İlk tam kopya ve karşılaştırma QEMU üzerinden yürütülür.

## İş akışı

```text
Donmuş kaynak + doğrulanmış descriptor/CID zinciri
    ↓
Örtüşmeyen mantıksal bölgeleri en fazla 4 MiB parçalara ayır
    ↓
Tek NBD bağlantısında birden fazla eşzamansız okuma isteği
    ↓
Bellek bütçesiyle sınırlı veri kuyruğu
    ↓
Kaynak SHA-256 → hedefte aynı adrese yaz
    ↓
Yazması bitmiş parçaları kalıcılaştırma grubuna al
    ↓
fsync → hedefi geri oku → SHA-256 karşılaştır
    ├─ Eşleşti → doğrulama kaydını yaz ve fsync
    └─ Uyuşmadı → kaynağı yeniden oku; ilk özetle aynı mı?
                     ├─ Hayır → hata
                     └─ Evet → bir kez yaz, fsync, geri oku ve doğrula
    ↓
Bütün plan kalıcı doğrulandı, bekleyen iş yok → tamamlandı
```

Kaynak olay döngüsü (event loop), tek NBD bağlantısında bloke etmeyen istek/yanıt çoğullaması (async multiplexing) kullanır. Sırası değişen yanıtlar istek kimliğiyle doğru adreslere eşleştirilir. Ayrı bir yazıcı iş parçacığı (thread) kaynak özetini hesaplayıp konumsal yazma yapar. Ayrı bir kalıcılaştırma işçisi eşitleme, geri okuma ve doğrulama günlüğünü yönetir. Ek bir eşzamansız çalışma zamanı (async runtime) gerekmez.

Bu aşamalar farklı gruplar üzerinde örtüşür. Önceki grup doğrulanırken sonraki grubun okumaları ve ayrık adreslerdeki yazmaları ilerleyebilir. Bir turun her bölgesinin tek sahibi vardır. Diskler ve snapshot turları bağımlılık sırasıyla tamamlanır; farklı kaynak nesilleri aynı hedef adresine eşzamanlı uygulanmaz. QEMU haritasındaki sıfır bölgeleri de aktarım planına dahildir.

## Sınırlar ve G/Ç dengesi

| Ayar | Varsayılan |
|---|---:|
| Kaynak okuma parçası | En fazla 4 MiB |
| İlk bekleyen okuma sayısı | 2; üst sınır 1 ise 1 |
| Okuma derinliği üst sınırı | 16; `--io-max-depth` ile 1..64 |
| Veri tamponu bütçesi | 128 MiB; `--io-memory-mib` ile 16..1024 MiB |
| Kalıcılaştırma grubu | En fazla 64 MiB veya 4096 parça |
| Hedef başına eşzamanlı kalıcılaştırma | 1 |
| Kaynak NBD okuma zaman aşımı | 120 saniye; kütüphanede ayarlanabilir |
| Geri okuma | Doğrudan G/Ç; `--verify-read direct` |

Yazıcı 100 ms boyunca yeni parça almazsa kısmi grubu da gönderir. Grup eşitlenmeden önce o gruba ait bütün yazmaların bitmiş olması gerekir. Grubun bütün parçaları doğrulandıktan sonra doğrulama kayıtları yazılıp eşitlenir. Yeni günlük dosyalarının dizin girişleri de eşitlenir. Başarı ve ilerleme, yalnızca bu kalıcı kayıtları tamamlanmış veriyi sayar.

Bellek bütçesi bekleyen kaynak okumalarını, yanıtları ve yazma kuyruğundaki veriyi sınırlar; geri okuma ve tekrar için ayrıca aynı bütçeden pay ayrılır. Grup üst verileri de parça sayısıyla sınırlıdır. Bu seçenek tüm süreç belleği veya çekirdeğin sayfa önbelleği için üst sınır değildir.

Dengeli denetleyici saniyelik pencerelerde hedef yazma/geri okuma gecikmesini, `fsync` süresini, kuyruk birikmesini ve bulunabiliyorsa aygıt kuyruğu ile Linux G/Ç baskısını (PSI) değerlendirir. İki ardışık yoğun pencerede yeni okuma derinliğini yarıya indirir ve gönderimler arasına 5–100 ms gecikme ekler. Üç uygun kararlı pencereden sonra önce gecikmeyi azaltır, sonra derinliği bir artırmayı dener. Derinlik denemesi en az %5 hız kazandırmazsa önceki değere döner. Çalışan istekler iptal edilmez; yeni gönderimler kısılır.

Yerel aygıt ve PSI ölçümleri, aynı SAN/depolamayı kullanan diğer düğümlerin bütün yükünü göstermez. Bulunamayan ölçümler JSON'da `null` olur. Bu mekanizma diğer VM'lerde sıfır gecikme garantisi vermez; gözlenen baskıya göre ihtiyatlı davranır.

## Kalıcılık ve geri okuma

Varsayılan doğrulama, Linux `O_DIRECT` ile çekirdek sayfa önbelleğini atlayarak geri okur. Güvenli Rust ile hizalı tampon kullanılır; sektör boyutunda küçük parçalar ve disk sonu desteklenir. Hedef ayrıldıktan sonra, kaynak VM kapatılmadan önce bu okuma yolu denenir. Destek yoksa işlem durur; sessizce önbellekli okumaya geçilmez.

`--verify-read buffered` açıkça seçilirse normal geri okuma kullanılır ve uyarı kaydedilir. Bu okuma sayfa önbelleğinden karşılanabilir. Her iki yöntemde de önce `fsync` beklenir; yazılımın güvencesi depolamanın bildirdiği kalıcılık davranışına dayanır. Doğrudan okuma, donanımın elektrik kesintisindeki davranışını fiziksel olarak sınamaz.

## Hata ve kontrollü tekrar

Özet uyuşmazlığında yalnızca sorunlu parça bir kez yeniden aktarılır. Kaynak tekrar okunur ve ilk kaynak özetiyle karşılaştırılır. Kaynak değişmişse beklenen özet değiştirilmez; işlem durur. Kaynak aynıysa hedef yeniden yazılır, eşitlenir ve tekrar doğrulanır. İkinci uyuşmazlık hatadır. Her disk/fark turunda en fazla üç böyle tekrar yapılır; dördüncü uyuşmazlık işlemi durdurur. İlk uyuşmazlık ve tekrar sonucu ayrı kalıcı kayıtlara yazılır.

Okuma/yazma aygıt hatası, kaynak zinciri hatası, dolu disk, izin veya `fsync` hatası otomatik tekrar edilmez. Descriptor zincirinin parmak izi aktarım öncesi ve sonrası karşılaştırılır. Donmuş kaynak zincirini aktarım boyunca başka araçlarla değiştirmeyin.

Hata halinde yeni gönderimler durur. Bütün yerel yazıcı ve doğrulama işçileri bitmeden işlev dönmez; sahipsiz arka plan hedef yazması bırakılmaz. Çekirdekte bloke olmuş G/Ç beklemeyi uzatabilir. Zaman aşımı alttaki işlemin iptal edildiği anlamına gelmez. Yarım hedef ve tanı kayıtları korunur.

## İşlenebilir kayıtlar

`final-ranges-0.verified.jsonl` gibi doğrulama dosyaları şu alanları taşır:

- `offset`, `length`, `sha256`: doğrulanan mantıksal bölge ve ilk kaynak özeti.
- `attempt`: ilk aktarım için 0, başarılı tekrar için 1.
- `verification`: `direct` veya `buffered`.

Yan dosyalar `final-ranges-0.verified.metrics.jsonl` ve `final-ranges-0.verified.retries.jsonl` biçiminde adlandırılır. Ölçümler planlı/doğrulanmış baytları, okuma isteklerini, en yüksek bekleyen istek/tampon kullanımını, derinliği, tekrar sayısını ve aşama sürelerini kaydeder. Saniyelik kayıtlarda yerel baskı, gönderim gecikmesi ve kaynak bayt sayısı da bulunur. Sürelerin `*_us` alanları birikimli mikrosaniyedir; eşzamanlı okumalar örtüştüğünden toplamları gerçek geçen süreye eşit değildir. `elapsed_ms` duvar saati süresidir.

Bu kayıtlar otomatik kaldığı yerden sürdürme (resume) sağlamaz. İş başlatılırken mevcut kaynak snapshot zinciri reddedilir. Uygulamanın oluşturduğu snapshotlar başarı sonunda da saklanır. Son aşamada bağımsız tam disk hash taraması otomatik yapılmaz. Hedefin açılması ve uygulama hizmetinin sağlıklı olması ayrı denetimlerdir.
