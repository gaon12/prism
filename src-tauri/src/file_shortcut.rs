//! Validate the configurable in-app file opening key before settings are persisted.
//! Keep aliases and reservations in sync with src/lib/file-shortcut.ts (shared fixtures).

pub(super) fn canonical(input: &str, mac: bool) -> Result<String, &'static str> {
    if input.is_empty() || input.len() > 64 || !input.bytes().all(|b| (32..=126).contains(&b)) {
        return Err("fileShortcutInvalid");
    }
    let mut parts: Vec<_> = input
        .split('+')
        .map(|p| p.trim().to_ascii_lowercase())
        .collect();
    let key = parts.pop().ok_or("fileShortcutInvalid")?;
    let mut mods = [false; 4]; // Ctrl, Alt, Shift, Super, in canonical order.
    for part in parts {
        let index = match part.as_str() {
            "ctrl" | "control" => 0,
            "alt" | "option" => 1,
            "shift" => 2,
            "super" | "meta" | "cmd" | "command" => 3,
            "cmdorctrl" | "commandorcontrol" | "commandorctrl" | "cmdorcontrol" => {
                if mac {
                    3
                } else {
                    0
                }
            }
            _ => return Err("fileShortcutInvalid"),
        };
        if mods[index] {
            return Err("fileShortcutInvalid");
        }
        mods[index] = true;
    }
    let key = match key.as_str() {
        "enter" | "return" | "numpadenter" => "Enter".to_string(),
        "space" => "Space".to_string(),
        value => {
            let letter = value.strip_prefix("key").unwrap_or(value);
            let digit = value.strip_prefix("digit").unwrap_or(value);
            if letter.len() == 1 && letter.as_bytes()[0].is_ascii_lowercase() {
                format!("Key{}", letter.to_ascii_uppercase())
            } else if digit.len() == 1 && digit.as_bytes()[0].is_ascii_digit() {
                format!("Digit{digit}")
            } else if value
                .strip_prefix('f')
                .and_then(|n| n.parse::<u8>().ok())
                .is_some_and(|n| (1..=24).contains(&n) && value == format!("f{n}"))
            {
                value.to_ascii_uppercase()
            } else {
                return Err("fileShortcutInvalid");
            }
        }
    };
    if (!mods[0] && !mods[1] && !mods[3] && key != "Enter" && !key.starts_with('F'))
        || ((mods[0] || mods[3]) && key == "Enter")
        || (mods[1] && key == "F4")
        || ((mods[0] || mods[3]) && matches!(key.as_str(), "KeyQ" | "KeyW"))
    {
        return Err("fileShortcutReserved");
    }
    let mut tokens: Vec<_> = ["Ctrl", "Alt", "Shift", "Super"]
        .into_iter()
        .zip(mods)
        .filter_map(|(name, on)| on.then_some(name))
        .collect();
    tokens.push(&key);
    Ok(tokens.join("+"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_file_shortcut_contract() {
        let cases: serde_json::Value =
            serde_json::from_str(include_str!("../../tests/fixtures/file-shortcuts.json")).unwrap();
        for case in cases.as_array().unwrap() {
            let result = canonical(
                case["input"].as_str().unwrap(),
                case["mac"].as_bool().unwrap_or(false),
            );
            if let Some(expected) = case["canonical"].as_str() {
                assert_eq!(result, Ok(expected.to_string()), "{case}");
            } else {
                assert_eq!(result, Err(case["error"].as_str().unwrap()), "{case}");
            }
        }
    }
}
