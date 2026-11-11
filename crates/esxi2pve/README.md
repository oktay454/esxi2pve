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

Delta transfers use bounded adaptive asynchronous NBD reads and durable SHA-256 verification. Limits: `--io-max-depth 1..64` and `--io-memory-mib 16..1024`, defaulting to 16 and 128 MiB. Direct readback is the default; explicit `--verify-read buffered` records a warning. [Transfer engine](https://github.com/oktay454/esxi2pve/blob/main/docs/architecture.md).

## License

GNU GPL version 3 or later (`GPL-3.0-or-later`). See [LICENSE](LICENSE).

## Job names and terminal output

Job directories use a UTC start time and VM IDs, for example:

```text
/var/lib/esxi2pve/2026-10-24T103000.123456789Z_esxi-72_pve-108281_p3832933/
```

The date/time is UTC (Z), followed by the source ESXi VMID and target PVE VMID. Names are independent of the selected language. Nine-digit nanoseconds and the process ID (PID) are always present; the format is fixed. Directories are allocated exclusively; an existing name is an error. Existing jobs are not renamed or overwritten. The event log uses the same job name with a .jsonl suffix. The source host and all job metadata remain available in state.json.

Terminal messages use the PVE node's local time with a UTC offset, localized phase/level labels, concise command descriptions and elapsed command durations. Progress percentages are shown at most once every two seconds, plus the first and last record. --verbose shows full command arguments, command result paths and every progress event.

GNU date from Debian coreutils resolves the system timezone or TZ; the offset is cached for the current minute. If date is unavailable or fails, terminal timestamps explicitly fall back to UTC (Z). Formatting does not invoke a process for every event.

JSONL event fields, raw phase identifiers, epoch timestamps and commands/*.argv.json, stdout, stderr and result.json files retain their formats and complete records. Display throttling never drops persistent progress records. Terminal messages go to stderr. Final and status results are human-readable on a terminal; --json forces JSON, and redirected/piped stdout automatically remains JSON. status can read both old and new job directories and reports saved state, not live VM status.
