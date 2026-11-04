// Modern Hangul syllables follow Unicode's algorithmic decomposition (19 × 21 × 28).
// https://www.unicode.org/versions/latest/ch03.pdf — Hangul Syllable Decomposition.
const INITIALS: [char; 19] = [
    'ㄱ', 'ㄲ', 'ㄴ', 'ㄷ', 'ㄸ', 'ㄹ', 'ㅁ', 'ㅂ', 'ㅃ', 'ㅅ', 'ㅆ', 'ㅇ', 'ㅈ', 'ㅉ', 'ㅊ', 'ㅋ',
    'ㅌ', 'ㅍ', 'ㅎ',
];

fn query_initial(ch: char) -> Option<char> {
    let code = ch as u32;
    if (0x1100..=0x1112).contains(&code) {
        Some(INITIALS[(code - 0x1100) as usize])
    } else if INITIALS.contains(&ch) {
        Some(ch)
    } else {
        None
    }
}
fn syllable(ch: char) -> bool {
    ('가'..='힣').contains(&ch)
}
fn initial(ch: char) -> char {
    if syllable(ch) {
        INITIALS[((ch as u32 - 0xac00) / (21 * 28)) as usize]
    } else {
        query_initial(ch).unwrap_or(ch)
    }
}
pub fn initial_projection(text: &str) -> Option<String> {
    // ASCII paths and already-compatible jamo can reuse their original stored string.
    text.chars()
        .any(|ch| initial(ch) != ch)
        .then(|| text.chars().map(initial).collect())
}

pub struct FilenamePattern {
    text: String,
    projection: String,
    has_initial: bool,
    mixed_syllables: bool,
    characters: Vec<char>,
}
impl FilenamePattern {
    pub fn new(text: String) -> Self {
        let has_initial = text.chars().any(|ch| query_initial(ch).is_some());
        Self {
            projection: if has_initial {
                text.chars().map(initial).collect()
            } else {
                String::new()
            },
            mixed_syllables: text.chars().any(syllable),
            characters: if has_initial {
                text.chars().collect()
            } else {
                vec![]
            },
            has_initial,
            text,
        }
    }
    pub fn text(&self) -> &str {
        &self.text
    }
    pub fn literal_matches(&self, text: &str) -> bool {
        text.contains(&self.text)
    }
    pub fn matches(&self, text: &str, projection: Option<&str>) -> bool {
        if self.literal_matches(text) {
            return true;
        }
        if !self.has_initial || !projection.unwrap_or(text).contains(&self.projection) {
            return false;
        }
        if !self.mixed_syllables {
            return true;
        }
        // In a mixed query such as ㄱ문서, 문서 must remain literal; do not reduce it to ㅁㅅ.
        text.char_indices().any(|(offset, _)| {
            let mut candidate = text[offset..].chars();
            self.characters.iter().all(|&expected| {
                candidate.next().is_some_and(|actual| {
                    query_initial(expected)
                        .map_or(actual == expected, |lead| initial(actual) == lead)
                })
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn matches(query: &str, name: &str) -> bool {
        FilenamePattern::new(query.into()).matches(name, initial_projection(name).as_deref())
    }
    #[test]
    fn initials_are_contiguous_and_mixed_syllables_stay_literal() {
        for query in ["ㄱㅁㅅ", "ㄱ문서", "공ㅁ서", "ᄀᄆᄉ", "2026_ㄱㅁㅅ"] {
            assert!(matches(query, "2026_공문서.hwpx"), "{query}");
        }
        assert!(!matches("ㄱㅅ", "공문서.hwpx"));
        assert!(!matches("ㄱ문서", "김민수.hwpx"));
        assert!(matches("ㅁ서", "문서/다른.txt"));
        assert!(!matches("ㅁ서", "민수/다른.txt"));
        assert!(matches("ㅆ", "쌀.txt"));
        assert!(!matches("ㅅ", "쌀.txt"));
        assert!(matches("공문서", "공문서.hwpx"));
        assert!(!matches("ㄱ.ㅁ", "공문서.hwpx"));
    }
}
