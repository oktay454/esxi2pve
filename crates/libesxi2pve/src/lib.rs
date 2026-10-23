//! ESXi → Proxmox VE aktarım motoru. Linux/PVE üzerinde çalışır.
//! Rust dışı uygulama betiği ve üçüncü taraf Cargo bağımlılığı içermez.
#![forbid(unsafe_code)]

#[macro_use]
pub mod i18n;
pub mod config;
pub mod disk;
pub mod json;
pub mod migration;
pub mod process;
pub mod sha256;
mod transport;
pub mod vmware;

pub use config::{Config, DeltaEngine, HealthCheck, Method};
pub use i18n::Language;
pub use migration::{Migration, Report};
pub use process::{CommandExecutor, CommandOutput, CommandSpec, ProcessExecutor};

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug)]
pub struct Error(pub String);
impl std::fmt::Display for Error {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		f.write_str(&self.0)
	}
}
impl std::error::Error for Error {}
impl From<std::io::Error> for Error {
	fn from(value: std::io::Error) -> Self {
		Self(value.to_string())
	}
}
pub(crate) fn ensure(condition: bool, message: impl Into<String>) -> Result<()> {
	if condition {
		Ok(())
	} else {
		Err(Error(message.into()))
	}
}
