# esxi2pve

[English](README.md) | [Türkçe](README.tr.md)

A command-line tool for migrating virtual machines from VMware ESXi to Proxmox VE. It supports four migration methods, persistent logs, verified disk transfers and Turkish/English messages. Written in Rust and powered by `libesxi2pve`.

## Language selection

Turkish and English are supported. The first nonempty value is used, in this order:

1. `ESXI2PVE_LANG` — an explicit override for this application.
2. `LC_ALL`
3. `LC_MESSAGES`
4. `LANG`

`tr`, `tr_TR.UTF-8` and `tr-TR` select Turkish. English locales, `C`/`POSIX`, unsupported languages and an unset environment select English. Empty values are skipped. No installed system locale or translation package is required.

```console
ESXI2PVE_LANG=tr esxi2pve --help
ESXI2PVE_LANG=en esxi2pve --help
LANG=tr_TR.UTF-8 esxi2pve --help
```

Higher-priority variables still take precedence in the last example. Interactive confirmation requires `EVET` in Turkish and `YES` in English; `--yes` works identically in either language. Help, application errors, descriptive log messages and `RECOVERY.txt` use the selected language. Commands, options, JSON keys, phase identifiers and filenames remain stable. State and event records include `language: "tr"` or `"en"`. Existing records are not translated retroactively. Raw SSH/QEMU/PVE and operating-system output is preserved.

## Building from source

The repository contains two packages in one Cargo workspace:

```text
esxi2pve/
├── Cargo.toml
├── Cargo.lock
├── crates/
│   ├── libesxi2pve/   migration library
│   └── esxi2pve/      command-line application
└── README.md
```

```console
cd esxi2pve
cargo build --offline --release
./target/release/esxi2pve --help
```

For amd64 (`x86_64-unknown-linux-gnu`), `.cargo/config.toml` enables `-C target-feature=+crt-static` to produce a glibc-based static position-independent executable (static PIE). The build requires Rust 1.89 or newer, a linker and static glibc archives, normally provided by `libc6-dev` on Debian. Other targets require their own build configuration.

Run Cargo from the repository root so it reads `.cargo/config.toml`. When building from elsewhere with `--manifest-path`, also pass the repository configuration using `--config`. To inspect static linking, use `file` and `readelf`: the binary should report `static-pie linked` with neither an ELF interpreter (`INTERP`) nor shared-library dependencies (`NEEDED`).

Deploy the resulting `esxi2pve` binary to the target PVE node. No Rust runtime or separate Rust library file is required. External SSH/QEMU/PVE tools must still be available on the target. Both packages share the root lock file, build configuration and release profile. The default package is the CLI; use `--workspace` for both packages or `-p libesxi2pve` to select the library.

## Requirements

Run as root on the target PVE node, with an unused VMID, active storage and a Linux bridge.

Local tools: `ssh`, `sshfs`, `qemu-img`, `qemu-nbd`, `qm`, `pvesm`, `fusermount3`, `findmnt`, `kill`. PVE supplies its QEMU tools through `pve-qemu-kvm`; do not replace them with different QEMU packages on PVE. SSHFS uses `sshfs`, and the SSH client uses `openssh-client`. Missing tools are reported, never installed automatically.

ESXi needs SSH, `vim-cmd`, `vmkfstools`, `esxcli`, `readlink`, `stat`, `dd` and `sha256sum`. Live Clone computes block hashes using `dd | sha256sum` in ESXi's built-in shell. The operation fails if that shell does not support `pipefail`.

The SSH host key must already be verified and present in `known_hosts`. `StrictHostKeyChecking=yes` is enforced; unknown host keys are not accepted automatically. Existing key/ssh-agent authentication is tried first. If it fails, OpenSSH prompts for a password on a real terminal. The password is never put in arguments, environment variables or logs. `--known-hosts` and `--ssh-key` take absolute paths.

## Plan first

The IP addresses, IDs and storage names below are examples:

```console
esxi2pve plan --method snapshot-iterative \
	--esxi 192.0.2.10 --esxi-vmid 12 --vmid 212 \
	--storage local-lvm --bridge vmbr0 --iterations 0
```

The plan performs actual ESXi/PVE discovery and creates logs and an SSH connection. It also establishes and validates a read-only SSHFS/SFTP mount, then unmounts it on successful completion. It does not shut down the source or create snapshots, clones or a target VM. `--mode` is an alias for `--method`; `snapshot-delta` is an alias for `snapshot-iterative`.

## Base copy followed by the final delta only

```console
esxi2pve migrate --method snapshot-iterative \
	--esxi 192.0.2.10 --esxi-vmid 12 --vmid 212 \
	--storage local-lvm --bridge vmbr0 --iterations 0 \
	--start --health-tcp 192.0.2.30:443
```

Sequence:

1. Acquire the local target VMID lock, inspect the source and validate read-only SSHFS/SFTP access. Failure at this point creates neither a source lock nor a target VM. After operator confirmation, acquire the source lock and recheck source hardware.
2. Create the target VM powered off with a `migrate` lock and allocate new raw disks.
3. Request a snapshot with `vim-cmd vmsvc/snapshot.create VMID NAME DESCRIPTION 0 1`: no memory snapshot, guest filesystem quiescing requested. Verify its ID and that every disk switched to a new descriptor.
4. Clone the frozen base using `vmkfstools -i ... -d thin`. Transfer it with `qemu-img convert -n`, perform a full `qemu-img compare`, and record local SHA-256 hashes. Remove only the verified temporary clone. The source VM remains running during this stage.
5. After cutover confirmation, request graceful shutdown with `vim-cmd vmsvc/power.shutdown VMID` and poll its power state. No forced power-off or reset is used.
6. Validate the final snapshot descriptor's parent/CID chain. Use `qemu-img map` to select logical ranges containing data or explicit zeros in the top layer.
7. Export the decoded chain through `qemu-nbd --read-only`. The Rust NBD client reads those logical ranges and writes to the same guest offsets on the target. Each 64MiB batch is fsynced and read back for SHA-256 verification. The separate verification journal is also synced.
8. Attach the disks and verify again that the source is powered off. With `--start`, boot the target and check service readiness.

The final delta stage does not scan the whole disk. Consistency relies on the initial full comparison, ordered parent/CID links, and verified readback of applied deltas. This is not an independent full-disk comparison at final cutover. Correct application shutdown is also necessary.

`--iterations 0` disables online delta rounds. The default six rounds take additional snapshots while the source runs, applying each newly frozen previous layer in order. Cutover begins once an iteration is below the default 64MiB convergence threshold. Without convergence, the final delta can be large.

## Other migration methods

| Method | Sequence | Downtime and verification |
|---|---|---|
| `offline` | Graceful shutdown → full copy → full compare → configure target | Downtime covers the entire transfer; simplest workflow |
| `live-clone` | Snapshot → copy frozen clone → graceful shutdown → clone final chain → compare 64MiB SHA-256 blocks → apply changed blocks over NBD | Final cloning and complete hash scans are part of downtime |
| `snapshot-iterative` | Frozen base clone → optional online delta rounds → graceful shutdown → final logical delta | Aims for short downtime when the final delta is small; no fixed-time guarantee |
| `live-mirror` | Graceful shutdown → boot from remote read-only disk with a local qcow2 write layer → check service → live `qm move-disk` → verify QMP and persistent configuration | Boot depends on remote I/O and network speed; time until first readiness is reported |

For Offline or Live Clone, change `--method` in the example and omit `--iterations`.

Snapshot mode also offers an explicit `--delta-engine clone`: instead of a logical map, clone each frozen chain on ESXi and compare hash blocks. This costs more space, scans and time. Errors never trigger an automatic switch to this engine.

## Live Mirror

```console
esxi2pve migrate --method live-mirror \
	--esxi 192.0.2.10 --esxi-vmid 12 --vmid 212 \
	--storage local-lvm --staging-storage local \
	--start --health-tcp 192.0.2.30:443 --mirror-mib 64
```

Staging must be a different `dir` storage on an actual local ext2/3/4, XFS, Btrfs or ZFS filesystem. Free space for the full virtual disk capacity plus 2GiB is required: an overlay can fill completely in the worst case. Each remote disk is sampled using 64 random 4KiB reads and 16MiB of sequential reads. Defaults are a 25ms p95 limit and 20MiB/s sequential minimum. This sample is not a sustained-performance guarantee. A failing sample prevents target startup.

Guest writes go to the local qcow2 layer. Service readiness is checked before starting bulk mirroring. PVE's `qm move-disk --format raw --delete 0 --bwlimit ...` manages QMP mirroring/pivot and persistent VM configuration together. Bandwidth is capped at the smaller of the requested rate and half the measured sequential rate. MiB/s is converted to KiB/s for PVE. There is no fallback to live `dd` or `qemu-img convert -U` on failure.

Verify the persistent PVE disk path, active QMP device ID, raw format, absence of backing storage, and completion of block jobs. Check the service a second time, then write `mirror-local.verified`. The unused staging overlay is retained because of `--delete 0`; inspect it before removing it.

**After an incomplete Live Mirror, remote NBD/SSHFS processes and the local overlay may remain part of the running VM's disk. Do not start the source simultaneously or restart these processes or the PVE node. Inspect `RECOVERY.txt` and QMP/PVE disk configuration first.**

## Services and hardware

Without `--start`, the verified target is left powered off; the service outage is not marked as ended. Live Mirror requires `--start`. The CLI's `--health-tcp IP:PORT` tests only TCP connectivity: specify the target VM's address and port. Application-specific verification, such as a database query, can be implemented using the library's `with_health_check` Rust callback. First readiness is recorded once; continuous availability throughout mirroring is not claimed.

PVSCSI/LSI controllers map to corresponding PVE SCSI models. Other cases require explicit `--disk-bus`/`--scsihw`. BIOS/EFI and Secure Boot state are mapped. Source NVRAM, custom Secure Boot keys and TPM state are not transferred. Guests using custom keys or boot entries may need preparation. The tool does not inject guest drivers or repair bootloaders. The first sorted data disk is assumed to be the boot disk. Separate VMX snapshot and data-disk directories may cause this version to stop at chain validation.

Limits: 1..6 disks, at most one NIC. Existing snapshot chains, RDM, independent/shared disks or shared SCSI buses, encryption, vTPM and PCI passthrough are rejected. Raw-writing targets must be local files or block devices. Ceph requires krbd mapping for raw methods; librbd URIs are not mapped or written implicitly. PVE's live move may support other storage types, but Ceph has not been tested end to end.

Before snapshots, check 16GiB of free reserve per datastore. Before each clone, require the full virtual disk size plus 16GiB. The fixed reserve can still be exhausted by writes, so monitor datastore usage during migration. This check does not reserve space continuously.

## Persistent logs and recovery

```text
/var/log/esxi2pve/JOB.jsonl
/var/lib/esxi2pve/JOB/
  state.json                   atomically written phase and state
  RECOVERY.txt                 recovery notes in the job language
  resources.jsonl              snapshot/clone/volume/NBD and QMP inventory
  commands/000001.argv.json    exact argument array
  commands/000001.stdout       output written while the command runs
  commands/000001.stderr       errors and progress written while it runs
  commands/000001.result.json  exit code and timeout status
  target-0.sha256.pending      checksum progress during scanning
  final-ranges-0.verified.jsonl fsynced delta verification records
```

Times use Unix epoch seconds. `source_connection`, `source_inspection`, `source_mount`, `source_lock` and `target_creation` are separate phases. The last eight lines/8KiB of failed-command stderr are included in the error message; the full file is preserved. `RECOVERY.txt` records attempted and confirmed creation, startup, shutdown and mounting. Live Mirror dependency warnings are shown only for that method after startup without verified independence. Files use mode 0600 and new private directories use 0700. SSH password prompts are not redirected to logs. `target_create_attempted` and `target_start_attempted` are written before their respective commands.

```console
esxi2pve status /var/lib/esxi2pve/JOB
```

This reads saved `state.json`; it does not query a live VM or confirm that the job is still running. For long QEMU operations, inspect the corresponding `.stderr`; local hash scans update `.sha256.pending` and the event log. Delta verification records are synced batch by batch. There is no automatic log rotation or retention policy; archive logs only after the job finishes.

On failure, source VMs/snapshots, partial clones, target disks and required SSHFS/NBD dependencies are preserved. No automatic deletion, source restart or resume is implemented. After SIGTERM/SIGKILL or a power failure, inspect saved state and command intents. An ESXi operation can continue after the local command times out. If running under systemd, account for dependent NBD/SSH processes before configuring failure cleanup of a unit's process group.

The source `.esxi2pve.lock` is cooperative: it cannot prevent external vSphere/PVE operators or other software from changing the source. Do not perform concurrent snapshot/disk/hardware operations during migration. Source VMs and snapshots remain after success; clean them manually from the inventory once verification and the rollback period are complete.

## Compatibility

Disk layout, guest drivers, firmware and storage affect migration compatibility. VMFS/SEsparse, VMware Tools quiescing, guest boot, PVE live disk moves and Ceph require validation for the specific source and target configuration. Qualify the complete migration and application health checks with a test VM before production use. Automated disk and workflow tests do not establish end-to-end compatibility with every ESXi/PVE environment.

## Troubleshooting

`read: Connection reset by peer` can indicate an SSH/SFTP failure. Confirm that ESXi provides SFTP, that the SSH host key is verified, and that the configured account can read the VM directory. Inspect the failed command's `.stderr` and `RECOVERY.txt`.

A target VM or source `.esxi2pve.lock` left by an interrupted job is preserved. Confirm that the job has ended and inspect `qm status VMID`, `qm config VMID`, `state.json` and `resources.jsonl` before cleanup. Remove an unused empty source lock with `rmdir` only after checking that no migration still owns it. Review partial target disks and VM state before deciding whether to remove the target.

## Packages and crates.io

Both packages are published separately from this repository: `libesxi2pve` is the library and `esxi2pve` is the CLI. The CLI uses a local path and a version requirement for the library; published packages resolve the version from crates.io.

Run checks and create local package archives from the repository root:

```console
cargo test --offline --workspace --all-targets
cargo clippy --offline --workspace --all-targets -- -D warnings
cargo package --workspace --allow-dirty
```

Publish the library first and wait for it to become available before publishing the CLI:

```console
cargo publish -p libesxi2pve --dry-run
cargo publish -p libesxi2pve
cargo publish -p esxi2pve --dry-run
cargo publish -p esxi2pve
```

Publishing requires a crates.io account, authentication and ownership of the package names. A Git push does not publish crates. Inspect each archive before publishing; source files, both READMEs, the license and CLI translation resources are included.

After publication, install with `cargo install esxi2pve --locked`. For a static amd64 GNU build, use `RUSTFLAGS="-C target-feature=+crt-static" cargo install esxi2pve --locked --target x86_64-unknown-linux-gnu`; Cargo installation does not inherit this repository's `.cargo/config.toml`. External PVE/SSH/QEMU tools remain required.

## License

Licensed under GNU GPL version 3 or any later version (`GPL-3.0-or-later`). See [LICENSE](LICENSE).
