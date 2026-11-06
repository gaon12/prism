//! Convert Markdown structure to a fixed Typst vocabulary. Every literal is a
//! quoted string, including raw HTML and apparent Typst code. Images are visibly
//! omitted and counted; exporting never fetches a URL or opens a local image.
use super::{Code, Result};
use pulldown_cmark::{Alignment, Event, Options, Parser, Tag, TagEnd};
use std::collections::{HashMap, HashSet};

pub(super) struct Body {
    pub source: String,
    pub omitted_images: usize,
}
pub(super) fn string(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for c in value.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => {}
            '\t' => out.push_str("\\t"),
            c if c.is_control() => out.push('\u{fffd}'),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}
struct Context<'a> {
    language: &'static str,
    notes: HashMap<String, Vec<Event<'a>>>,
    note_ids: HashMap<String, usize>,
    rendered_notes: HashSet<String>,
    active_notes: HashSet<String>,
    omitted_images: usize,
}
pub(super) fn convert(input: &str, language: &str) -> Result<Body> {
    let options = Options::ENABLE_TABLES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_FOOTNOTES
        | Options::ENABLE_TASKLISTS;
    let events = Parser::new_ext(input, options).collect::<Vec<_>>();
    if events.len() > 20000 {
        return Err(Code::Limit);
    }
    let mut ctx = Context {
        language: match language {
            "ko" => "ko",
            "ja" => "ja",
            "zh-CN" => "zh-CN",
            "zh-TW" => "zh-TW",
            _ => "en",
        },
        notes: HashMap::new(),
        note_ids: HashMap::new(),
        rendered_notes: HashSet::new(),
        active_notes: HashSet::new(),
        omitted_images: 0,
    };
    let mut main = Vec::new();
    let mut i = 0;
    while i < events.len() {
        if let Event::Start(Tag::FootnoteDefinition(label)) = &events[i] {
            let end = find_end(&events, i, TagEnd::FootnoteDefinition)?;
            ctx.notes
                .insert(label.to_string(), events[i + 1..end].to_vec());
            i = end + 1;
        } else {
            main.push(events[i].clone());
            i += 1;
        }
    }
    let source = render(&main, &mut ctx, 0)?;
    Ok(Body {
        source,
        omitted_images: ctx.omitted_images,
    })
}
fn find_end(events: &[Event<'_>], start: usize, tag: TagEnd) -> Result<usize> {
    let mut depth = 0;
    for (i, event) in events.iter().enumerate().skip(start) {
        match event {
            Event::Start(t) if t.to_end() == tag => depth += 1,
            Event::End(t) if *t == tag => {
                depth -= 1;
                if depth == 0 {
                    return Ok(i);
                }
            }
            _ => {}
        }
    }
    Err(Code::Input)
}
fn text(out: &mut String, value: &str) {
    out.push_str("#text(");
    out.push_str(&string(value));
    out.push(')');
}
fn render(events: &[Event<'_>], ctx: &mut Context<'_>, depth: usize) -> Result<String> {
    if depth > 16 {
        return Err(Code::Limit);
    }
    let mut out = String::new();
    let mut endings = Vec::new();
    let mut i = 0;
    while i < events.len() {
        match &events[i] {
            Event::Start(Tag::Table(columns)) => {
                let end = find_end(events, i, TagEnd::Table)?;
                out.push_str(&table(&events[i + 1..end], columns, ctx, depth + 1)?);
                i = end;
            }
            Event::Start(Tag::CodeBlock(_)) => {
                let end = find_end(events, i, TagEnd::CodeBlock)?;
                let contents = events[i + 1..end]
                    .iter()
                    .filter_map(|e| {
                        if let Event::Text(t) = e {
                            Some(t.as_ref())
                        } else {
                            None
                        }
                    })
                    .collect::<String>();
                out.push_str(&format!("#raw({}, block: true)\n", string(&contents)));
                i = end;
            }
            Event::Start(Tag::Image { .. }) => {
                let end = find_end(events, i, TagEnd::Image)?;
                let alt = events[i + 1..end]
                    .iter()
                    .filter_map(|e| match e {
                        Event::Text(t) | Event::Code(t) => Some(t.as_ref()),
                        _ => None,
                    })
                    .collect::<String>();
                text(
                    &mut out,
                    &format!("[{}: {alt}]", label("image", ctx.language)),
                );
                ctx.omitted_images += 1;
                i = end;
            }
            Event::Start(tag) => {
                if endings.len() > 32 {
                    return Err(Code::Limit);
                }
                let (begin, end) = match tag {
                    Tag::Paragraph => ("#par[".into(), "]\n"),
                    Tag::Heading { level, .. } => {
                        (format!("#heading(level: {})[", *level as u8), "]\n")
                    }
                    Tag::Strong => ("#strong[".into(), "]"),
                    Tag::Emphasis => ("#emph[".into(), "]"),
                    Tag::Strikethrough => ("#strike[".into(), "]"),
                    Tag::BlockQuote(_) => (
                        "#block(inset: (left: 10pt), stroke: (left: 1pt + luma(65%)))[".into(),
                        "]\n",
                    ),
                    Tag::List(Some(start)) => {
                        if *start > 999999 {
                            return Err(Code::Limit);
                        }
                        (format!("#enum(start: {start},\n"), ")\n")
                    }
                    Tag::List(None) => ("#list(\n".into(), ")\n"),
                    Tag::Item => ("[".into(), "],\n"),
                    Tag::Link { dest_url, .. } if safe_link(dest_url) => {
                        (format!("#link({})[", string(dest_url)), "]")
                    }
                    _ => (String::new(), ""),
                };
                out.push_str(&begin);
                endings.push(end);
            }
            Event::End(_) => {
                out.push_str(endings.pop().ok_or(Code::Input)?);
            }
            Event::Text(value) => text(&mut out, value),
            Event::Code(value) => out.push_str(&format!("#raw({})", string(value))),
            Event::Html(value) | Event::InlineHtml(value) => {
                let kind = value
                    .trim()
                    .strip_prefix("<!-- prism:")
                    .and_then(|v| v.strip_suffix("; verified=false -->"));
                if let Some(kind) =
                    kind.filter(|v| ["fact", "userInput", "aiProposal", "blank"].contains(v))
                {
                    out.push_str("#block[");
                    text(
                        &mut out,
                        &format!(
                            "{} · {}",
                            label(kind, ctx.language),
                            label("unverified", ctx.language)
                        ),
                    );
                    out.push_str("]\n");
                } else {
                    text(&mut out, value);
                }
            }
            Event::SoftBreak => out.push(' '),
            Event::HardBreak => out.push_str("#linebreak()\n"),
            Event::Rule => out.push_str("#line(length: 100%, stroke: 0.5pt + luma(65%))\n"),
            Event::TaskListMarker(checked) => {
                text(&mut out, if *checked { "[x] " } else { "[ ] " })
            }
            Event::FootnoteReference(label) => {
                if !ctx.active_notes.insert(label.to_string()) {
                    return Err(Code::Input);
                }
                let n = ctx.note_ids.len() + 1;
                let number = *ctx.note_ids.entry(label.to_string()).or_insert(n);
                if ctx.rendered_notes.insert(label.to_string()) {
                    if let Some(mut contents) = ctx.notes.get(label.as_ref()).cloned() {
                        // Keep the first paragraph inline with Typst's note
                        // marker. A block paragraph would strand that marker
                        // on its own line above the actual footnote text.
                        if matches!(contents.first(), Some(Event::Start(Tag::Paragraph))) {
                            let end = find_end(&contents, 0, TagEnd::Paragraph)?;
                            contents.remove(end);
                            contents.remove(0);
                        }
                        out.push_str("#footnote[");
                        out.push_str(&render(&contents, ctx, depth + 1)?);
                        out.push_str(&format!("] <prism-note-{number}>"));
                    } else {
                        text(&mut out, &format!("[^{label}]"));
                    }
                } else {
                    out.push_str(&format!("#footnote(<prism-note-{number}>)"));
                }
                ctx.active_notes.remove(label.as_ref());
            }
            _ => {}
        }
        if out.len() > 4 << 20 {
            return Err(Code::Limit);
        }
        i += 1;
    }
    if !endings.is_empty() {
        return Err(Code::Input);
    }
    Ok(out)
}
fn safe_link(value: &str) -> bool {
    value.len() <= 4096
        && ["https://", "http://", "mailto:"]
            .iter()
            .any(|p| value.starts_with(p))
        && !value.chars().any(char::is_control)
}
fn label(kind: &str, language: &str) -> &'static str {
    let values = match kind {
        "image" => [
            "이미지 생략",
            "Image omitted",
            "图片已省略",
            "圖片已省略",
            "画像を省略",
        ],
        "fact" => ["사실", "Fact", "事实", "事實", "事実"],
        "userInput" => [
            "사용자 입력",
            "User input",
            "用户输入",
            "使用者輸入",
            "ユーザー入力",
        ],
        "aiProposal" => ["AI 제안", "AI proposal", "AI建议", "AI建議", "AIの提案"],
        "blank" => ["빈 항목", "Blank", "空白项", "空白項目", "空欄"],
        _ => [
            "검증되지 않음",
            "Claim not verified",
            "未经核实",
            "未經核實",
            "未検証",
        ],
    };
    values[match language {
        "ko" => 0,
        "zh-CN" => 2,
        "zh-TW" => 3,
        "ja" => 4,
        _ => 1,
    }]
}
fn table(
    events: &[Event<'_>],
    alignments: &[Alignment],
    ctx: &mut Context<'_>,
    depth: usize,
) -> Result<String> {
    let columns = alignments.len();
    if columns == 0 || columns > 32 {
        return Err(Code::Limit);
    }
    let mut header = Vec::new();
    let mut cells = Vec::new();
    let mut head = false;
    let mut i = 0;
    while i < events.len() {
        match &events[i] {
            Event::Start(Tag::TableHead) => head = true,
            Event::End(TagEnd::TableHead) => head = false,
            Event::Start(Tag::TableCell) => {
                let end = find_end(events, i, TagEnd::TableCell)?;
                let cell = render(&events[i + 1..end], ctx, depth + 1)?;
                if head {
                    header.push(cell);
                } else {
                    cells.push(cell);
                }
                if cells.len() + header.len() > 4096 {
                    return Err(Code::Limit);
                }
                i = end;
            }
            _ => {}
        }
        i += 1;
    }
    if header.len() != columns || cells.len() % columns != 0 {
        return Err(Code::Input);
    }
    let align = alignments
        .iter()
        .map(|value| match value {
            Alignment::None | Alignment::Left => "left,",
            Alignment::Center => "center,",
            Alignment::Right => "right,",
        })
        .collect::<String>();
    let mut out = format!(
        "#table(columns: ({}), align: ({align}),\ntable.header(",
        "1fr,".repeat(columns)
    );
    for value in header {
        out.push('[');
        out.push_str("#strong[");
        out.push_str(&value);
        out.push_str("]],");
    }
    out.push_str("),\n");
    for value in cells {
        out.push('[');
        out.push_str(&value);
        out.push_str("],\n");
    }
    out.push_str(")\n");
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn code_html_paths_and_network_images_are_only_literals() {
        let input="#eval(\"read(secret)\")\n\n<script src=\"https://private.invalid\"></script>\n\n[bad](javascript:alert(1)) ![alt](file:///private.png)";
        let body = convert(input, "en").unwrap();
        assert_eq!(body.omitted_images, 1);
        assert!(body
            .source
            .contains("#text(\"#eval(\\\"read(secret)\\\")\")"));
        assert!(!body.source.contains("#image("));
        assert!(!body.source.contains("#link(\"javascript:"));
    }
}
