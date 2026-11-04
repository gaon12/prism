#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    if let Some(code) = prism_lib::run_runtime_supervisor() {
        std::process::exit(code);
    }
    prism_lib::run();
}
