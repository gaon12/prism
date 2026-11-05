//! Dedicated one-shot administrator helper. The service payload is part of this
//! executable, never read from a writable path after the elevation boundary.
#![cfg_attr(windows, windows_subsystem = "windows")]

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Action {
    Install,
    Start,
    Upgrade,
    Uninstall,
}
fn action(arguments: &[String]) -> Option<Action> {
    if arguments.len() != 1 {
        return None;
    }
    match arguments[0].as_str() {
        "--install" => Some(Action::Install),
        "--start" => Some(Action::Start),
        "--upgrade" => Some(Action::Upgrade),
        "--uninstall" => Some(Action::Uninstall),
        _ => None,
    }
}
#[cfg(windows)]
const SERVICE: &[u8] = include_bytes!(env!("PRISM_INDEX_SERVICE_PAYLOAD"));

fn main() {
    let Some(action) = action(&std::env::args().skip(1).collect::<Vec<_>>()) else {
        std::process::exit(87);
    };
    #[cfg(windows)]
    {
        use prism_ntfs_windows::administration;
        let result = match action {
            Action::Install => administration::install(SERVICE),
            Action::Start => administration::start(),
            Action::Upgrade => administration::upgrade(SERVICE),
            Action::Uninstall => administration::uninstall(),
        };
        if let Err(error) = result {
            std::process::exit(administration::exit_code(&error));
        }
    }
    #[cfg(not(windows))]
    {
        let _ = action;
        std::process::exit(50);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[cfg(windows)]
    fn embeds_the_real_service_image_for_this_windows_architecture() {
        prism_ntfs_windows::administration::validate_payload(SERVICE).unwrap();
    }
    #[test]
    fn management_has_only_explicit_single_argument_actions() {
        for (argument, expected) in [
            ("--install", Action::Install),
            ("--start", Action::Start),
            ("--upgrade", Action::Upgrade),
            ("--uninstall", Action::Uninstall),
        ] {
            assert_eq!(action(&[argument.into()]), Some(expected));
            assert_eq!(action(&[argument.into(), "C:\\arbitrary.exe".into()]), None);
        }
        for arguments in [
            vec![],
            vec!["--service".into()],
            vec!["--install=C:\\source.exe".into()],
        ] {
            assert_eq!(action(&arguments), None);
        }
    }
}
