//! Test the production setup modules with fake system integration and temporary files.
pub mod args;
pub mod folder_lock;
pub mod install;
pub mod operation;
pub mod payload;
pub mod safe_path;
pub mod system;
pub mod temp_cleanup;
pub mod transaction;

#[cfg(test)]
mod install_regression;
