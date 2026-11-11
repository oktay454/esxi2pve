# esxi2pve CLI yönergeleri

Bu paket için önce [depo kılavuzunu](../../AGENTS.md) oku. CLI kütüphaneyi yapılandırıp çağırır; aktarım algoritması burada uygulanmaz.

## Seçenek ve çıktı düzeni

- Ayrıştırma ve sonuç sunumu `src/main.rs` içindedir. Ortak varsayılanlar ve kurallar `libesxi2pve::Config` içinde kalmalıdır.
- Yeni seçenek için ayrıştırma, eksik/geçersiz/yinelenen değer, kütüphane doğrulaması ve iki dilde yardım metnini birlikte güncelle. Sayısal birimleri ve sınırları açık belirt.
- Mevcut komutlar `plan`, `migrate`, `status`; yöntem/seçenek takma adlarını ve tam ASCII eşleşmesini koru. Türkçe görünüm için makine kimliklerini yerelleştirme.
- Yardım kaynakları `locales/tr/help.txt` ve `locales/en/help.txt` dosyalarıdır. Yeni kullanıcı metni kütüphanenin iki dilli `tr!` kataloğunda bulunmalıdır.
- Durum mesajları/ilerleme stderr'e, son sonuç stdout'a gider. Terminalde insan okunur sunum; stdout yönlendirmesinde otomatik JSON; `--json` ile zorunlu JSON davranışını koru. `--verbose`, kalıcı kaydı değiştirmeden terminal ayrıntısını artırır.
- `status` kayıtlı durumu okur; canlı VM sorgusu veya otomatik devam işlemi başlatmaz. Eski iş adları ve durum dosyalarıyla uyumluluğu koru.
- Kullanıcı girdisini çevirmeden ve büyük/küçük harfe dönüştürmeden hata/JSON çıktısına taşı. `i/İ`, `ı/I` ve birleşik Unicode yazımları korunmalı; desteklenmeyen ASCII dışı makine kimlikleri tutarlı biçimde reddedilmelidir.
- Parolayı CLI argümanına, ortama veya günlüğe ekleme; mevcut OpenSSH terminal istemi yolunu koru.

## Test ve belgeler

`src/main.rs` içindeki birim testleri seçenek doğrulamasını; `tests/locale.rs` dil önceliği, Unicode ve Türkiye testini; `tests/status.rs` eski durum dosyaları ve JSON davranışını kapsar. Testlerde CLI'yi `env!("CARGO_BIN_EXE_esxi2pve")` ile çocuk süreç olarak başlat ve dil değişkenlerini yalnız o çocuğun ortamında ayarla.

CLI testi için gerçek sunucuya bağlanma. Yardım/sürüm, geçici `state.json` veya kütüphaneye ulaşmadan reddedilecek geçersiz seçenekleri kullan. Geçerli bir `plan` çağrısının ağ ve yerel PVE keşfi yaptığını unutma.

Yeni kullanıcı davranışında paket beniokularını ve kök kullanım belgelerini Türkçe/İngilizce birlikte güncelle. Paket manifestinin `include` listesi CLI kaynaklarını, yardım çevirilerini, testleri, iki beniokuyu ve lisansı kapsamalıdır. Yönerge dosyalarının yayımlanan paketin çalışması için gerekli olduğu bir bağımlılık oluşturma.
