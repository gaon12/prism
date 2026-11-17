//! Test the production setup modules with fake system integration and temporary files.
pub mod args;
pub mod folder_lock;
pub mod install;
pub mod journal;
pub mod native_text;
pub mod operation;
pub mod payload;
pub mod previous;
pub mod recovery;
pub mod safe_path;
pub mod setup_log;
pub mod system;
pub mod temp_cleanup;
pub mod transaction;
pub mod trust;

#[cfg(test)]
mod install_regression;
#[cfg(test)]
mod previous_tests;
#[cfg(test)]
mod recovery_tests;
