//! Bounded Markdown preparation for the HWPX codec. Resource references are
//! displayed as text; only the caller supplies document bytes and styles.
use super::{Code, Control, Result};
use pulldown_cmark::{Alignment, Event, Options, Parser, Tag, TagEnd};
use std::collections::{HashMap, HashSet};
use std::ops::Range;

type Note = (usize, usize, Vec<String>);
fn note_cost(
    name: &str,
    notes: &HashMap<String, Note>,
    memo: &mut HashMap<String, (usize, usize)>,
    active: &mut HashSet<String>,
) -> Result<(usize, usize)> {
    if let Some(cost) = memo.get(name) {
        return Ok(*cost);
    }
    if active.len() >= 16 {
        return Err(Code::Limit);
    }
    if !active.insert(name.into()) {
        return Err(Code::Input);
    }
    let (bytes, events, refs) = notes.get(name).ok_or(Code::Input)?;
    let mut cost = (*bytes, *events);
    for reference in refs {
        let child = note_cost(reference, notes, memo, active)?;
        cost.0 = cost.0.checked_add(child.0).ok_or(Code::Limit)?;
        cost.1 = cost.1.checked_add(child.1).ok_or(Code::Limit)?;
        if cost.0 > 2 << 20 || cost.1 > 40000 {
            return Err(Code::Limit);
        }
    }
    active.remove(name);
    memo.insert(name.into(), cost);
    Ok(cost)
}
fn bound_notes(
    events: &[(Event<'_>, Range<usize>)],
    control: &Control<'_>,
) -> Result<HashSet<String>> {
    let mut notes = HashMap::new();
    let mut main_refs = Vec::new();
    let mut i = 0;
    while i < events.len() {
        control.check()?;
        match &events[i].0 {
            Event::Start(Tag::FootnoteDefinition(name)) => {
                if notes.len() >= 256 {
                    return Err(Code::Limit);
                }
                let last = end(events, i, TagEnd::FootnoteDefinition)?;
                let refs = events[i + 1..last]
                    .iter()
                    .filter_map(|(event, _)| {
                        if let Event::FootnoteReference(label) = event {
                            Some(label.to_string())
                        } else {
                            None
                        }
                    })
                    .collect();
                notes.insert(
                    name.to_string(),
                    (events[last].1.end - events[i].1.start, last - i + 1, refs),
                );
                i = last;
            }
            Event::FootnoteReference(name) => main_refs.push(name.to_string()),
            _ => {}
        }
        i += 1;
    }
    let mut total = (0usize, 0usize);
    let mut memo = HashMap::new();
    for name in main_refs {
        control.check()?;
        let cost = note_cost(&name, &notes, &mut memo, &mut HashSet::new())?;
        total.0 = total.0.checked_add(cost.0).ok_or(Code::Limit)?;
        total.1 = total.1.checked_add(cost.1).ok_or(Code::Limit)?;
        if total.0 > 2 << 20 || total.1 > 40000 {
            return Err(Code::Limit);
        }
    }
    Ok(memo.into_keys().collect())
}

pub(super) struct Slot {
    pub token: String,
    pub text: String,
}
pub(super) struct Table {
    pub token: String,
    pub alignments: Vec<Alignment>,
}
pub(super) struct Prepared {
    pub text: String,
    pub prefix: String,
    pub codes: Vec<Slot>,
    pub tables: Vec<Table>,
    pub omitted_images: usize,
    pub literal_html: usize,
}
struct Edit {
    range: Range<usize>,
    text: String,
}
fn escaped(text: &str) -> String {
    let mut result = String::new();
    for c in text.chars() {
        if c.is_ascii_punctuation() {
            result.push('\\');
        }
        result.push(if c == '\n' || c == '\r' { ' ' } else { c });
    }
    result
}
fn end(events: &[(Event<'_>, Range<usize>)], start: usize, target: TagEnd) -> Result<usize> {
    let mut depth = 0usize;
    for (i, (event, _)) in events.iter().enumerate().skip(start) {
        match event {
            Event::Start(tag) if tag.to_end() == target => depth += 1,
            Event::End(tag) if *tag == target => {
                depth = depth.checked_sub(1).ok_or(Code::Input)?;
                if depth == 0 {
                    return Ok(i);
                }
            }
            _ => {}
        }
    }
    Err(Code::Input)
}
fn caption(events: &[(Event<'_>, Range<usize>)]) -> String {
    events
        .iter()
        .filter_map(|(e, _)| match e {
            Event::Text(s) | Event::Code(s) => Some(s.as_ref()),
            _ => None,
        })
        .collect()
}
fn safe_url(url: &str) -> bool {
    let lower = url.to_ascii_lowercase();
    url.len() <= 4096
        && !url.chars().any(char::is_control)
        && ["https://", "http://", "mailto:"]
            .iter()
            .any(|s| lower.starts_with(s))
}
fn labels(language: &str) -> [&'static str; 6] {
    match language {
        "ko" => [
            "이미지 생략",
            "사실",
            "사용자 입력",
            "AI 제안",
            "빈 항목",
            "검증되지 않음",
        ],
        "ja" => [
            "画像を省略",
            "事実",
            "ユーザー入力",
            "AIの提案",
            "空欄",
            "未検証",
        ],
        "zh-CN" => [
            "图片已省略",
            "事实",
            "用户输入",
            "AI建议",
            "空白项",
            "未经核实",
        ],
        "zh-TW" => [
            "圖片已省略",
            "事實",
            "使用者輸入",
            "AI建議",
            "空白項目",
            "未經核實",
        ],
        _ => [
            "Image omitted",
            "Fact",
            "User input",
            "AI proposal",
            "Blank",
            "Claim not verified",
        ],
    }
}
pub(super) fn prepare(input: &str, language: &str, control: &Control<'_>) -> Result<Prepared> {
    let mut nonce = [0u8; 16];
    getrandom::fill(&mut nonce).map_err(|_| Code::Encode)?;
    let prefix = format!("PRISMHWPX{:032X}", u128::from_le_bytes(nonce));
    if input.contains(&prefix) {
        return Err(Code::Input);
    }
    let mut events = Vec::new();
    let mut depth = 0usize;
    let mut cells = 0usize;
    for item in Parser::new_ext(
        input,
        Options::ENABLE_TABLES
            | Options::ENABLE_FOOTNOTES
            | Options::ENABLE_STRIKETHROUGH
            | Options::ENABLE_TASKLISTS,
    )
    .into_offset_iter()
    {
        control.check()?;
        match &item.0 {
            Event::Start(tag) => {
                depth += 1;
                if depth > 32 {
                    return Err(Code::Limit);
                }
                if let Tag::Table(columns) = tag {
                    if columns.is_empty() || columns.len() > 32 {
                        return Err(Code::Limit);
                    }
                }
                if matches!(tag, Tag::TableCell) {
                    cells += 1;
                }
            }
            Event::End(_) => depth = depth.checked_sub(1).ok_or(Code::Input)?,
            _ => {}
        }
        if events.len() >= 20000 || cells > 4096 {
            return Err(Code::Limit);
        }
        events.push(item);
    }
    let used_notes = bound_notes(&events, control)?;
    let names = labels(language);
    let mut result = Prepared {
        text: String::new(),
        prefix,
        codes: Vec::new(),
        tables: Vec::new(),
        omitted_images: 0,
        literal_html: 0,
    };
    let mut edits = Vec::new();
    let mut table_stack: Vec<(usize, bool)> = Vec::new();
    let mut links = 0usize;
    let mut i = 0;
    while i < events.len() {
        control.check()?;
        let (event, range) = &events[i];
        match event {
            Event::Start(Tag::FootnoteDefinition(name)) if !used_notes.contains(name.as_ref()) => {
                let last = end(&events, i, TagEnd::FootnoteDefinition)?;
                edits.push(Edit {
                    range: range.start..events[last].1.end,
                    text: String::new(),
                });
                i = last;
            }
            Event::Start(Tag::Image { .. }) => {
                let last = end(&events, i, TagEnd::Image)?;
                let alt = caption(&events[i + 1..last]);
                edits.push(Edit {
                    range: range.start..events[last].1.end,
                    text: escaped(&format!("[{}: {alt}]", names[0])),
                });
                result.omitted_images += 1;
                i = last;
            }
            Event::Start(Tag::Link { dest_url, .. }) if !safe_url(dest_url) => {
                let last = end(&events, i, TagEnd::Link)?;
                edits.push(Edit {
                    range: range.start..events[last].1.end,
                    text: escaped(&caption(&events[i + 1..last])),
                });
                i = last;
            }
            Event::Start(Tag::Link { .. }) => links += 1,
            Event::End(TagEnd::Link) => links = links.checked_sub(1).ok_or(Code::Input)?,
            Event::Code(text) if links == 0 => {
                if result.codes.len() >= 4096 {
                    return Err(Code::Limit);
                }
                let token = format!("{}C{}END", result.prefix, result.codes.len());
                edits.push(Edit {
                    range: range.clone(),
                    text: token.clone(),
                });
                result.codes.push(Slot {
                    token,
                    text: text.to_string(),
                });
            }
            Event::Start(Tag::Table(columns)) => {
                let index = result.tables.len();
                let token = format!("{}T{index}END", result.prefix);
                result.tables.push(Table {
                    token,
                    alignments: columns.clone(),
                });
                table_stack.push((index, false));
            }
            Event::Start(Tag::TableCell) => {
                let (index, inserted) = table_stack.last_mut().ok_or(Code::Input)?;
                if !*inserted {
                    edits.push(Edit {
                        range: range.start..range.start,
                        text: result.tables[*index].token.clone(),
                    });
                    *inserted = true;
                }
            }
            Event::End(TagEnd::Table) => {
                table_stack.pop().ok_or(Code::Input)?;
            }
            Event::Html(text) | Event::InlineHtml(text) => {
                let kind = text
                    .trim()
                    .strip_prefix("<!-- prism:")
                    .and_then(|s| s.strip_suffix("; verified=false -->"));
                let name = match kind {
                    Some("fact") => Some(names[1]),
                    Some("userInput") => Some(names[2]),
                    Some("aiProposal") => Some(names[3]),
                    Some("blank") => Some(names[4]),
                    _ => None,
                };
                let replacement = if let Some(name) = name {
                    format!("\n\n**{name} · {}**\n\n", names[5])
                } else {
                    result.literal_html += 1;
                    escaped(text)
                };
                edits.push(Edit {
                    range: range.clone(),
                    text: replacement,
                });
            }
            _ => {}
        }
        i += 1;
    }
    // Insertions at a cell's first byte must precede replacements at that byte.
    edits.sort_by_key(|e| (e.range.start, e.range.end));
    let mut cursor = 0;
    for edit in edits {
        if edit.range.start < cursor {
            return Err(Code::Input);
        }
        result.text.push_str(&input[cursor..edit.range.start]);
        result.text.push_str(&edit.text);
        cursor = edit.range.end;
        if result.text.len() > 4 << 20 {
            return Err(Code::Limit);
        }
    }
    result.text.push_str(&input[cursor..]);
    if result.text.len() > 4 << 20 {
        return Err(Code::Limit);
    }
    Ok(result)
}
