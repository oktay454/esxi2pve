# esxi2pve

[English](https://github.com/oktay454/esxi2pve/blob/main/crates/esxi2pve/README.md) | [Türkçe](https://github.com/oktay454/esxi2pve/blob/main/crates/esxi2pve/README.tr.md)

A Rust command-line tool for migrating virtual machines from VMware ESXi to Proxmox VE, powered by `libesxi2pve`.

Supports `offline`, `live-clone`, `snapshot-iterative` and `live-mirror` migrations, persistent command logs, verified disk transfers and Turkish/English messages.

## Installation

After publication:

```console
cargo install esxi2pve --locked
```

Run as root on the target PVE node. External tools are required: `ssh`, `sshfs`, `qemu-img`, `qemu-nbd`, `qm`, `pvesm`, `fusermount3`, `findmnt` and `kill`. ESXi must provide SSH/SFTP and its native VM/disk management tools. Verify the SSH host key in `known_hosts` first. Existing SSH keys are tried before prompting for a password.

## Usage

```console
esxi2pve plan --method snapshot-iterative \
	--esxi 192.0.2.10 --esxi-vmid 12 --vmid 212 \
	--storage local-lvm --bridge vmbr0 --iterations 0

esxi2pve migrate --method snapshot-iterative \
	--esxi 192.0.2.10 --esxi-vmid 12 --vmid 212 \
	--storage local-lvm --bridge vmbr0 --iterations 0
```

The migration prompts before source shutdown. Without `--start`, the target remains powered off. A TCP health check is required when using `--start`. Live Mirror also requires separate local staging storage.

Select messages using `ESXI2PVE_LANG > LC_ALL > LC_MESSAGES > LANG`. Turkish locales select Turkish; other values fall back to English. For example: `ESXI2PVE_LANG=en esxi2pve --help`.

Logs are stored under `/var/log/esxi2pve` and job state under `/var/lib/esxi2pve`. On failure, inspect `RECOVERY.txt`, `state.json` and command output. Do not start source and target simultaneously. No automatic resume or source restart is performed.

See the [full operational guide](https://github.com/oktay454/esxi2pve/blob/main/README.md) for compatibility limits, storage requirements, static builds and recovery procedures.

## License

GNU GPL version 3 or later (`GPL-3.0-or-later`). See [LICENSE](LICENSE).
