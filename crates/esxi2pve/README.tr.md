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

## Lisans

GNU GPL sürüm 3 veya daha yeni bir sürüm (`GPL-3.0-or-later`). Tam metin: [LICENSE](LICENSE).
