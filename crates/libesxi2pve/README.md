# libesxi2pve

[English](README.md) | [Türkçe](README.tr.md)

A Rust library for migrating virtual machines from VMware ESXi to Proxmox VE. It provides four migration workflows, verified disk transfers, persistent logs and recovery state. The `esxi2pve` command-line application uses this library.

Requires Linux and Rust 1.89 or newer. Production operations run as root on the target PVE node. External SSH/QEMU/PVE tools remain necessary: QEMU decodes disk formats and PVE manages its own VMs and storage.

## Language selection

`Config::default().language` uses the first nonempty value from `ESXI2PVE_LANG > LC_ALL > LC_MESSAGES > LANG`. `tr`, `tr_TR.UTF-8` and `tr-TR` select Turkish. Other locales, `C`/`POSIX` and an unset environment fall back to English. The application-specific override takes precedence over all standard locale variables.

```rust,no_run
use libesxi2pve::{Config, Language};
let config = Config {
	language: Language::English,
	..Config::default()
};
```

Configuration validation and migration use `Config.language`. For standalone parsing, error or logging operations, use `libesxi2pve::i18n::with_language(Language::Turkish, || { /* operation */ })`. Language scopes are thread-local and restore the previous value after nested calls or panic unwinding. Select a language explicitly for child threads. Environment variables are only read; global process state is not modified.

Help, errors, confirmations, descriptive logs and recovery notes are translated. JSON keys, protocol strings and command identifiers remain stable. Raw external-tool and OS messages are preserved. Interactive confirmation requires `EVET` in Turkish and `YES` in English. The catalog is `src/messages.rs`, detection/scoping is `src/i18n.rs`, and CLI help resources are in the CLI package's `locales/tr/help.txt` and `locales/en/help.txt`. No Cargo dependency or installed locale package is needed.

## Using the library

```toml
[dependencies]
libesxi2pve = "0.1.2"
```

```rust,no_run
use libesxi2pve::{Config, Method, Migration};

fn main() -> libesxi2pve::Result<()> {
	let config = Config {
		method: Method::SnapshotIterative,
		esxi: "192.0.2.10".into(),
		esxi_vmid: 12,
		vmid: 212,
		storage: "local-lvm".into(),
		iterations: 0,
		execute: false, // inspect and plan; do not modify source/target VMs
		..Config::default()
	};
	let report = Migration::new(config).run()?;
	println!("{}", report.state_dir.display());
	Ok(())
}
```

`execute=true` enables changes. `yes=false` requests terminal confirmation at startup and cutover. `start=false` leaves the target powered off. `start=true` requires `Config.health` or a custom Rust health check:

```rust,ignore
let migration = Migration::new(config).with_health_check(
	std::time::Duration::from_secs(600),
	move |vmid| {
		// Run your application-specific check.
		// true: ready; false: not yet ready; Err: stop migration.
		// Each call must have its own bounded timeout.
		Ok(my_service_is_ready(vmid))
	},
);
let report = migration.run()?;
```

The second snippet is schematic: implement `my_service_is_ready` for your application. The CLI's built-in check uses Rust `TcpStream::connect_timeout`; an open TCP port does not prove database or application health.

## Modules

| Module | Responsibility |
|---|---|
| `i18n` | Locale precedence, thread-local scopes, compile-time checked message catalog |
| `config` | Migration methods and option validation |
| `migration` | Four workflows, source/VMID locks and recovery state |
| `transport` | Native SSHFS options and FUSE escaping |
| `process` | Argument-array command execution, persistent stdout/stderr and timeouts |
| `vmware` | VMX/VMDK parsing, CID chains and ESXi/PVE metadata |
| `disk` | Read-only NBD client, logical deltas, QMP and verified readback |
| `json` | QEMU/QMP JSON parsing and encoding |
| `sha256` | Streaming SHA-256 for disk consistency comparison |

`CommandExecutor` lets consumers replace the external-command boundary. Production uses `ProcessExecutor`. Tests simulate ESXi/PVE responses while actually executing QEMU disk operations.

## Building and testing

```console
cargo test --offline --all-targets
cargo clippy --offline --all-targets -- -D warnings
cargo fmt --check
cargo doc --offline --no-deps
```

Tests use `ssh`, `sshfs`, `qemu-img`, `qemu-nbd`, `qemu-io`, `sha256sum` and Debian's `ls`/`sleep` tools. They create temporary disks inside the project and do not create PVE VMs or connect to ESXi. No missing components are installed automatically. SSHFS parser tests use a local fake `ssh`; OpenSSH configuration tests use `-G -F /dev/null`, without a network connection.

For operational steps, migration methods, static CLI builds and limitations, read the CLI's [English README](https://github.com/oktay454/esxi2pve/blob/main/README.md) or [Turkish README](https://github.com/oktay454/esxi2pve/blob/main/README.tr.md). Validate migration compatibility and application health checks for the specific ESXi/PVE configuration before production use.

## License

GNU GPL version 3 or later (`GPL-3.0-or-later`). See [LICENSE](LICENSE).
