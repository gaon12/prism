//! Test the production setup modules with fake system integration and temporary files.
pub mod args;
pub mod install;
pub mod payload;
pub mod safe_path;
pub mod system;
pub mod transaction;

#[cfg(test)]
mod install_regression;
