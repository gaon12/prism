//! Read-only diagnostic: no elevation, service mutation, account enumeration,
//! path/identity output or filesystem writes.
fn main() {
    #[cfg(windows)]
    match prism_ntfs_windows::administration::check_install_location() {
        Ok(()) => println!("{{\"ready\":true}}"),
        Err(error) => {
            println!("{{\"ready\":false,\"kind\":\"{:?}\"}}", error.kind());
            std::process::exit(2);
        }
    }
    #[cfg(not(windows))]
    {
        println!("{{\"ready\":false,\"kind\":\"Unsupported\"}}");
        std::process::exit(2);
    }
}
