//! Explicit-submit English Wiktionary definitions for four headword languages.
//! Construction never sends a request; call `lookup` on a blocking worker only
//! after the user submits a word. No offline fallback or input watching lives here.
use crate::dictionary_store::Definition;
use reqwest::{blocking::Client, StatusCode, Url};
use scraper::{Html, Node};
use serde::Serialize;
use serde_json::Value;
use std::{
    collections::HashSet,
    fmt,
    io::Read,
    time::{Duration, Instant},
};
use unicode_normalization::UnicodeNormalization;

const ORIGIN: &str = "https://en.wiktionary.org";
const MAX_BODY: u64 = 2 * 1024 * 1024;
const MAX_HOPS: usize = 3;
const MAX_DEFINITIONS: usize = 40;
const TIMEOUT: Duration = Duration::from_secs(8);
const USER_AGENT: &str = "Prism/0.1 (desktop dictionary; explicit user queries)";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ProviderErrorCode {
    Network,
    NotFound,
    RateLimited,
    Server,
    InvalidResponse,
    Input,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderError {
    pub code: ProviderErrorCode,
    pub message: String,
}
impl ProviderError {
    fn new(code: ProviderErrorCode, message: &str) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
    /// A missing entry, service error, or malformed response is not evidence of being offline.
    pub fn fallback_allowed(&self) -> bool {
        self.code == ProviderErrorCode::Network
    }
}
impl fmt::Display for ProviderError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}
impl std::error::Error for ProviderError {}
fn invalid_response() -> ProviderError {
    ProviderError::new(
        ProviderErrorCode::InvalidResponse,
        "Wiktionary returned an invalid response.",
    )
}
fn not_found() -> ProviderError {
    ProviderError::new(
        ProviderErrorCode::NotFound,
        "No definition was found for this word and language.",
    )
}
fn network_error() -> ProviderError {
    ProviderError::new(
        ProviderErrorCode::Network,
        "Could not reach Wiktionary within the lookup timeout.",
    )
}

#[derive(Clone)]
pub struct WiktionaryProvider {
    client: Client,
}
impl WiktionaryProvider {
    pub fn new() -> Result<Self, ProviderError> {
        let client = Client::builder()
            .https_only(true)
            .user_agent(USER_AGENT)
            .connect_timeout(Duration::from_secs(4))
            .timeout(TIMEOUT)
            // MediaWiki redirects are resolved explicitly below. Never follow an HTTP
            // Location to a different service or turn returned text into a destination.
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| invalid_response())?;
        Ok(Self { client })
    }

    /// At most three alias/redirect hops and eight seconds for the whole lookup.
    pub fn lookup(&self, language: &str, word: &str) -> Result<Vec<Definition>, ProviderError> {
        validate_language(language)?;
        let mut current = validate_word(word)?;
        let deadline = Instant::now() + TIMEOUT;
        let mut visited = HashSet::new();
        let mut hops = 0;
        loop {
            if !visited.insert(current.clone()) {
                return Err(not_found());
            }
            match self
                .fetch(definition_url(&current)?, deadline)
                .and_then(|value| parse_definitions(&value, language, &current))
            {
                Ok(definitions) => return Ok(definitions),
                Err(error) if error.code == ProviderErrorCode::NotFound => {}
                Err(error) => return Err(error),
            }
            let raw = parse_raw_resolution(&self.fetch(raw_url(&current)?, deadline)?, language)?;
            let target = if raw.title != current {
                Some(raw.title.clone())
            } else {
                raw.alias
            };
            let Some(target) = target else {
                return Err(not_found());
            };
            let mut chain = HashSet::from([current.clone()]);
            for (from, to) in &raw.redirects {
                if from == to || !chain.insert(to.clone()) || visited.contains(to) {
                    return Err(not_found());
                }
            }
            let cost = raw.redirects.len().max(1);
            if hops + cost > MAX_HOPS || visited.contains(&target) {
                return Err(not_found());
            }
            hops += cost;
            // Record intermediate redirects too; the final target is inserted next iteration.
            for (_, to) in raw.redirects {
                if to != target {
                    visited.insert(to);
                }
            }
            current = target;
        }
    }

    fn fetch(&self, url: Url, deadline: Instant) -> Result<Value, ProviderError> {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .filter(|time| !time.is_zero())
            .ok_or_else(network_error)?;
        let response = self
            .client
            .get(url)
            .timeout(remaining)
            .send()
            .map_err(|error| {
                if error.is_builder() || error.is_redirect() {
                    invalid_response()
                } else {
                    network_error()
                }
            })?;
        classify_status(response.status())?;
        if response
            .content_length()
            .is_some_and(|length| length > MAX_BODY)
        {
            return Err(invalid_response());
        }
        let mut bytes = Vec::new();
        response
            .take(MAX_BODY + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| network_error())?;
        if bytes.len() as u64 > MAX_BODY {
            return Err(invalid_response());
        }
        serde_json::from_slice(&bytes).map_err(|_| invalid_response())
    }
}

fn classify_status(status: StatusCode) -> Result<(), ProviderError> {
    if status.is_success() {
        return Ok(());
    }
    Err(match status {
        StatusCode::NOT_FOUND => not_found(),
        StatusCode::TOO_MANY_REQUESTS => ProviderError::new(
            ProviderErrorCode::RateLimited,
            "Wiktionary is rate limiting requests. Try again later.",
        ),
        _ if status.is_server_error()
            || status == StatusCode::FORBIDDEN
            || status == StatusCode::UNAUTHORIZED =>
        {
            ProviderError::new(
                ProviderErrorCode::Server,
                "Wiktionary is temporarily unavailable or denied the request.",
            )
        }
        _ => invalid_response(),
    })
}
fn validate_language(language: &str) -> Result<(), ProviderError> {
    if ["ko", "en", "zh", "ja"].contains(&language) {
        Ok(())
    } else {
        Err(ProviderError::new(
            ProviderErrorCode::Input,
            "Choose Korean, English, Chinese, or Japanese.",
        ))
    }
}
fn validate_word(word: &str) -> Result<String, ProviderError> {
    // Check before trim/NFC so invisible controls are never silently accepted.
    if word.chars().count() > 256
        || word.chars().any(|character| {
            character.is_control()
                || matches!(character, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
        })
    {
        return Err(ProviderError::new(
            ProviderErrorCode::Input,
            "A dictionary word must have at most 256 characters and no control characters.",
        ));
    }
    let word = word.trim().nfc().collect::<String>();
    if word.is_empty() || matches!(word.as_str(), "." | "..") {
        return Err(ProviderError::new(
            ProviderErrorCode::Input,
            "Enter a dictionary word.",
        ));
    }
    Ok(word)
}
fn definition_url(word: &str) -> Result<Url, ProviderError> {
    let mut url = Url::parse(&format!("{ORIGIN}/api/rest_v1/page/definition/"))
        .map_err(|_| invalid_response())?;
    url.path_segments_mut()
        .map_err(|_| invalid_response())?
        .pop_if_empty()
        .push(word);
    Ok(url)
}
fn source_url(word: &str) -> Result<String, ProviderError> {
    let mut url = Url::parse(&format!("{ORIGIN}/wiki/")).map_err(|_| invalid_response())?;
    url.path_segments_mut()
        .map_err(|_| invalid_response())?
        .pop_if_empty()
        .push(word);
    Ok(url.into())
}
fn raw_url(word: &str) -> Result<Url, ProviderError> {
    let mut url = Url::parse(&format!("{ORIGIN}/w/api.php")).map_err(|_| invalid_response())?;
    url.query_pairs_mut().extend_pairs([
        ("action", "query"),
        ("format", "json"),
        ("formatversion", "2"),
        ("prop", "revisions"),
        ("rvprop", "content"),
        ("rvslots", "main"),
        ("redirects", "1"),
        ("titles", word),
    ]);
    Ok(url)
}

fn parse_definitions(
    value: &Value,
    language: &str,
    term: &str,
) -> Result<Vec<Definition>, ProviderError> {
    let object = value.as_object().ok_or_else(invalid_response)?;
    if object.contains_key("error") || object.contains_key("type") {
        return Err(invalid_response());
    }
    let entries = object
        .get(language)
        .ok_or_else(not_found)?
        .as_array()
        .ok_or_else(invalid_response)?;
    let mut result = Vec::new();
    let mut seen = HashSet::new();
    let source = source_url(term)?;
    for entry in entries {
        let part_of_speech = entry
            .get("partOfSpeech")
            .and_then(Value::as_str)
            .ok_or_else(invalid_response)?;
        let definitions = entry
            .get("definitions")
            .and_then(Value::as_array)
            .ok_or_else(invalid_response)?;
        for definition in definitions {
            let html = definition
                .get("definition")
                .and_then(Value::as_str)
                .ok_or_else(invalid_response)?;
            let text = plain_text(html);
            if text.is_empty() || !seen.insert((part_of_speech.to_string(), text.clone())) {
                continue;
            }
            let mut examples = Vec::new();
            // Prefer structured examples so translations are retained, without duplicates.
            if let Some(parsed) = definition.get("parsedExamples").and_then(Value::as_array) {
                for example in parsed.iter().take(8) {
                    let text = example
                        .get("example")
                        .and_then(Value::as_str)
                        .map(plain_text)
                        .unwrap_or_default();
                    if text.is_empty() {
                        continue;
                    }
                    let translation = example
                        .get("translation")
                        .and_then(Value::as_str)
                        .map(plain_text)
                        .unwrap_or_default();
                    examples.push(if translation.is_empty() {
                        text
                    } else {
                        format!("{text} — {translation}")
                    });
                }
            }
            if examples.is_empty() {
                if let Some(raw) = definition.get("examples").and_then(Value::as_array) {
                    examples = raw
                        .iter()
                        .filter_map(Value::as_str)
                        .take(8)
                        .map(plain_text)
                        .filter(|text| !text.is_empty())
                        .collect();
                }
            }
            result.push(Definition {
                term: term.into(),
                language: language.into(),
                part_of_speech: plain_text(part_of_speech),
                text,
                examples,
                source_url: source.clone(),
                source_name: "English Wiktionary".into(),
                license: "CC BY-SA 4.0 and GFDL".into(),
                license_url: "https://en.wiktionary.org/wiki/Wiktionary:Copyrights".into(),
                definition_language: "en".into(),
            });
            if result.len() >= MAX_DEFINITIONS {
                return Ok(result);
            }
        }
    }
    if result.is_empty() {
        Err(not_found())
    } else {
        Ok(result)
    }
}

/// HTML is parsed as inert data: no resources, links, styles or scripts are run.
/// Preserve inline punctuation and insert boundaries around block-level elements.
fn plain_text(html: &str) -> String {
    let fragment = Html::parse_fragment(html);
    let mut text = String::new();
    let mut pending = vec![(fragment.tree.root(), false)];
    while let Some((node, exiting)) = pending.pop() {
        match node.value() {
            Node::Text(value) => text.push_str(value),
            Node::Element(element) => {
                let name = element.name();
                if matches!(name, "script" | "style" | "template" | "noscript") {
                    continue;
                }
                let block = matches!(
                    name,
                    "br" | "p"
                        | "div"
                        | "li"
                        | "ul"
                        | "ol"
                        | "dl"
                        | "dt"
                        | "dd"
                        | "table"
                        | "tr"
                        | "td"
                        | "th"
                        | "blockquote"
                        | "section"
                        | "h1"
                        | "h2"
                        | "h3"
                        | "h4"
                );
                if block {
                    text.push(' ');
                }
                if !exiting {
                    pending.push((node, true));
                    pending.extend(node.children().rev().map(|child| (child, false)));
                }
                continue;
            }
            _ => {}
        }
        if !exiting {
            pending.extend(node.children().rev().map(|child| (child, false)));
        }
    }
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

struct RawResolution {
    title: String,
    alias: Option<String>,
    redirects: Vec<(String, String)>,
}
fn parse_raw_resolution(value: &Value, language: &str) -> Result<RawResolution, ProviderError> {
    if let Some(error) = value.get("error") {
        return Err(
            match error.get("code").and_then(Value::as_str).unwrap_or("") {
                "ratelimited" => ProviderError::new(
                    ProviderErrorCode::RateLimited,
                    "Wiktionary is rate limiting requests. Try again later.",
                ),
                "maxlag" | "readonly" => ProviderError::new(
                    ProviderErrorCode::Server,
                    "Wiktionary is temporarily unavailable.",
                ),
                "badtitle" | "invalidtitle" => ProviderError::new(
                    ProviderErrorCode::Input,
                    "The word is not a valid Wiktionary title.",
                ),
                _ => invalid_response(),
            },
        );
    }
    let query = value.get("query").ok_or_else(invalid_response)?;
    let pages = query
        .get("pages")
        .and_then(Value::as_array)
        .ok_or_else(invalid_response)?;
    if pages.len() != 1 {
        return Err(invalid_response());
    }
    let page = &pages[0];
    if page.get("invalid").and_then(Value::as_bool) == Some(true) {
        return Err(ProviderError::new(
            ProviderErrorCode::Input,
            "The word is not a valid Wiktionary title.",
        ));
    }
    if page.get("missing").and_then(Value::as_bool) == Some(true)
        || page.get("ns").and_then(Value::as_i64) != Some(0)
    {
        return Err(not_found());
    }
    let title = validate_word(
        page.get("title")
            .and_then(Value::as_str)
            .ok_or_else(invalid_response)?,
    )
    .map_err(|_| invalid_response())?;
    let content = page
        .get("revisions")
        .and_then(Value::as_array)
        .and_then(|items| items.first())
        .and_then(|revision| revision.get("slots"))
        .and_then(|slots| slots.get("main"))
        .and_then(|slot| slot.get("content"))
        .and_then(Value::as_str)
        .ok_or_else(invalid_response)?;
    let mut redirects = Vec::new();
    if let Some(chain) = query.get("redirects") {
        for redirect in chain.as_array().ok_or_else(invalid_response)? {
            let from = validate_word(
                redirect
                    .get("from")
                    .and_then(Value::as_str)
                    .ok_or_else(invalid_response)?,
            )
            .map_err(|_| invalid_response())?;
            let to = validate_word(
                redirect
                    .get("to")
                    .and_then(Value::as_str)
                    .ok_or_else(invalid_response)?,
            )
            .map_err(|_| invalid_response())?;
            redirects.push((from, to));
            if redirects.len() > MAX_HOPS {
                return Err(not_found());
            }
        }
    }
    Ok(RawResolution {
        title,
        alias: alias_target(content, language),
        redirects,
    })
}

fn alias_target(wikitext: &str, language: &str) -> Option<String> {
    let heading = match language {
        "ja" => "Japanese",
        "zh" => "Chinese",
        _ => return None,
    };
    let clean = strip_comments(wikitext);
    let mut in_language = false;
    let mut section = String::new();
    for line in clean.lines() {
        if let Some(found) = level_two_heading(line) {
            if in_language {
                break;
            }
            in_language = found == heading;
            continue;
        }
        if in_language {
            section.push_str(line);
            section.push('\n');
        }
    }
    let mut offset = 0;
    for line in section.split_inclusive('\n') {
        let suffix = section[offset..].trim_start();
        offset += line.len();
        let Some(body) = suffix.strip_prefix("{{") else {
            continue;
        };
        let Some((name, arguments)) = body.split_once('|') else {
            continue;
        };
        let supported = match language {
            "ja" => {
                name.trim().eq_ignore_ascii_case("ja-see-kango")
                    || name.trim().eq_ignore_ascii_case("ja-see")
            }
            "zh" => name.trim().eq_ignore_ascii_case("zh-see"),
            _ => false,
        };
        if !supported {
            continue;
        }
        let Some(end) = arguments.find("}}") else {
            continue;
        };
        let argument = arguments[..end].split('|').next()?.trim();
        let argument = argument.strip_prefix("1=").unwrap_or(argument).trim();
        if argument
            .chars()
            .any(|value| matches!(value, '{' | '}' | '[' | ']' | '<' | '>' | '|'))
        {
            continue;
        }
        if let Ok(target) = validate_word(argument) {
            return Some(target);
        }
    }
    None
}
fn level_two_heading(line: &str) -> Option<&str> {
    let line = line.trim();
    if line.starts_with("===") || line.ends_with("===") {
        return None;
    }
    line.strip_prefix("==")?.strip_suffix("==").map(str::trim)
}
fn strip_comments(text: &str) -> String {
    let mut rest = text;
    let mut clean = String::new();
    while let Some(start) = rest.find("<!--") {
        clean.push_str(&rest[..start]);
        let Some(end) = rest[start + 4..].find("-->") else {
            return clean;
        };
        rest = &rest[start + 4 + end + 3..];
    }
    clean.push_str(rest);
    clean
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn html_text_keeps_words_and_punctuation_but_drops_executable_content() {
        assert_eq!(plain_text("<style>.x{}</style><script>steal()</script><a href='https://bad'>apple</a>, <i>fruit</i> &amp; food.<div>Next<br>line</div>end<!-- ignored -->"), "apple, fruit & food. Next line end");
        assert_eq!(
            plain_text("<span>り</span><span>んご</span><template>hidden</template>"),
            "りんご"
        );
    }
    #[test]
    fn definitions_filter_language_clean_examples_and_keep_attribution() {
        let value = serde_json::json!({
            "ja": [{"partOfSpeech":"Noun","definitions":[{"definition":"<style>.x{}</style><a>apple</a>","parsedExamples":[{"example":"<b>林檎</b>です。","translation":"It is an apple."}],"examples":["duplicate"]}]}],
            "zh": [{"partOfSpeech":"Noun","definitions":[{"definition":"wrong language"}]}]
        });
        let definitions = parse_definitions(&value, "ja", "林檎").unwrap();
        assert_eq!(definitions.len(), 1);
        let row = &definitions[0];
        assert_eq!(row.text, "apple");
        assert_eq!(row.language, "ja");
        assert_eq!(row.definition_language, "en");
        assert_eq!(row.examples, ["林檎です。 — It is an apple."]);
        assert_eq!(row.source_name, "English Wiktionary");
        assert!(row
            .source_url
            .starts_with("https://en.wiktionary.org/wiki/"));
        assert!(row.license.contains("4.0"));
        assert_eq!(
            parse_definitions(&value, "ko", "林檎").unwrap_err().code,
            ProviderErrorCode::NotFound
        );
        assert_eq!(
            parse_definitions(&serde_json::json!({"ja": {}}), "ja", "x")
                .unwrap_err()
                .code,
            ProviderErrorCode::InvalidResponse
        );
    }
    #[test]
    fn aliases_are_limited_to_the_requested_language_and_ignore_comments() {
        let text = "{{also|other}}\n==Chinese==\n{{zh-see|蘋果}}\n==Japanese==\n<!-- {{ja-see-kango|wrong}} -->\n{{ja-see-kango|林檎}}\n==Korean==\n{{ja-see-kango|wrong}}";
        assert_eq!(alias_target(text, "ja").as_deref(), Some("林檎"));
        assert_eq!(alias_target(text, "zh").as_deref(), Some("蘋果"));
        assert_eq!(alias_target(text, "ko"), None);
        assert_eq!(
            alias_target("==Japanese==\n{{ja-see-kango|{{bad}}}}", "ja"),
            None
        );
        assert_eq!(
            alias_target("== Japanese ==\n{{ja-see-kango|1=林檎}}", "ja").as_deref(),
            Some("林檎")
        );
    }
    #[test]
    fn raw_query_accepts_resolved_mediawiki_redirect_and_rejects_missing_pages() {
        let value = serde_json::json!({"query":{"redirects":[{"from":"old","to":"林檎"}],"pages":[{"ns":0,"title":"林檎","revisions":[{"slots":{"main":{"content":"==Japanese==\n# apple"}}}]}]}});
        let raw = parse_raw_resolution(&value, "ja").unwrap();
        assert_eq!(raw.title, "林檎");
        assert_eq!(raw.redirects.len(), 1);
        let missing =
            serde_json::json!({"query":{"pages":[{"ns":0,"title":"missing","missing":true}]}});
        assert_eq!(
            parse_raw_resolution(&missing, "en").err().unwrap().code,
            ProviderErrorCode::NotFound
        );
        let invalid =
            serde_json::json!({"query":{"pages":[{"ns":0,"title":"bad","invalid":true}]}});
        assert_eq!(
            parse_raw_resolution(&invalid, "en").err().unwrap().code,
            ProviderErrorCode::Input
        );
    }
    #[test]
    fn unsafe_input_stays_data_and_transport_is_the_only_offline_fallback() {
        for word in ["", " ", "..", "x\ny", "x\0y", "x\u{202e}y"] {
            assert!(validate_word(word).is_err());
        }
        assert!(validate_word(&"가".repeat(257)).is_err());
        assert!(validate_word(&"가".repeat(256)).is_ok());
        assert_eq!(validate_word("  cafe\u{301} ").unwrap(), "café");
        assert!(validate_language("fr").is_err());
        assert_eq!(
            definition_url("apple").unwrap().as_str(),
            "https://en.wiktionary.org/api/rest_v1/page/definition/apple"
        );
        assert_eq!(
            source_url("apple").unwrap(),
            "https://en.wiktionary.org/wiki/apple"
        );
        let url = definition_url("https://evil.example/x?y=z#fragment").unwrap();
        assert_eq!(url.host_str(), Some("en.wiktionary.org"));
        assert!(url.query().is_none() && url.fragment().is_none());
        assert!(url.path().contains("%2F"));
        let raw = raw_url("x&action=delete").unwrap();
        assert_eq!(
            raw.query_pairs()
                .find(|(key, _)| key == "action")
                .unwrap()
                .1,
            "query"
        );
        assert_eq!(
            raw.query_pairs()
                .find(|(key, _)| key == "titles")
                .unwrap()
                .1,
            "x&action=delete"
        );
        assert!(network_error().fallback_allowed());
        for status in [404, 429, 500, 403, 301] {
            assert!(!classify_status(StatusCode::from_u16(status).unwrap())
                .unwrap_err()
                .fallback_allowed());
        }
    }
    #[test]
    #[ignore = "Set PRISM_TEST_DICTIONARY_ONLINE=1 to query live Wiktionary"]
    fn real_four_language_lookup_and_cjk_aliases() {
        if std::env::var("PRISM_TEST_DICTIONARY_ONLINE").as_deref() != Ok("1") {
            return;
        }
        let provider = WiktionaryProvider::new().unwrap();
        for (language, word) in [
            ("en", "apple"),
            ("ko", "사과"),
            ("zh", "中文"),
            ("ja", "林檎"),
            ("ja", "りんご"),
            ("zh", "苹果"),
        ] {
            let entries = provider
                .lookup(language, word)
                .unwrap_or_else(|error| panic!("{language}/{word}: {error:?}"));
            assert!(!entries.is_empty());
            assert!(entries.iter().all(|entry| entry.language == language
                && entry.definition_language == "en"
                && !entry.text.contains("<style")
                && !entry.text.contains("<script")));
            if word == "りんご" {
                assert_eq!(entries[0].term, "林檎");
            }
            if word == "苹果" {
                assert_eq!(entries[0].term, "蘋果");
            }
            println!(
                "live dictionary {language}/{word}: {} definitions, term={}",
                entries.len(),
                entries[0].term
            );
        }
    }
}
