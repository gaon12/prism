//! "Is this computer running on battery right now?"
//!
//! Used by the optional "pause content indexing on battery" setting. Every platform answers
//! `Some(true)` (on battery), `Some(false)` (plugged in, or a desktop without a battery) or
//! `None` (unknown). Callers must treat `None` like "plugged in", so a machine whose power
//! state cannot be read never stops indexing by surprise.

/// Asks the operating system once. Cheap on Windows and Linux; on macOS it runs `pmset`,
/// so callers should cache the answer for a while (the content worker checks every 30 s).
pub fn on_battery() -> Option<bool> {
    imp::on_battery()
}

#[cfg(windows)]
mod imp {
    use windows_sys::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};

    pub fn on_battery() -> Option<bool> {
        // SAFETY: the struct is plain data and is fully written by the call on success.
        let mut status: SYSTEM_POWER_STATUS = unsafe { std::mem::zeroed() };
        if unsafe { GetSystemPowerStatus(&mut status) } == 0 {
            return None;
        }
        super::from_windows(status.ACLineStatus, status.BatteryFlag)
    }
}

#[cfg(target_os = "macos")]
mod imp {
    pub fn on_battery() -> Option<bool> {
        // `pmset -g batt` prints "Now drawing from 'AC Power'" or "... 'Battery Power'".
        let output = std::process::Command::new("/usr/bin/pmset")
            .args(["-g", "batt"])
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        super::from_pmset(&String::from_utf8_lossy(&output.stdout))
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
mod imp {
    use std::fs;

    pub fn on_battery() -> Option<bool> {
        // Each power source is a folder with a `type` file and, for chargers, `online`.
        let mut supplies = Vec::new();
        for entry in fs::read_dir("/sys/class/power_supply").ok()?.flatten() {
            let read = |name: &str| {
                fs::read_to_string(entry.path().join(name))
                    .map(|text| text.trim().to_string())
                    .ok()
            };
            let Some(kind) = read("type") else { continue };
            supplies.push(super::Supply {
                kind,
                online: read("online"),
                status: read("status"),
            });
        }
        super::from_linux(&supplies)
    }
}

#[cfg(not(any(windows, unix)))]
mod imp {
    pub fn on_battery() -> Option<bool> {
        None
    }
}

/// Windows `SYSTEM_POWER_STATUS`: AC line 0 = offline, 1 = online, 255 = unknown.
/// Battery flag 128 means "no system battery" (a desktop), which is never "on battery".
#[cfg_attr(not(windows), allow(dead_code))]
fn from_windows(ac_line: u8, battery_flag: u8) -> Option<bool> {
    if battery_flag == 128 {
        return Some(false);
    }
    match ac_line {
        0 => Some(true),
        1 => Some(false),
        _ => None,
    }
}

/// Reads the first line of `pmset -g batt`.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn from_pmset(text: &str) -> Option<bool> {
    let first = text.lines().next()?;
    if first.contains("'Battery Power'") {
        Some(true)
    } else if first.contains("'AC Power'") || first.contains("'UPS Power'") {
        Some(false)
    } else {
        None
    }
}

/// One entry under `/sys/class/power_supply`.
#[cfg_attr(any(windows, target_os = "macos"), allow(dead_code))]
struct Supply {
    /// "Mains", "Battery", "USB", ...
    kind: String,
    /// "1" when a charger is connected (chargers only).
    online: Option<String>,
    /// "Discharging", "Charging", "Full", ... (batteries only).
    status: Option<String>,
}

/// Linux rules: any connected charger means "plugged in"; otherwise a discharging battery
/// means "on battery"; with no battery at all it is a desktop.
#[cfg_attr(any(windows, target_os = "macos"), allow(dead_code))]
fn from_linux(supplies: &[Supply]) -> Option<bool> {
    let charger_online = supplies
        .iter()
        .any(|supply| supply.kind != "Battery" && supply.online.as_deref() == Some("1"));
    if charger_online {
        return Some(false);
    }
    let batteries: Vec<&Supply> = supplies.iter().filter(|s| s.kind == "Battery").collect();
    if batteries.is_empty() {
        return Some(false);
    }
    if batteries
        .iter()
        .any(|battery| battery.status.as_deref() == Some("Discharging"))
    {
        return Some(true);
    }
    // A battery that is charging or full is plugged in even if no charger was listed.
    if batteries.iter().all(|battery| {
        matches!(
            battery.status.as_deref(),
            Some("Charging") | Some("Full") | Some("Not charging")
        )
    }) {
        return Some(false);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn supply(kind: &str, online: Option<&str>, status: Option<&str>) -> Supply {
        Supply {
            kind: kind.into(),
            online: online.map(Into::into),
            status: status.map(Into::into),
        }
    }

    #[test]
    fn windows_flags_map_to_battery_state() {
        assert_eq!(from_windows(0, 1), Some(true), "unplugged laptop");
        assert_eq!(from_windows(1, 8), Some(false), "charging laptop");
        assert_eq!(
            from_windows(0, 128),
            Some(false),
            "desktop without a battery"
        );
        assert_eq!(from_windows(255, 1), None, "unknown");
    }

    #[test]
    fn pmset_first_line_decides() {
        assert_eq!(
            from_pmset("Now drawing from 'Battery Power'\n -InternalBattery-0 80%; discharging"),
            Some(true)
        );
        assert_eq!(
            from_pmset("Now drawing from 'AC Power'\n -InternalBattery-0 100%; charged"),
            Some(false)
        );
        assert_eq!(
            from_pmset("Now drawing from 'AC Power'\n"),
            Some(false),
            "desktop Mac"
        );
        assert_eq!(from_pmset(""), None);
    }

    #[test]
    fn linux_charger_battery_and_desktop() {
        assert_eq!(
            from_linux(&[
                supply("Mains", Some("0"), None),
                supply("Battery", None, Some("Discharging")),
            ]),
            Some(true)
        );
        assert_eq!(
            from_linux(&[
                supply("Mains", Some("1"), None),
                supply("Battery", None, Some("Charging")),
            ]),
            Some(false)
        );
        assert_eq!(from_linux(&[]), Some(false), "desktop: no supplies at all");
        assert_eq!(
            from_linux(&[supply("Battery", None, Some("Full"))]),
            Some(false)
        );
        assert_eq!(
            from_linux(&[supply("Battery", None, Some("Unknown"))]),
            None
        );
    }
}
