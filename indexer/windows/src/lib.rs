//! Platform adapters for the optional NTFS service. No startup action installs
//! a service, changes privileges, writes a volume or launches another process.
#[cfg(windows)]
pub mod administration;
#[cfg(windows)]
pub mod client;
#[cfg(windows)]
pub mod host;
#[cfg(windows)]
mod local_accounts;
#[cfg(windows)]
pub mod pipe;
#[cfg(windows)]
pub mod scm;
#[cfg(windows)]
mod security;
#[cfg(windows)]
pub mod service;
#[cfg(windows)]
pub mod volume;
