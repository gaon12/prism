// QWERTY / modern Korean two-set physical keys, not romanization or edit-distance matching.
// Layout cross-check: https://github.com/libhangul/libhangul/blob/main/hangul/hangulkeyboard.h
const LEADS: [&str; 19] = [
    "r", "R", "s", "e", "E", "f", "a", "q", "Q", "t", "T", "d", "w", "W", "c", "z", "x", "v", "g",
];
const VOWELS: [&str; 21] = [
    "k", "o", "i", "O", "j", "p", "u", "P", "h", "hk", "ho", "hl", "y", "n", "nj", "np", "nl", "b",
    "m", "ml", "l",
];
const FINALS: [&str; 28] = [
    "", "r", "R", "rt", "s", "sw", "sg", "e", "f", "fr", "fa", "fq", "ft", "fx", "fv", "fg", "a",
    "q", "qt", "t", "T", "d", "w", "c", "z", "x", "v", "g",
];
const CONSONANTS: [&str; 30] = [
    "r", "R", "rt", "s", "sw", "sg", "e", "E", "f", "fr", "fa", "fq", "ft", "fx", "fv", "fg", "a",
    "q", "Q", "qt", "t", "T", "d", "w", "W", "c", "z", "x", "v", "g",
];

pub fn keyboard_projection(text: &str) -> Option<String> {
    let mut result = String::with_capacity(text.len());
    let mut converted = false;
    for ch in text.chars() {
        let code = ch as u32;
        let keys = match code {
            0xac00..=0xd7a3 => {
                let offset = (code - 0xac00) as usize;
                result.push_str(LEADS[offset / (21 * 28)]);
                result.push_str(VOWELS[(offset / 28) % 21]);
                result.push_str(FINALS[offset % 28]);
                converted = true;
                continue;
            }
            0x1100..=0x1112 => Some(LEADS[(code - 0x1100) as usize]),
            0x1161..=0x1175 => Some(VOWELS[(code - 0x1161) as usize]),
            0x11a8..=0x11c2 => Some(FINALS[(code - 0x11a7) as usize]),
            0x3131..=0x314e => Some(CONSONANTS[(code - 0x3131) as usize]),
            0x314f..=0x3163 => Some(VOWELS[(code - 0x314f) as usize]),
            _ => None,
        };
        if let Some(keys) = keys {
            result.push_str(keys);
            converted = true;
        } else {
            result.push(ch);
        }
    }
    converted.then_some(result)
}

pub struct KeyboardPattern {
    keys: String,
    from_korean: bool,
}
impl KeyboardPattern {
    pub fn new(text: &str) -> Option<Self> {
        // Full paths and syntax remain literal; only filename-like terms get a fallback.
        if text.chars().any(|ch| {
            ch.is_ascii() && !ch.is_ascii_alphanumeric() && !matches!(ch, ' ' | '.' | '_' | '-')
        }) {
            return None;
        }
        let korean = keyboard_projection(text);
        let from_korean = korean.is_some();
        let keys = if let Some(keys) = korean {
            keys.to_ascii_lowercase()
        } else {
            text.chars()
                .map(|ch| {
                    if "REQTWOP".contains(ch) {
                        ch
                    } else {
                        ch.to_ascii_lowercase()
                    }
                })
                .collect()
        };
        if !keys.is_ascii() || keys.chars().filter(char::is_ascii_alphabetic).count() < 2 {
            return None;
        }
        Some(Self { keys, from_korean })
    }
    pub fn matches(&self, literal: &str, keyboard: Option<&str>) -> bool {
        if self.from_korean {
            literal.contains(&self.keys)
        } else {
            keyboard.is_some_and(|text| text.contains(&self.keys))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn maps_compound_jamo_and_preserves_shift_distinctions() {
        assert_eq!(keyboard_projection("공문서").as_deref(), Some("rhdanstj"));
        assert_eq!(keyboard_projection("회의").as_deref(), Some("ghldml"));
        assert_eq!(keyboard_projection("괜찮아").as_deref(), Some("rhoscksgdk"));
        assert_eq!(
            keyboard_projection("까 쌀 얘 예").as_deref(),
            Some("Rk Tkf dO dP")
        );
        assert_eq!(keyboard_projection("공문서").as_deref(), Some("rhdanstj"));
        assert!(KeyboardPattern::new("Tkf")
            .unwrap()
            .matches("쌀", keyboard_projection("쌀").as_deref()));
        assert!(!KeyboardPattern::new("tkf")
            .unwrap()
            .matches("쌀", keyboard_projection("쌀").as_deref()));
        assert!(KeyboardPattern::new("ㅖㄱㅑ느")
            .unwrap()
            .matches("prism-guide.txt", None));
        for input in [
            "1",
            "r",
            "C:/docs",
            "edit-date:2024",
            "https://example.com",
            "漢字",
        ] {
            assert!(KeyboardPattern::new(input).is_none(), "{input}");
        }
    }
}
