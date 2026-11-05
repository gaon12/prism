//! Platform adapters for the optional NTFS service. No startup action installs
//! a service, changes privileges, writes a volume or launches another process.
#[cfg(windows)]
pub mod volume;
