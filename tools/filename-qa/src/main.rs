fn main() {
    if prism_filename_qa::run().is_err() {
        // Input captures may contain private paths. Logs retain counts and
        // hashes, never path values or captured rows.
        eprintln!("Filename preparation benchmark failed.");
        std::process::exit(2);
    }
}
