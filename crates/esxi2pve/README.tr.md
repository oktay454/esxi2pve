# esxi2pve

[Türkçe](https://github.com/oktay454/esxi2pve/blob/main/crates/esxi2pve/README.tr.md) | [English](https://github.com/oktay454/esxi2pve/blob/main/crates/esxi2pve/README.md)

VMware ESXi sanal makinelerini Proxmox VE ortamına aktaran Rust komut satırı aracı (CLI). `libesxi2pve` kütüphanesini kullanır.

`offline`, `live-clone`, `snapshot-iterative` ve `live-mirror` yöntemlerini, kalıcı komut günlüklerini, doğrulamalı disk aktarımını ve Türkçe/İngilizce metinleri destekler.

## Kurulum

Yayımlandıktan sonra:

```console
cargo install esxi2pve --locked
```

Hedef PVE düğümünde root olarak çalıştırın. Harici araçlar gerekir: `ssh`, `sshfs`, `qemu-img`, `qemu-nbd`, `qm`, `pvesm`, `fusermount3`, `findmnt`, `kill`. ESXi üzerinde SSH/SFTP ve yerleşik VM/disk yönetim araçları bulunmalıdır. Önce SSH sunucu anahtarını doğrulayıp `known_hosts` içine ekleyin. Mevcut SSH anahtarı denenir; başarısızsa terminalde parola sorulur.

## Kullanım

```console
esxi2pve plan --method snapshot-iterative \
	--esxi 192.0.2.10 --esxi-vmid 12 --vmid 212 \
	--storage local-lvm --bridge vmbr0 --iterations 0

esxi2pve migrate --method snapshot-iterative \
	--esxi 192.0.2.10 --esxi-vmid 12 --vmid 212 \
	--storage local-lvm --bridge vmbr0 --iterations 0
```

Kaynak kapatılmadan önce onay istenir. `--start` olmadan hedef kapalı bırakılır. `--start` ile TCP sağlık denetimi zorunludur. Live Mirror ayrıca ayrı yerel geçici depolama gerektirir.

Dil `ESXI2PVE_LANG > LC_ALL > LC_MESSAGES > LANG` sırasıyla seçilir. Türkçe yerel ayarlar Türkçe, diğer değerler İngilizce seçer. Örnek: `ESXI2PVE_LANG=tr esxi2pve --help`.

Günlükler `/var/log/esxi2pve`, iş durumu `/var/lib/esxi2pve` altında tutulur. Hata halinde `RECOVERY.txt`, `state.json` ve komut çıktılarını inceleyin. Kaynak ve hedefi aynı anda açmayın. Otomatik sürdürme veya kaynağı yeniden başlatma uygulanmaz.

Uyumluluk sınırları, depolama gereksinimleri, sabit derleme ve kurtarma adımları için [tam kullanım belgesini](https://github.com/oktay454/esxi2pve/blob/main/README.tr.md) okuyun.

Fark aktarımı uyarlanabilir, sınırlı eşzamansız NBD okumaları ve kalıcı SHA-256 doğrulaması kullanır. Sınırlar: `--io-max-depth 1..64`, `--io-memory-mib 16..1024`. Varsayılanlar 16 ve 128 MiB'dir. Doğrudan geri okuma varsayılandır; `--verify-read buffered` açık seçimi uyarı yazar. [Aktarım motoru](https://github.com/oktay454/esxi2pve/blob/main/docs/architecture.tr.md).

## Lisans

GNU GPL sürüm 3 veya daha yeni bir sürüm (`GPL-3.0-or-later`). Tam metin: [LICENSE](LICENSE).

## İş adları ve terminal çıktısı

İş dizinleri UTC başlangıç zamanı ve VM kimlikleriyle adlandırılır. Örnek:

```text
/var/lib/esxi2pve/2026-10-24T103000.123456789Z_esxi-72_pve-108281_p3832933/
```

Tarih-saat UTC'dir (Z); ardından kaynak ESXi VMID ve hedef PVE VMID gelir. Ad seçilen dilden bağımsızdır. Dokuz basamaklı nanosaniye ve süreç kimliği (PID) her adda bulunur; biçim sabittir. Dizinler atomik olarak ayrılır; aynı ad varsa hata verilir. Eski işler yeniden adlandırılmaz veya üzerlerine yazılmaz. Olay günlüğü aynı iş adı ve .jsonl uzantısını kullanır. Kaynak sunucu ve diğer iş bilgileri state.json içinde bulunur.

Terminal iletileri PVE düğümünün yerel saatini UTC ofsetiyle, çevrilmiş aşama/seviye adlarını, kısa komut açıklamalarını ve geçen komut süresini gösterir. İlerleme yüzdesi ilk ve son kayıtta, arada en sık iki saniyede bir gösterilir. --verbose tam komut argümanlarını, sonuç dosyası yollarını ve her ilerleme olayını gösterir.

Debian coreutils içindeki GNU date, sistem saat dilimini veya TZ değerini çözümler; UTC ofseti bulunduğu dakika için önbelleğe alınır. date yoksa veya başarısızsa terminal saatleri açıkça UTC'ye (Z) döner. Her olay için ayrı süreç başlatılmaz.

JSONL olay alanları, ham aşama kimlikleri, epoch zamanları ve commands/*.argv.json, stdout, stderr, result.json dosyaları biçimlerini ve tüm kayıtları korur. Ekran ilerlemesini seyrekleştirmek kalıcı günlükten kayıt eksiltmez. Terminal iletileri stderr'e yazılır. Sonuç ve status çıktısı terminalde insan okunurdur; --json JSON'u zorlar, stdout dosyaya/boruya yönlendirilince otomatik JSON kalır. status eski ve yeni iş dizinlerini okuyabilir; canlı VM durumu yerine kayıtlı durumu gösterir.
