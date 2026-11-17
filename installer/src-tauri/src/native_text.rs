//! Texts shown by setup *before* its window exists (plain Windows message boxes), in the
//! user's Windows display language. The window itself picks its language in JavaScript; this
//! follows the same rule, so both always agree.

/// The five languages the setup window supports.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Language {
    Korean,
    English,
    SimplifiedChinese,
    TraditionalChinese,
    Japanese,
}

/// Picks the language for a Windows locale name such as `ko-KR`, `zh-Hant-TW` or `en-US`.
/// Same rule as `pickLanguage` in the setup window: Traditional Chinese for Taiwan, Hong Kong,
/// Macau and `Hant`; Simplified for other Chinese; English for anything unsupported.
pub fn pick_language(locale: &str) -> Language {
    let wanted = locale.to_ascii_lowercase();
    if wanted.starts_with("zh") {
        let traditional = ["tw", "hk", "mo", "hant"]
            .iter()
            .any(|part| wanted.split(['-', '_']).any(|piece| piece == *part));
        return if traditional {
            Language::TraditionalChinese
        } else {
            Language::SimplifiedChinese
        };
    }
    match wanted.get(..2) {
        Some("ko") => Language::Korean,
        Some("ja") => Language::Japanese,
        _ => Language::English,
    }
}

/// Title and question of the "WebView2 is missing" message box. `log` is this run's log file,
/// mentioned so the cause can be found later.
pub fn missing_webview(language: Language, log: Option<&str>) -> (&'static str, String) {
    let (title, body, log_label) = match language {
        Language::Korean => (
            "Prism 설치",
            "Prism을 실행하려면 Microsoft Edge WebView2가 필요한데 이 PC에 없습니다.\n\n지금 다운로드 페이지를 열까요?",
            "설치 기록",
        ),
        Language::English => (
            "Prism setup",
            "Prism needs Microsoft Edge WebView2, which is missing on this PC.\n\nOpen the download page now?",
            "Setup log",
        ),
        Language::SimplifiedChinese => (
            "Prism 安装",
            "Prism 需要 Microsoft Edge WebView2，但此电脑上没有安装。\n\n现在打开下载页面吗？",
            "安装日志",
        ),
        Language::TraditionalChinese => (
            "Prism 安裝",
            "Prism 需要 Microsoft Edge WebView2，但這台電腦上沒有安裝。\n\n現在開啟下載頁面嗎？",
            "安裝記錄",
        ),
        Language::Japanese => (
            "Prism セットアップ",
            "Prism には Microsoft Edge WebView2 が必要ですが、この PC にはありません。\n\n今すぐダウンロード ページを開きますか？",
            "セットアップ ログ",
        ),
    };
    let text = match log {
        Some(path) => format!("{body}\n\n{log_label}: {path}"),
        None => body.to_owned(),
    };
    (title, text)
}

/// The user's Windows display language as a locale name (for example `ko-KR`), or "".
#[cfg(windows)]
pub fn user_locale() -> String {
    use windows::Win32::Globalization::GetUserDefaultLocaleName;
    // LOCALE_NAME_MAX_LENGTH is 85 characters including the terminating zero.
    let mut buffer = [0u16; 85];
    // SAFETY: the buffer is writable and its length is passed with it.
    let length = unsafe { GetUserDefaultLocaleName(&mut buffer) };
    if length <= 1 {
        return String::new();
    }
    String::from_utf16_lossy(&buffer[..length as usize - 1])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn languages_follow_the_setup_window_rule() {
        assert_eq!(pick_language("ko-KR"), Language::Korean);
        assert_eq!(pick_language("ja-JP"), Language::Japanese);
        assert_eq!(pick_language("en-US"), Language::English);
        assert_eq!(pick_language("de-DE"), Language::English);
        assert_eq!(pick_language(""), Language::English);
        assert_eq!(pick_language("zh-CN"), Language::SimplifiedChinese);
        assert_eq!(pick_language("zh-SG"), Language::SimplifiedChinese);
        assert_eq!(pick_language("zh-TW"), Language::TraditionalChinese);
        assert_eq!(pick_language("zh-HK"), Language::TraditionalChinese);
        assert_eq!(pick_language("zh-Hant-MO"), Language::TraditionalChinese);
        // "tw" must be a whole part, not part of another word.
        assert_eq!(pick_language("zh-Hans-CN"), Language::SimplifiedChinese);
    }

    #[test]
    fn the_message_names_the_log_when_there_is_one() {
        let (title, text) = missing_webview(Language::Korean, Some(r"C:\logs\setup.log"));
        assert_eq!(title, "Prism 설치");
        assert!(text.contains("WebView2") && text.ends_with(r"설치 기록: C:\logs\setup.log"));
        let (_, plain) = missing_webview(Language::English, None);
        assert!(!plain.contains("Setup log"));
    }
}
