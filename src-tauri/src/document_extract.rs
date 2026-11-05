//! Local-only document extraction. Callers must authorize the canonical path against
//! content-index scope before invoking this module. OCR is opt-in and local; no
//! network, scripts or macros are executed.
use encoding_rs::{EUC_KR, UTF_16BE, UTF_16LE};
use quick_xml::{events::Event, Reader};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs::{self, File, Metadata},
    io::{Cursor, Read},
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
    time::UNIX_EPOCH,
};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SourceFingerprint {
    pub sha256: String,
    pub size_bytes: u64,
    pub modified_unix_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DocumentSection {
    pub text: String,
    pub page: Option<u32>,
    pub paragraph: Option<u32>,
    pub heading: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExtractedDocument {
    pub file_path: String,
    pub fingerprint: SourceFingerprint,
    pub format: String,
    pub sections: Vec<DocumentSection>,
}

/// Worker resource budgets, not a product limit on indexed file count. Larger
/// documents can be retried with larger budgets by the scheduler.
#[derive(Debug, Clone)]
pub struct ExtractOptions {
    pub ocr: crate::ocr::Preferences,
    pub ocr_limits: crate::ocr::Limits,
    pub max_file_bytes: u64,
    pub max_expanded_bytes: u64,
    pub max_text_bytes: usize,
    pub max_sections: usize,
    pub max_archive_entries: usize,
    pub max_xml_depth: usize,
}

impl Default for ExtractOptions {
    fn default() -> Self {
        Self {
            ocr: crate::ocr::Preferences::default(),
            ocr_limits: crate::ocr::Limits::default(),
            max_file_bytes: 64 * 1024 * 1024,
            max_expanded_bytes: 128 * 1024 * 1024,
            max_text_bytes: 32 * 1024 * 1024,
            max_sections: 100_000,
            max_archive_entries: 10_000,
            max_xml_depth: 128,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ExtractErrorCode {
    UnsupportedFormat,
    Encrypted,
    PasswordRejected,
    CorruptDocument,
    OcrRequired,
    OcrUnavailable,
    OcrLanguageUnavailable,
    OcrNoText,
    OcrTimedOut,
    OcrBusy,
    OcrFailed,
    LimitExceeded,
    Cancelled,
    Io,
    UnsafePath,
    SourceChanged,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExtractError {
    pub code: ExtractErrorCode,
    /// Stable diagnostic only; never contains source text, passwords or XML.
    pub message: String,
}

impl std::fmt::Display for ExtractError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for ExtractError {}
type Result<T> = std::result::Result<T, ExtractError>;
fn err(code: ExtractErrorCode, message: &str) -> ExtractError {
    ExtractError {
        code,
        message: message.into(),
    }
}
fn corrupt() -> ExtractError {
    err(
        ExtractErrorCode::CorruptDocument,
        "Document structure is invalid or damaged",
    )
}
fn limit() -> ExtractError {
    err(
        ExtractErrorCode::LimitExceeded,
        "Document exceeds extraction worker budget",
    )
}
fn encrypted() -> ExtractError {
    err(
        ExtractErrorCode::Encrypted,
        "Document is encrypted or access protected",
    )
}
fn check_cancel(cancel: &AtomicBool) -> Result<()> {
    if cancel.load(Ordering::Relaxed) {
        Err(err(
            ExtractErrorCode::Cancelled,
            "Document extraction cancelled",
        ))
    } else {
        Ok(())
    }
}
fn modified(meta: &Metadata) -> Option<u64> {
    meta.modified()
        .ok()?
        .duration_since(UNIX_EPOCH)
        .ok()?
        .as_millis()
        .try_into()
        .ok()
}

/// Apply the shared link policy, including verified macOS root aliases.
/// Scope authorization remains the caller's job; this is not a sandbox.
fn check_path(path: &Path) -> Result<()> {
    crate::path_identity::no_links(path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::InvalidInput {
            err(
                ExtractErrorCode::UnsafePath,
                "Symlink or reparse-point document paths are skipped",
            )
        } else {
            err(ExtractErrorCode::Io, "Cannot inspect document path")
        }
    })
}

fn bounded_read(mut input: impl Read, max: u64, cancel: &AtomicBool) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    let mut block = [0u8; 32768];
    loop {
        check_cancel(cancel)?;
        let count = input.read(&mut block).map_err(|_| corrupt())?;
        if count == 0 {
            break;
        }
        if bytes.len() as u64 + count as u64 > max {
            return Err(limit());
        }
        bytes.extend_from_slice(&block[..count]);
    }
    Ok(bytes)
}

pub fn is_supported_extension(extension: &str) -> bool {
    if is_image_extension(extension) {
        return true;
    }
    matches!(
        extension.to_ascii_lowercase().as_str(),
        "txt"
            | "md"
            | "markdown"
            | "csv"
            | "tsv"
            | "json"
            | "jsonl"
            | "html"
            | "htm"
            | "xml"
            | "yaml"
            | "yml"
            | "toml"
            | "ini"
            | "cfg"
            | "log"
            | "rs"
            | "ts"
            | "tsx"
            | "js"
            | "jsx"
            | "py"
            | "java"
            | "c"
            | "h"
            | "cpp"
            | "hpp"
            | "cs"
            | "go"
            | "rb"
            | "php"
            | "swift"
            | "kt"
            | "css"
            | "scss"
            | "sql"
            | "sh"
            | "ps1"
            | "bat"
            | "vue"
            | "svelte"
            | "pptx"
            | "xlsx"
            | "odt"
            | "ods"
            | "odp"
            | "docx"
            | "hwpx"
            | "pdf"
            | "hwp"
    )
}

pub fn is_image_extension(extension: &str) -> bool {
    matches!(
        extension.to_ascii_lowercase().as_str(),
        "png" | "jpg" | "jpeg" | "bmp" | "tif" | "tiff" | "webp"
    )
}

fn ocr_error(value: crate::ocr::Error) -> ExtractError {
    use crate::ocr::Code;
    let code = match value.code {
        Code::Disabled => ExtractErrorCode::OcrRequired,
        Code::Unavailable => ExtractErrorCode::OcrUnavailable,
        Code::LanguageUnavailable => ExtractErrorCode::OcrLanguageUnavailable,
        Code::TimedOut => ExtractErrorCode::OcrTimedOut,
        Code::Busy => ExtractErrorCode::OcrBusy,
        Code::Cancelled => ExtractErrorCode::Cancelled,
        Code::LimitExceeded => ExtractErrorCode::LimitExceeded,
        Code::CorruptDocument => ExtractErrorCode::CorruptDocument,
        Code::InvalidInput | Code::Failed => ExtractErrorCode::OcrFailed,
    };
    ExtractError {
        code,
        message: value.message,
    }
}

fn recognize(
    input: crate::ocr::Input<'_>,
    out: &Collector<'_>,
    remaining: usize,
) -> Result<Vec<crate::ocr::Page>> {
    let mut limits = out.options.ocr_limits.clone();
    limits.max_input_bytes = limits
        .max_input_bytes
        .min(out.options.max_file_bytes as usize);
    limits.max_text_bytes = limits.max_text_bytes.min(remaining);
    limits.max_pages = limits.max_pages.min(out.options.max_sections);
    if remaining == 0 {
        return Err(limit());
    }
    crate::ocr::recognize(input, &out.options.ocr, &limits, out.cancel).map_err(ocr_error)
}

fn extract_image(bytes: &[u8], out: &mut Collector<'_>) -> Result<()> {
    let pages = recognize(
        crate::ocr::Input::Image(bytes),
        out,
        out.options.max_text_bytes.saturating_sub(out.bytes),
    )?;
    for page in pages {
        extract_lines(&page.text, false, Some(page.number), out)?;
    }
    if out.sections.is_empty() {
        return Err(err(
            ExtractErrorCode::OcrNoText,
            "Local OCR found no readable text in the image",
        ));
    }
    Ok(())
}

pub fn extract_document(
    path: &Path,
    options: &ExtractOptions,
    cancel: &AtomicBool,
) -> Result<ExtractedDocument> {
    extract_document_inner(path, options, cancel, None)
}

/// Explicit user retry. The caller owns the transient password and must not
/// persist it; neither the original PDF nor a decrypted copy is written here.
pub fn extract_document_with_password(
    path: &Path,
    options: &ExtractOptions,
    cancel: &AtomicBool,
    password: &str,
) -> Result<ExtractedDocument> {
    if password.len() > 1024 {
        return Err(err(
            ExtractErrorCode::LimitExceeded,
            "PDF password exceeds the input budget",
        ));
    }
    if !path
        .extension()
        .and_then(|value| value.to_str())
        .is_some_and(|value| value.eq_ignore_ascii_case("pdf"))
    {
        return Err(err(
            ExtractErrorCode::UnsupportedFormat,
            "Password retry currently supports PDF documents",
        ));
    }
    extract_document_inner(path, options, cancel, Some(password))
}

fn extract_document_inner(
    path: &Path,
    options: &ExtractOptions,
    cancel: &AtomicBool,
    password: Option<&str>,
) -> Result<ExtractedDocument> {
    check_cancel(cancel)?;
    let extension = path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if !is_supported_extension(&extension) {
        return Err(err(
            ExtractErrorCode::UnsupportedFormat,
            "Document format is not supported",
        ));
    }
    // Avoid even reading raster payloads while opt-in OCR is disabled.
    if is_image_extension(&extension) && !options.ocr.enabled {
        return Err(ocr_error(crate::ocr::error(crate::ocr::Code::Disabled)));
    }
    let absolute = std::path::absolute(path)
        .map_err(|_| err(ExtractErrorCode::Io, "Cannot resolve document path"))?;
    check_path(&absolute)?;
    let canonical = absolute
        .canonicalize()
        .map_err(|_| err(ExtractErrorCode::Io, "Cannot resolve document path"))?;
    let file =
        File::open(&canonical).map_err(|_| err(ExtractErrorCode::Io, "Cannot open document"))?;
    let before = file
        .metadata()
        .map_err(|_| err(ExtractErrorCode::Io, "Cannot inspect document"))?;
    if !before.is_file() {
        return Err(err(
            ExtractErrorCode::UnsafePath,
            "Document is not a regular file",
        ));
    }
    if before.len() > options.max_file_bytes {
        return Err(limit());
    }
    let bytes = bounded_read(&file, options.max_file_bytes, cancel)?;
    let mut output = Collector::new(options, cancel);
    match extension.as_str() {
        "docx" | "hwpx" => extract_zip(&bytes, &extension, &mut output)?,
        "pptx" | "xlsx" | "odt" | "ods" | "odp" => extract_office(&bytes, &extension, &mut output)?,
        "hwp" => extract_hwp(&bytes, &mut output)?,
        "pdf" => extract_pdf(&bytes, password, &mut output)?,
        extension if is_image_extension(extension) => extract_image(&bytes, &mut output)?,
        "html" | "htm" => extract_html(&decode_text(&bytes)?, &mut output)?,
        "csv" | "tsv" => extract_delimited(
            &decode_text(&bytes)?,
            if extension == "csv" { ',' } else { '\t' },
            &mut output,
        )?,
        _ => extract_lines(
            &decode_text(&bytes)?,
            extension == "md" || extension == "markdown",
            None,
            &mut output,
        )?,
    }
    check_cancel(cancel)?;
    check_path(&absolute)?;
    let after = file
        .metadata()
        .map_err(|_| err(ExtractErrorCode::Io, "Cannot inspect document"))?;
    let current = fs::metadata(&canonical).map_err(|_| {
        err(
            ExtractErrorCode::SourceChanged,
            "Document changed during extraction",
        )
    })?;
    if before.len() != after.len()
        || before.len() != current.len()
        || modified(&before) != modified(&after)
        || modified(&before) != modified(&current)
        || canonical
            != absolute.canonicalize().map_err(|_| {
                err(
                    ExtractErrorCode::SourceChanged,
                    "Document changed during extraction",
                )
            })?
    {
        return Err(err(
            ExtractErrorCode::SourceChanged,
            "Document changed during extraction",
        ));
    }
    Ok(ExtractedDocument {
        file_path: canonical.to_string_lossy().into_owned(),
        fingerprint: SourceFingerprint {
            sha256: format!("{:x}", Sha256::digest(&bytes)),
            size_bytes: bytes.len() as u64,
            modified_unix_ms: modified(&before),
        },
        format: extension,
        sections: output.sections,
    })
}

struct Collector<'a> {
    sections: Vec<DocumentSection>,
    bytes: usize,
    paragraph: u32,
    heading: Option<String>,
    options: &'a ExtractOptions,
    cancel: &'a AtomicBool,
}
impl<'a> Collector<'a> {
    fn new(options: &'a ExtractOptions, cancel: &'a AtomicBool) -> Self {
        Self {
            sections: Vec::new(),
            bytes: 0,
            paragraph: 0,
            heading: None,
            options,
            cancel,
        }
    }
    fn push(&mut self, text: &str, page: Option<u32>, paragraph: u32) -> Result<()> {
        check_cancel(self.cancel)?;
        let text = text.trim_matches(['\n', '\r', ' ']);
        if text.trim().is_empty() {
            return Ok(());
        }
        if self.sections.len() >= self.options.max_sections
            || self
                .bytes
                .saturating_add(text.len())
                .saturating_add(self.heading.as_ref().map_or(0, String::len))
                > self.options.max_text_bytes
        {
            return Err(limit());
        }
        self.bytes += text.len() + self.heading.as_ref().map_or(0, String::len);
        self.sections.push(DocumentSection {
            text: text.into(),
            page,
            paragraph: Some(paragraph),
            heading: self.heading.clone(),
        });
        Ok(())
    }
    fn next_paragraph(&mut self) -> u32 {
        self.paragraph += 1;
        self.paragraph
    }
}

fn decode_text(bytes: &[u8]) -> Result<String> {
    let (text, bad) = if let Some(rest) = bytes.strip_prefix(&[0xff, 0xfe]) {
        let (s, _, bad) = UTF_16LE.decode(rest);
        (s.into_owned(), bad)
    } else if let Some(rest) = bytes.strip_prefix(&[0xfe, 0xff]) {
        let (s, _, bad) = UTF_16BE.decode(rest);
        (s.into_owned(), bad)
    } else {
        let bytes = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(bytes);
        match std::str::from_utf8(bytes) {
            Ok(s) => (s.to_owned(), false),
            Err(_) => {
                let (s, _, bad) = EUC_KR.decode(bytes);
                (s.into_owned(), bad)
            }
        }
    };
    if bad
        || text.contains('\0')
        || text
            .chars()
            .filter(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t' | '\u{c}'))
            .count()
            > 0
    {
        return Err(corrupt());
    }
    Ok(text)
}

fn extract_lines(
    text: &str,
    markdown: bool,
    page: Option<u32>,
    out: &mut Collector<'_>,
) -> Result<()> {
    for line in text.lines() {
        let paragraph = out.next_paragraph();
        if markdown {
            let start = line.trim_start();
            let count = start.chars().take_while(|c| *c == '#').count();
            if (1..=6).contains(&count) && start.as_bytes().get(count) == Some(&b' ') {
                out.heading = Some(start[count..].trim().trim_end_matches('#').trim().into());
            }
        }
        out.push(line, page, paragraph)?;
    }
    Ok(())
}

fn extract_delimited(text: &str, delimiter: char, out: &mut Collector<'_>) -> Result<()> {
    let mut chars = text.chars().peekable();
    let mut field = String::new();
    let mut fields = Vec::new();
    let mut quoted = false;
    let mut closed_quote = false;
    let mut line = 1u32;
    let mut row_line = line;
    let mut headers: Vec<String> = Vec::new();
    while let Some(ch) = chars.next() {
        check_cancel(out.cancel)?;
        if quoted {
            if ch == '"' {
                if chars.peek() == Some(&'"') {
                    chars.next();
                    field.push('"');
                } else {
                    quoted = false;
                    closed_quote = true;
                }
            } else {
                if ch == '\n' {
                    line += 1;
                }
                field.push(ch);
            }
        } else if ch == delimiter {
            fields.push(std::mem::take(&mut field));
            closed_quote = false;
        } else if matches!(ch, '\r' | '\n') {
            if ch == '\r' && chars.peek() == Some(&'\n') {
                chars.next();
            }
            fields.push(std::mem::take(&mut field));
            if headers.is_empty() {
                headers = fields.clone();
            }
            emit_row(
                &fields,
                &headers,
                "CSV/TSV; header=first-row inferred",
                row_line,
                None,
                out,
            )?;
            fields.clear();
            line += 1;
            row_line = line;
            closed_quote = false;
        } else if ch == '"' && field.is_empty() && !closed_quote {
            quoted = true;
        } else {
            if closed_quote || ch == '"' {
                return Err(corrupt());
            }
            field.push(ch);
        }
        if field.len() > out.options.max_text_bytes || fields.len() > out.options.max_sections {
            return Err(limit());
        }
    }
    if quoted {
        return Err(corrupt());
    }
    if !fields.is_empty() || !field.is_empty() || closed_quote {
        fields.push(field);
        if headers.is_empty() {
            headers = fields.clone();
        }
        emit_row(
            &fields,
            &headers,
            "CSV/TSV; header=first-row inferred",
            row_line,
            None,
            out,
        )?;
    }
    Ok(())
}

fn extract_html(text: &str, out: &mut Collector<'_>) -> Result<()> {
    let html = scraper::Html::parse_document(text);
    let body = scraper::Selector::parse("body").expect("static selector");
    fn walk(
        element: scraper::ElementRef<'_>,
        text: &mut String,
        depth: usize,
        out: &Collector<'_>,
    ) -> Result<()> {
        check_cancel(out.cancel)?;
        if depth > out.options.max_xml_depth {
            return Err(limit());
        }
        let name = element.value().name();
        if matches!(name, "script" | "style" | "head" | "noscript" | "template") {
            return Ok(());
        }
        let block = matches!(
            name,
            "p" | "div"
                | "section"
                | "article"
                | "li"
                | "tr"
                | "h1"
                | "h2"
                | "h3"
                | "h4"
                | "h5"
                | "h6"
                | "pre"
                | "br"
        );
        if block && !text.ends_with('\n') {
            text.push('\n');
        }
        for child in element.children() {
            if let Some(element) = scraper::ElementRef::wrap(child) {
                walk(element, text, depth + 1, out)?;
            } else if let scraper::node::Node::Text(value) = child.value() {
                text.push_str(value);
            }
            if text.len() > out.options.max_text_bytes {
                return Err(limit());
            }
        }
        if matches!(name, "td" | "th") {
            text.push('\t');
        }
        if block && !text.ends_with('\n') {
            text.push('\n');
        }
        Ok(())
    }
    let mut rendered = String::new();
    if let Some(body) = html.select(&body).next() {
        walk(body, &mut rendered, 0, out)?;
    }
    extract_lines(&rendered, false, None, out)
}

#[derive(Default)]
struct XmlNode {
    name: String,
    attrs: Vec<(String, String)>,
    text: String,
    children: Vec<XmlNode>,
}
fn xml_tree(bytes: &[u8], out: &Collector<'_>) -> Result<XmlNode> {
    let text = decode_text(bytes)?;
    // Reject before quick-xml scans the DTD: older parser releases have had
    // malformed-DTD panics. No package part needs a DTD for text extraction.
    if text.contains("<!DOCTYPE") {
        return Err(corrupt());
    }
    let mut reader = Reader::from_str(&text);
    let mut stack = vec![XmlNode::default()];
    let mut nodes = 0usize;
    loop {
        check_cancel(out.cancel)?;
        nodes = nodes.saturating_add(1);
        if nodes > out.options.max_sections.saturating_mul(40).max(200) {
            return Err(limit());
        }
        match reader.read_event().map_err(|_| corrupt())? {
            Event::Start(e) | Event::Empty(e) => {
                // Empty elements are identified using the reader's consumed token.
                let empty = text[..reader.buffer_position() as usize]
                    .trim_end()
                    .ends_with("/>");
                nodes += 1;
                if nodes > out.options.max_sections.saturating_mul(20).max(100)
                    || stack.len() > out.options.max_xml_depth
                {
                    return Err(limit());
                }
                let name = String::from_utf8_lossy(e.local_name().as_ref()).into_owned();
                let mut node = XmlNode {
                    name,
                    ..XmlNode::default()
                };
                for attribute in e.attributes() {
                    let attr = attribute.map_err(|_| corrupt())?;
                    let key = String::from_utf8_lossy(attr.key.local_name().as_ref()).into_owned();
                    let value = attr
                        .decode_and_unescape_value(reader.decoder())
                        .map_err(|_| corrupt())?
                        .into_owned();
                    let qualified = String::from_utf8_lossy(attr.key.as_ref()).into_owned();
                    if qualified != key {
                        node.attrs.push((qualified, value.clone()));
                    }
                    node.attrs.push((key, value));
                }
                if empty {
                    stack.last_mut().ok_or_else(corrupt)?.children.push(node);
                } else {
                    stack.push(node);
                }
            }
            Event::End(_) => {
                if stack.len() <= 1 {
                    return Err(corrupt());
                }
                let node = stack.pop().ok_or_else(corrupt)?;
                stack.last_mut().ok_or_else(corrupt)?.children.push(node);
            }
            Event::Text(e) => xml_text(
                stack.last_mut().ok_or_else(corrupt)?,
                &e.decode().map_err(|_| corrupt())?,
            ),
            Event::CData(e) => xml_text(
                stack.last_mut().ok_or_else(corrupt)?,
                &e.decode().map_err(|_| corrupt())?,
            ),
            Event::GeneralRef(e) => {
                let value = e.decode().map_err(|_| corrupt())?;
                let entity = format!("&{value};");
                xml_text(
                    stack.last_mut().ok_or_else(corrupt)?,
                    &quick_xml::escape::unescape(&entity).map_err(|_| corrupt())?,
                );
            }
            Event::DocType(_) => return Err(corrupt()), // no DTDs/entities or external resolution
            Event::Eof => break,
            _ => {}
        }
    }
    if stack.len() != 1
        || stack[0]
            .children
            .iter()
            .filter(|n| n.name != "#text")
            .count()
            != 1
        || !stack[0].text.trim().is_empty()
    {
        return Err(corrupt());
    }
    stack.pop().ok_or_else(corrupt)
}

fn xml_inline(node: &XmlNode, text: &mut String) {
    match node.name.as_str() {
        "t" => text.push_str(&node.text),
        "tab" => text.push('\t'),
        "br" | "lineBreak" => text.push('\n'),
        _ => {}
    }
    for child in &node.children {
        if matches!(child.name.as_str(), "tbl" | "tr" | "tc") {
            continue;
        }
        xml_inline(child, text);
    }
}
fn xml_paragraphs(node: &XmlNode, out: &mut Collector<'_>) -> Result<()> {
    check_cancel(out.cancel)?;
    if node.name == "tbl" {
        let rows: Vec<_> = node.children.iter().filter(|n| n.name == "tr").collect();
        let headers = rows
            .first()
            .map(|row| aligned_table_cells(row, true, out.options.max_sections))
            .transpose()?
            .unwrap_or_default();
        let explicit = rows
            .first()
            .is_some_and(|row| descendants(row, "tblHeader").next().is_some());
        let context = format!(
            "{} / table; header={}",
            out.heading.as_deref().unwrap_or("Document"),
            if explicit {
                "declared"
            } else {
                "first-row inferred"
            }
        );
        for row in rows {
            let para = out.next_paragraph();
            emit_row(
                &aligned_table_cells(row, false, out.options.max_sections)?,
                &headers,
                &context,
                para,
                None,
                out,
            )?;
        }
        return Ok(());
    }
    if node.name == "tr" {
        let paragraph = out.next_paragraph();
        let mut cells = Vec::new();
        for cell in node.children.iter().filter(|n| n.name == "tc") {
            let mut text = String::new();
            fn cell_text(node: &XmlNode, text: &mut String) {
                if node.name == "tr" {
                    if !text.is_empty() {
                        text.push('\n');
                    }
                    let cells: Vec<String> = node
                        .children
                        .iter()
                        .filter(|n| n.name == "tc")
                        .map(|cell| {
                            let mut cell_value = String::new();
                            cell_text(cell, &mut cell_value);
                            cell_value
                        })
                        .collect();
                    text.push_str(&cells.join("\t"));
                    return;
                }
                if node.name == "p" {
                    if !text.is_empty() {
                        text.push('\n');
                    }
                    xml_inline(node, text);
                }
                for child in &node.children {
                    cell_text(child, text);
                }
            }
            cell_text(cell, &mut text);
            cells.push(text);
        }
        out.push(&cells.join("\t"), None, paragraph)?;
        return Ok(());
    }
    if node.name == "p" {
        let paragraph = out.next_paragraph();
        let mut text = String::new();
        xml_inline(node, &mut text);
        let style = node
            .children
            .iter()
            .find(|n| n.name == "pPr")
            .and_then(|n| n.children.iter().find(|n| n.name == "pStyle"))
            .and_then(|n| n.attrs.iter().find(|(k, _)| k == "val"));
        if style.is_some_and(|(_, value)| value.to_ascii_lowercase().starts_with("heading")) {
            out.heading = Some(text.clone());
        }
        out.push(&text, None, paragraph)?;
    }
    for child in &node.children {
        xml_paragraphs(child, out)?;
    }
    Ok(())
}

fn zip_error(error: zip::result::ZipError) -> ExtractError {
    match error {
        zip::result::ZipError::UnsupportedArchive(message)
            if message.to_ascii_lowercase().contains("password")
                || message.to_ascii_lowercase().contains("encrypt") =>
        {
            encrypted()
        }
        _ => corrupt(),
    }
}
fn extract_zip(bytes: &[u8], format: &str, out: &mut Collector<'_>) -> Result<()> {
    if bytes.starts_with(&[0xd0, 0xcf, 0x11, 0xe0]) {
        let compound = cfb::CompoundFile::open(Cursor::new(bytes)).map_err(|_| corrupt())?;
        if compound.is_stream("/EncryptionInfo") || compound.is_stream("/EncryptedPackage") {
            return Err(encrypted());
        }
        return Err(corrupt());
    }
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).map_err(zip_error)?;
    if zip.len() > out.options.max_archive_entries {
        return Err(limit());
    }
    let mut expanded = 0u64;
    let mut parts = Vec::new();
    for index in 0..zip.len() {
        check_cancel(out.cancel)?;
        let entry = zip.by_index(index).map_err(zip_error)?;
        if entry.enclosed_name().is_none()
            || entry
                .unix_mode()
                .is_some_and(|mode| mode & 0o170000 == 0o120000)
        {
            return Err(err(
                ExtractErrorCode::UnsafePath,
                "Unsafe archive member path",
            ));
        }
        if entry.encrypted() {
            return Err(encrypted());
        }
        expanded = expanded.checked_add(entry.size()).ok_or_else(limit)?;
        if expanded > out.options.max_expanded_bytes {
            return Err(limit());
        }
        let name = entry.name().map_err(zip_error)?;
        if format == "docx" && name == "word/document.xml" {
            parts.push((0, index));
        } else if format == "hwpx" {
            if let Some(ordinal) = name
                .strip_prefix("Contents/section")
                .and_then(|s| s.strip_suffix(".xml"))
                .and_then(|s| s.parse::<u32>().ok())
            {
                parts.push((ordinal, index));
            }
        }
    }
    if parts.is_empty() {
        return Err(corrupt());
    }
    parts.sort_unstable();
    let mut actual_expanded = 0u64;
    for (_, index) in parts {
        let entry = zip.by_index(index).map_err(zip_error)?;
        let xml = bounded_read(
            entry,
            out.options
                .max_expanded_bytes
                .saturating_sub(actual_expanded),
            out.cancel,
        )?;
        actual_expanded += xml.len() as u64;
        let root = xml_tree(&xml, out)?;
        xml_paragraphs(&root, out)?;
    }
    Ok(())
}

fn extract_pdf(bytes: &[u8], password: Option<&str>, out: &mut Collector<'_>) -> Result<()> {
    struct TextWriter<'a> {
        bytes: Vec<u8>,
        max: usize,
        cancel: &'a AtomicBool,
        failure: Option<ExtractError>,
    }
    impl std::io::Write for TextWriter<'_> {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            let failure = check_cancel(self.cancel)
                .err()
                .or_else(|| (self.bytes.len().saturating_add(bytes.len()) > self.max).then(limit));
            if let Some(error) = failure {
                self.failure = Some(error);
                return Err(std::io::Error::other("PDF extraction interrupted"));
            }
            self.bytes.extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    if !bytes.starts_with(b"%PDF-") {
        return Err(corrupt());
    }
    // pdf-extract/lopdf decoding is an in-process dependency. Cancellation is
    // observed between pages; its internal stream allocations are not capped.
    let (mut texts, render_bytes) =
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> Result<_> {
            let options = password
                .map(pdf_extract::LoadOptions::with_password)
                .unwrap_or_default();
            let mut doc =
                pdf_extract::Document::load_mem_with_options(bytes, options).map_err(|value| {
                    if matches!(
                        value,
                        pdf_extract::Error::InvalidPassword | pdf_extract::Error::Decryption(_)
                    ) {
                        if password.is_some() {
                            err(
                                ExtractErrorCode::PasswordRejected,
                                "Could not decrypt this PDF with the supplied password",
                            )
                        } else {
                            encrypted()
                        }
                    } else {
                        corrupt()
                    }
                })?;
            check_cancel(out.cancel)?;
            if doc.is_encrypted() {
                return Err(encrypted());
            }
            let pages = doc.get_pages();
            if pages.len() > out.options.max_sections {
                return Err(limit());
            }
            let mut texts = Vec::new();
            let mut total = out.bytes;
            for number in pages.keys() {
                check_cancel(out.cancel)?;
                let mut writer = TextWriter {
                    bytes: Vec::new(),
                    max: out.options.max_text_bytes.saturating_sub(total),
                    cancel: out.cancel,
                    failure: None,
                };
                let result = {
                    let target: &mut dyn std::io::Write = &mut writer;
                    let mut target = pdf_extract::PlainTextOutput::new(target);
                    pdf_extract::output_doc_page(&doc, &mut target, *number)
                };
                if let Some(error) = writer.failure {
                    return Err(error);
                }
                result.map_err(|_| corrupt())?;
                let page_text = String::from_utf8(writer.bytes).map_err(|_| corrupt())?;
                total += page_text.len();
                texts.push((*number, page_text));
            }
            // Windows.Data.Pdf does not implement AES-256. Authentication and cipher
            // handling already happened in the existing Rust parser, so serialize
            // its decrypted objects only when scanned pages actually need OCR.
            // This writer has no file target; raw-source SHA remains the identity.
            let needs_ocr =
                out.options.ocr.enabled && texts.iter().any(|(_, t)| t.trim().is_empty());
            let render_bytes = if needs_ocr && doc.encryption_state.is_some() {
                let mut writer = TextWriter {
                    bytes: Vec::new(),
                    max: out
                        .options
                        .ocr_limits
                        .max_input_bytes
                        .min(out.options.max_file_bytes as usize),
                    cancel: out.cancel,
                    failure: None,
                };
                let result = doc.save_to(&mut writer);
                if let Some(error) = writer.failure {
                    return Err(error);
                }
                result.map_err(|_| corrupt())?;
                Some(zeroize::Zeroizing::new(writer.bytes))
            } else {
                None
            };
            // Drop the parser's decoded objects before allocating native raster pages.
            Ok((texts, render_bytes))
        }))
        .map_err(|_| corrupt())??;
    if out.options.ocr.enabled {
        let wanted: Vec<_> = texts
            .iter()
            .filter(|(_, text)| text.trim().is_empty())
            .map(|(number, _)| *number)
            .collect();
        if !wanted.is_empty() {
            let total: usize = texts.iter().map(|(_, t)| t.len()).sum();
            let recognized = recognize(
                crate::ocr::Input::Pdf {
                    bytes: render_bytes.as_deref().map(Vec::as_slice).unwrap_or(bytes),
                    pages: &wanted,
                },
                out,
                out.options
                    .max_text_bytes
                    .saturating_sub(out.bytes)
                    .saturating_sub(total),
            )?;
            for page in recognized {
                let target = texts
                    .iter_mut()
                    .find(|(number, _)| *number == page.number)
                    .ok_or_else(corrupt)?;
                target.1 = page.text;
            }
        }
    }
    // Keep original page order, including OCR between two typed pages.
    for (number, text) in texts {
        extract_lines(&text, false, Some(number), out)?;
    }
    if out.sections.is_empty() {
        return Err(err(
            if out.options.ocr.enabled {
                ExtractErrorCode::OcrNoText
            } else {
                ExtractErrorCode::OcrRequired
            },
            if out.options.ocr.enabled {
                "Local OCR found no readable text in the PDF"
            } else {
                "PDF has no extractable text; OCR is disabled"
            },
        ));
    }
    Ok(())
}

fn extract_hwp(bytes: &[u8], out: &mut Collector<'_>) -> Result<()> {
    let mut compound = cfb::CompoundFile::open(Cursor::new(bytes)).map_err(|_| corrupt())?;
    let header = bounded_read(
        compound.open_stream("/FileHeader").map_err(|_| corrupt())?,
        256,
        out.cancel,
    )?;
    if header.len() != 256 || !header.starts_with(b"HWP Document File\0") {
        return Err(corrupt());
    }
    if header[35] != 5 {
        return Err(err(
            ExtractErrorCode::UnsupportedFormat,
            "Only HWP version 5 is supported",
        ));
    }
    let flags = u32::from_le_bytes(header[36..40].try_into().map_err(|_| corrupt())?);
    if flags & ((1 << 1) | (1 << 2) | (1 << 4) | (1 << 8) | (1 << 10)) != 0 {
        return Err(encrypted());
    }
    let mut streams: Vec<_> = compound
        .walk()
        .filter(|entry| entry.is_stream())
        .filter_map(|entry| {
            let path = entry.path().to_string_lossy().replace('\\', "/");
            let ordinal = path
                .strip_prefix("/BodyText/Section")?
                .parse::<u32>()
                .ok()?;
            Some((ordinal, entry.path().to_owned()))
        })
        .collect();
    if streams.is_empty() {
        return Err(corrupt());
    }
    if streams.len() > out.options.max_archive_entries {
        return Err(limit());
    }
    streams.sort_unstable();
    let mut expanded = 0u64;
    for (_, path) in streams {
        let stream = compound.open_stream(path).map_err(|_| corrupt())?;
        let remaining = out.options.max_expanded_bytes.saturating_sub(expanded);
        let data = if flags & 1 != 0 {
            bounded_read(
                flate2::read::DeflateDecoder::new(stream),
                remaining,
                out.cancel,
            )?
        } else {
            bounded_read(stream, remaining, out.cancel)?
        };
        expanded += data.len() as u64;
        hwp_records(&data, out)?;
    }
    Ok(())
}

fn hwp_text(bytes: &[u8]) -> Result<String> {
    if bytes.len() % 2 != 0 {
        return Err(corrupt());
    }
    let units: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|b| u16::from_le_bytes([b[0], b[1]]))
        .collect();
    let mut clean = Vec::new();
    let mut index = 0;
    while index < units.len() {
        let ch = units[index];
        match ch {
            1..=9 | 11..=12 | 14..=23 => {
                if index + 8 > units.len() || units[index + 7] != ch {
                    return Err(corrupt());
                }
                if ch == 9 {
                    clean.push(b'\t' as u16);
                }
                index += 8;
            }
            10 | 13 => {
                clean.push(b'\n' as u16);
                index += 1;
            }
            24 => {
                clean.push(b'-' as u16);
                index += 1;
            }
            30 | 31 => {
                clean.push(b' ' as u16);
                index += 1;
            }
            0..=31 => {
                index += 1;
            }
            _ => {
                clean.push(ch);
                index += 1;
            }
        }
    }
    String::from_utf16(&clean).map_err(|_| corrupt())
}

fn hwp_records(data: &[u8], out: &mut Collector<'_>) -> Result<()> {
    struct Record<'a> {
        tag: u32,
        level: u32,
        data: &'a [u8],
        paragraph: u32,
    }
    fn render(records: &[Record<'_>], out: &mut Collector<'_>) -> Result<()> {
        let mut index = 0;
        while index < records.len() {
            check_cancel(out.cancel)?;
            let record = &records[index];
            // MAKE_4CHID('t','b','l',' ') is little endian in the stream.
            if record.tag == 71 && record.data.starts_with(b" lbt") {
                let end = (index + 1..records.len())
                    .find(|i| records[*i].level <= record.level)
                    .unwrap_or(records.len());
                let mut cells: std::collections::BTreeMap<(u16, u16), (String, u32)> =
                    std::collections::BTreeMap::new();
                let mut child = index + 1;
                while child < end {
                    let list = &records[child];
                    if list.tag == 72 && list.level == record.level + 1 && list.data.len() >= 32 {
                        // Contemporary HWP writers use a 4-byte paragraph count;
                        // the older published table describes a 2-byte count.
                        let base = if list.data.len() == 32 { 6 } else { 8 };
                        let column = u16::from_le_bytes([list.data[base], list.data[base + 1]]);
                        let row = u16::from_le_bytes([list.data[base + 2], list.data[base + 3]]);
                        let cell_end = (child + 1..end)
                            .find(|i| records[*i].level <= list.level)
                            .unwrap_or(end);
                        let mut cell_output = Collector::new(out.options, out.cancel);
                        render(&records[child + 1..cell_end], &mut cell_output)?;
                        let paragraph = cell_output
                            .sections
                            .first()
                            .and_then(|s| s.paragraph)
                            .unwrap_or(list.paragraph);
                        let text = cell_output
                            .sections
                            .iter()
                            .map(|s| s.text.as_str())
                            .collect::<Vec<_>>()
                            .join("\n");
                        cells.insert((row, column), (text, paragraph));
                        child = cell_end;
                    } else {
                        child += 1;
                    }
                }
                if cells.is_empty() {
                    render(&records[index + 1..end], out)?;
                } else {
                    let mut current_row = None;
                    let mut columns = Vec::new();
                    let mut paragraph = record.paragraph;
                    for ((row, column), (text, source_paragraph)) in cells {
                        if current_row != Some(row) {
                            if current_row.is_some() {
                                out.push(&columns.join("\t"), None, paragraph)?;
                            }
                            columns.clear();
                            current_row = Some(row);
                            paragraph = source_paragraph;
                        }
                        if column as usize > out.options.max_sections {
                            return Err(limit());
                        }
                        columns.resize_with(column as usize + 1, String::new);
                        columns[column as usize] = text;
                    }
                    out.push(&columns.join("\t"), None, paragraph)?;
                }
                index = end;
                continue;
            }
            if record.tag == 67 {
                out.push(&hwp_text(record.data)?, None, record.paragraph)?;
            }
            index += 1;
        }
        Ok(())
    }
    let mut position = 0usize;
    let mut records = Vec::new();
    while position < data.len() {
        check_cancel(out.cancel)?;
        if data.len() - position < 4 {
            return Err(corrupt());
        }
        let header = u32::from_le_bytes(
            data[position..position + 4]
                .try_into()
                .map_err(|_| corrupt())?,
        );
        position += 4;
        let tag = header & 0x3ff;
        let level = (header >> 10) & 0x3ff;
        let mut length = (header >> 20) as usize;
        if length == 0xfff {
            if data.len() - position < 4 {
                return Err(corrupt());
            }
            length = u32::from_le_bytes(
                data[position..position + 4]
                    .try_into()
                    .map_err(|_| corrupt())?,
            ) as usize;
            position += 4;
        }
        let end = position
            .checked_add(length)
            .filter(|end| *end <= data.len())
            .ok_or_else(corrupt)?;
        if level as usize > out.options.max_xml_depth {
            return Err(limit());
        }
        if tag == 66 {
            out.next_paragraph();
        }
        if tag == 67 && out.paragraph == 0 {
            out.next_paragraph();
        }
        if records.len() >= out.options.max_sections.saturating_mul(20).max(100) {
            return Err(limit());
        }
        records.push(Record {
            tag,
            level,
            data: &data[position..end],
            paragraph: out.paragraph,
        });
        position = end;
    }
    render(&records, out)
}

fn xml_text(node: &mut XmlNode, text: &str) {
    node.text.push_str(text);
    // Ordered text nodes preserve mixed ODF spans, including trailing punctuation.
    if let Some(last) = node.children.last_mut().filter(|n| n.name == "#text") {
        last.text.push_str(text);
    } else {
        node.children.push(XmlNode {
            name: "#text".into(),
            text: text.into(),
            ..Default::default()
        });
    }
}
fn attr<'a>(node: &'a XmlNode, name: &str) -> Option<&'a str> {
    node.attrs
        .iter()
        .find(|(k, _)| k == name)
        .map(|(_, v)| v.as_str())
}
fn descendants<'a>(node: &'a XmlNode, name: &str) -> std::vec::IntoIter<&'a XmlNode> {
    fn walk<'a>(n: &'a XmlNode, name: &str, items: &mut Vec<&'a XmlNode>) {
        if n.name == name {
            items.push(n);
        }
        for child in &n.children {
            walk(child, name, items);
        }
    }
    let mut items = Vec::new();
    walk(node, name, &mut items);
    items.into_iter()
}
fn column_name(mut index: usize) -> String {
    let mut chars = Vec::new();
    loop {
        chars.push((b'A' + (index % 26) as u8) as char);
        if index < 26 {
            break;
        }
        index = index / 26 - 1;
    }
    chars.iter().rev().collect()
}
fn excerpt(text: &str, max: usize) -> String {
    let mut chars = text.chars();
    let result: String = chars.by_ref().take(max).collect();
    if chars.next().is_some() {
        result + "…"
    } else {
        result
    }
}
/// Each section remains below the downstream 1200-scalar chunk budget. Repeated
/// labels and row keys keep a long cell's fragments interpretable independently.
fn emit_row(
    values: &[String],
    headers: &[String],
    context: &str,
    row: u32,
    page: Option<u32>,
    out: &mut Collector<'_>,
) -> Result<()> {
    let key = values
        .iter()
        .find(|v| !v.trim().is_empty())
        .map(String::as_str)
        .unwrap_or("");
    for (column, value) in values.iter().enumerate() {
        if value.trim().is_empty() {
            continue;
        }
        let generic = format!("Column {}", column_name(column));
        let label = headers
            .get(column)
            .filter(|s| !s.trim().is_empty())
            .unwrap_or(&generic);
        let address = format!("{}{}", column_name(column), row);
        emit_cell(value, label, key, &address, context, row, page, out)?;
    }
    Ok(())
}
fn table_cells(row: &XmlNode) -> Vec<String> {
    fn cell_text(node: &XmlNode, text: &mut String) {
        if node.name == "tr" {
            if !text.is_empty() {
                text.push('\n');
            }
            text.push_str(&table_cells(node).join("\t"));
            return;
        }
        if node.name == "p" {
            if !text.is_empty() {
                text.push('\n');
            }
            xml_inline(node, text);
        }
        for child in &node.children {
            cell_text(child, text);
        }
    }
    row.children
        .iter()
        .filter(|n| n.name == "tc")
        .map(|n| {
            let mut s = String::new();
            cell_text(n, &mut s);
            s
        })
        .collect()
}
/// Align explicit HWPX addresses and OOXML column spans before applying labels.
fn aligned_table_cells(row: &XmlNode, header: bool, max: usize) -> Result<Vec<String>> {
    let raw = table_cells(row);
    let mut result = Vec::new();
    let before = descendants(row, "gridBefore")
        .next()
        .and_then(|n| attr(n, "val"))
        .unwrap_or("0")
        .parse::<usize>()
        .map_err(|_| corrupt())?;
    if before > max {
        return Err(limit());
    }
    result.resize(before, String::new());
    for (cell, value) in row.children.iter().filter(|n| n.name == "tc").zip(raw) {
        let position = cell
            .children
            .iter()
            .find(|n| n.name == "cellAddr")
            .and_then(|n| attr(n, "colAddr"))
            .map(|s| s.parse::<usize>().map_err(|_| corrupt()))
            .transpose()?
            .unwrap_or(result.len());
        if position < result.len() {
            return Err(corrupt());
        }
        let span = attr(cell, "gridSpan")
            .or_else(|| {
                cell.children
                    .iter()
                    .find(|n| n.name == "tcPr")
                    .and_then(|p| p.children.iter().find(|n| n.name == "gridSpan"))
                    .and_then(|n| attr(n, "val"))
            })
            .or_else(|| {
                cell.children
                    .iter()
                    .find(|n| n.name == "cellSpan")
                    .and_then(|n| attr(n, "colSpan"))
            })
            .unwrap_or("1")
            .parse::<usize>()
            .map_err(|_| corrupt())?;
        let end = position.checked_add(span).ok_or_else(limit)?;
        if span == 0 {
            return Err(corrupt());
        }
        if end > max {
            return Err(limit());
        }
        result.resize(position, String::new());
        result.push(value.clone());
        result.resize(end, if header { value } else { String::new() });
    }
    Ok(result)
}
struct OfficePackage<'a> {
    zip: zip::ZipArchive<Cursor<&'a [u8]>>,
    names: BTreeMap<String, usize>,
    read_bytes: u64,
}
impl<'a> OfficePackage<'a> {
    fn new(bytes: &'a [u8], out: &Collector<'_>) -> Result<Self> {
        if bytes.starts_with(&[0xd0, 0xcf, 0x11, 0xe0]) {
            let compound = cfb::CompoundFile::open(Cursor::new(bytes)).map_err(|_| corrupt())?;
            return Err(
                if compound.is_stream("/EncryptionInfo") || compound.is_stream("/EncryptedPackage")
                {
                    encrypted()
                } else {
                    corrupt()
                },
            );
        }
        let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).map_err(zip_error)?;
        if zip.len() > out.options.max_archive_entries {
            return Err(limit());
        }
        let mut names = BTreeMap::new();
        let mut total = 0u64;
        for index in 0..zip.len() {
            check_cancel(out.cancel)?;
            let entry = zip.by_index(index).map_err(zip_error)?;
            let name = entry.name().map_err(zip_error)?.into_owned();
            if entry.enclosed_name().is_none()
                || name.contains(['\\', ':', '\0'])
                || entry.unix_mode().is_some_and(|m| m & 0o170000 == 0o120000)
            {
                return Err(err(
                    ExtractErrorCode::UnsafePath,
                    "Unsafe archive member path",
                ));
            }
            if entry.encrypted() {
                return Err(encrypted());
            }
            total = total.checked_add(entry.size()).ok_or_else(limit)?;
            if total > out.options.max_expanded_bytes {
                return Err(limit());
            }
            if names.insert(name, index).is_some() {
                return Err(corrupt());
            }
        }
        Ok(Self {
            zip,
            names,
            read_bytes: 0,
        })
    }
    fn optional(&mut self, name: &str, out: &Collector<'_>) -> Result<Option<XmlNode>> {
        let Some(&index) = self.names.get(name) else {
            return Ok(None);
        };
        let input = self.zip.by_index(index).map_err(zip_error)?;
        let data = bounded_read(
            input,
            out.options
                .max_expanded_bytes
                .saturating_sub(self.read_bytes),
            out.cancel,
        )?;
        self.read_bytes = self
            .read_bytes
            .checked_add(data.len() as u64)
            .ok_or_else(limit)?;
        Ok(Some(xml_tree(&data, out)?))
    }
    fn xml(&mut self, name: &str, out: &Collector<'_>) -> Result<XmlNode> {
        self.optional(name, out)?.ok_or_else(corrupt)
    }
    fn relations(
        &mut self,
        part: &str,
        out: &Collector<'_>,
    ) -> Result<BTreeMap<String, (String, String)>> {
        let (dir, file) = part.rsplit_once('/').unwrap_or(("", part));
        let relname = if dir.is_empty() {
            format!("_rels/{file}.rels")
        } else {
            format!("{dir}/_rels/{file}.rels")
        };
        let mut result = BTreeMap::new();
        if let Some(root) = self.optional(&relname, out)? {
            for rel in descendants(&root, "Relationship") {
                if attr(rel, "TargetMode") == Some("External") {
                    continue;
                }
                let id = attr(rel, "Id").ok_or_else(corrupt)?;
                let target = resolve_part(part, attr(rel, "Target").ok_or_else(corrupt)?)?;
                if result
                    .insert(id.into(), (target, attr(rel, "Type").unwrap_or("").into()))
                    .is_some()
                {
                    return Err(corrupt());
                }
            }
        }
        Ok(result)
    }
}
fn resolve_part(base: &str, target: &str) -> Result<String> {
    if target.contains(['\\', ':', '\0', '?', '#']) {
        return Err(err(
            ExtractErrorCode::UnsafePath,
            "Unsafe package relationship",
        ));
    }
    let mut parts: Vec<&str> = if target.starts_with('/') {
        Vec::new()
    } else {
        base.rsplit_once('/')
            .map(|(dir, _)| dir.split('/').collect())
            .unwrap_or_default()
    };
    for part in target.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop().ok_or_else(|| {
                    err(ExtractErrorCode::UnsafePath, "Unsafe package relationship")
                })?;
            }
            _ => parts.push(part),
        }
    }
    if parts.is_empty() {
        return Err(corrupt());
    }
    Ok(parts.join("/"))
}
fn relationship_id(node: &XmlNode) -> Option<&str> {
    node.attrs
        .iter()
        .find(|(k, _)| k.ends_with(":id"))
        .map(|(_, v)| v.as_str())
        .or_else(|| attr(node, "id"))
}
fn extract_office(bytes: &[u8], format: &str, out: &mut Collector<'_>) -> Result<()> {
    let mut package = OfficePackage::new(bytes, out)?;
    match format {
        "pptx" => extract_pptx(&mut package, out),
        "xlsx" => extract_xlsx(&mut package, out),
        _ => extract_odf(&mut package, format, out),
    }
}
fn presentation_text(
    node: &XmlNode,
    page: u32,
    notes: bool,
    out: &mut Collector<'_>,
) -> Result<()> {
    check_cancel(out.cancel)?;
    if notes
        && node.name == "sp"
        && descendants(node, "ph")
            .any(|ph| matches!(attr(ph, "type"), Some("sldNum" | "dt" | "hdr" | "ftr")))
    {
        return Ok(());
    }
    if node.name == "tbl" {
        let rows: Vec<_> = node.children.iter().filter(|n| n.name == "tr").collect();
        let headers = rows
            .first()
            .map(|r| aligned_table_cells(r, true, out.options.max_sections))
            .transpose()?
            .unwrap_or_default();
        let explicit =
            descendants(node, "tblPr").any(|n| matches!(attr(n, "firstRow"), Some("1" | "true")));
        let context = format!(
            "{}; table header={}",
            out.heading.as_deref().unwrap_or("Slide"),
            if explicit {
                "declared"
            } else {
                "first-row inferred"
            }
        );
        for row in rows {
            let p = out.next_paragraph();
            emit_row(
                &aligned_table_cells(row, false, out.options.max_sections)?,
                &headers,
                &context,
                p,
                Some(page),
                out,
            )?;
        }
        return Ok(());
    }
    if node.name == "p" {
        let p = out.next_paragraph();
        let mut text = String::new();
        xml_inline(node, &mut text);
        out.push(&text, Some(page), p)?;
        return Ok(());
    }
    for child in &node.children {
        presentation_text(child, page, notes, out)?;
    }
    Ok(())
}
fn extract_pptx(package: &mut OfficePackage<'_>, out: &mut Collector<'_>) -> Result<()> {
    let root = package.xml("ppt/presentation.xml", out)?;
    let relationships = package.relations("ppt/presentation.xml", out)?;
    for (index, slide) in descendants(&root, "sldId").enumerate() {
        let page = u32::try_from(index + 1).map_err(|_| limit())?;
        let (path, kind) = relationships
            .get(relationship_id(slide).ok_or_else(corrupt)?)
            .ok_or_else(corrupt)?;
        if !kind.ends_with("/slide") {
            return Err(corrupt());
        }
        let slide = package.xml(path, out)?;
        out.paragraph = 0;
        out.heading = Some(format!("Slide {page}"));
        presentation_text(&slide, page, false, out)?;
        for (_, (note_path, note_type)) in package.relations(path, out)? {
            if note_type.ends_with("/notesSlide") {
                let notes = package.xml(&note_path, out)?;
                out.paragraph = 0;
                out.heading = Some(format!("Slide {page} / Notes"));
                presentation_text(&notes, page, true, out)?;
            }
        }
    }
    Ok(())
}
#[derive(Default)]
struct SheetStyles {
    formats: Vec<(u32, String)>,
}
fn sheet_styles(root: Option<&XmlNode>) -> Result<SheetStyles> {
    let Some(root) = root else {
        return Ok(SheetStyles::default());
    };
    let formats: BTreeMap<u32, String> = descendants(root, "numFmt")
        .map(|n| {
            Ok((
                attr(n, "numFmtId")
                    .ok_or_else(corrupt)?
                    .parse()
                    .map_err(|_| corrupt())?,
                attr(n, "formatCode").unwrap_or("").into(),
            ))
        })
        .collect::<Result<_>>()?;
    let mut styles = SheetStyles::default();
    if let Some(xfs) = descendants(root, "cellXfs").next() {
        for xf in xfs.children.iter().filter(|n| n.name == "xf") {
            let id = attr(xf, "numFmtId")
                .unwrap_or("0")
                .parse()
                .map_err(|_| corrupt())?;
            styles
                .formats
                .push((id, formats.get(&id).cloned().unwrap_or_default()));
        }
    }
    Ok(styles)
}
fn format_kind(id: u32, code: &str) -> &'static str {
    match id {
        9 | 10 => return "percent",
        14..=17 => return "date",
        18..=21 | 45 | 47 => return "time",
        22 => return "datetime",
        46 => return "duration",
        _ => {}
    }
    let mut clean = String::new();
    let mut quoted = false;
    let mut escaped = false;
    let mut bracket = false;
    for c in code.chars() {
        if escaped {
            escaped = false;
            continue;
        }
        if matches!(c, '\\' | '_' | '*') {
            escaped = true;
            continue;
        }
        if c == '"' {
            quoted = !quoted;
            continue;
        }
        if quoted {
            continue;
        }
        if c == '[' {
            bracket = true;
            continue;
        }
        if c == ']' {
            bracket = false;
            continue;
        }
        if bracket {
            continue;
        }
        clean.push(c.to_ascii_lowercase());
    }
    // Multi-section, conditions and elapsed-time patterns are preserved as raw
    // formats instead of pretending to implement Excel's complete renderer.
    if code.contains(';')
        || code.to_ascii_lowercase().contains("[h")
        || code.to_ascii_lowercase().contains("[m")
        || code.to_ascii_lowercase().contains("[s")
    {
        return "raw";
    }
    if clean.contains('%') {
        "percent"
    } else {
        let date = clean.contains(['y', 'd']);
        let time = clean.contains(['h', 's']);
        match (date, time) {
            (true, true) => "datetime",
            (true, false) => "date",
            (false, true) => "time",
            _ => "raw",
        }
    }
}
fn excel_number(raw: &str, style: Option<&(u32, String)>, date1904: bool) -> Result<String> {
    let Some((id, code)) = style else {
        return Ok(format!("[number] {raw}"));
    };
    let value = raw.parse::<f64>().map_err(|_| corrupt())?;
    if !value.is_finite() {
        return Err(corrupt());
    }
    let kind = format_kind(*id, code);
    let rendered = match kind {
        "percent" => format!("{}%", value * 100.0),
        "date" | "datetime" | "time" => {
            if value < 0.0 || value > 2_958_465.99999 {
                return Ok(format!("[number; formatId={id}; format={code}] {raw}"));
            }
            let whole = value.floor() as i64;
            let seconds = ((value - value.floor()) * 86400.0).round() as i64;
            let carry = seconds / 86400;
            let seconds = seconds % 86400;
            let time = format!(
                "{:02}:{:02}:{:02}",
                seconds / 3600,
                (seconds % 3600) / 60,
                seconds % 60
            );
            if kind == "time" {
                time
            } else {
                let day = whole + carry;
                let date = if !date1904 && day == 60 {
                    "1900-02-29 [Excel compatibility day]".into()
                } else {
                    let base = chrono::NaiveDate::from_ymd_opt(
                        if date1904 { 1904 } else { 1899 },
                        if date1904 { 1 } else { 12 },
                        if date1904 { 1 } else { 31 },
                    )
                    .ok_or_else(corrupt)?;
                    let days = if !date1904 && day > 60 { day - 1 } else { day };
                    base.checked_add_signed(chrono::Duration::days(days))
                        .ok_or_else(corrupt)?
                        .format("%Y-%m-%d")
                        .to_string()
                };
                if kind == "datetime" {
                    format!("{date}T{time} [timezone unspecified]")
                } else {
                    date
                }
            }
        }
        "duration" => format!("{} hours", value * 24.0),
        _ => return Ok(format!("[number; formatId={id}; format={code}] {raw}")),
    };
    Ok(format!("[{kind}; raw={raw}] {rendered}"))
}
fn cell_reference(reference: &str) -> Result<(usize, u32)> {
    let reference = reference.trim_matches('$');
    let mut column = 0usize;
    let mut split = 0;
    for (i, c) in reference.char_indices() {
        if c.is_ascii_uppercase() {
            column = column
                .checked_mul(26)
                .and_then(|v| v.checked_add((c as u8 - b'A' + 1) as usize))
                .ok_or_else(limit)?;
            split = i + 1;
        } else {
            break;
        }
    }
    if column == 0 || column > 16384 {
        return Err(corrupt());
    }
    let row = reference
        .get(split..)
        .ok_or_else(corrupt)?
        .trim_start_matches('$')
        .parse::<u32>()
        .map_err(|_| corrupt())?;
    if row == 0 || row > 1_048_576 {
        return Err(corrupt());
    }
    Ok((column - 1, row))
}
fn cell_value(
    cell: &XmlNode,
    strings: &[String],
    styles: &SheetStyles,
    date1904: bool,
) -> Result<(String, String)> {
    let kind = attr(cell, "t").unwrap_or("n");
    let raw = cell
        .children
        .iter()
        .find(|n| n.name == "v")
        .map(|n| n.text.as_str());
    let formula = cell.children.iter().any(|n| n.name == "f");
    let (plain, rendered) = match kind {
        "s" => {
            let index = raw
                .ok_or_else(corrupt)?
                .parse::<usize>()
                .map_err(|_| corrupt())?;
            let s = strings.get(index).ok_or_else(corrupt)?.clone();
            (s.clone(), format!("[text] {s}"))
        }
        "inlineStr" => {
            let mut text = String::new();
            if let Some(inline) = cell.children.iter().find(|n| n.name == "is") {
                xml_inline(inline, &mut text);
            }
            (text.clone(), format!("[text] {text}"))
        }
        "str" => {
            let s = raw.unwrap_or("").to_owned();
            (s.clone(), format!("[text] {s}"))
        }
        "b" => {
            let s = match raw {
                Some("1") => "true",
                Some("0") => "false",
                _ => return Err(corrupt()),
            }
            .to_owned();
            (s.clone(), format!("[boolean] {s}"))
        }
        "e" => {
            let s = raw.unwrap_or("").to_owned();
            (s.clone(), format!("[error] {s}"))
        }
        "d" => {
            let s = raw.unwrap_or("").to_owned();
            (s.clone(), format!("[ISO date/time] {s}"))
        }
        "n" => {
            let s = raw.unwrap_or("").to_owned();
            let style = attr(cell, "s")
                .map(|s| s.parse::<usize>().map_err(|_| corrupt()))
                .transpose()?
                .map(|i| styles.formats.get(i).ok_or_else(corrupt))
                .transpose()?;
            let display = if s.is_empty() {
                String::new()
            } else {
                excel_number(&s, style, date1904)?
            };
            (s, display)
        }
        _ => return Err(corrupt()),
    };
    Ok((
        plain,
        if formula {
            if raw.is_none() {
                "[formula; cached result unavailable; not recalculated]".into()
            } else {
                format!("[formula cached result; not recalculated] {rendered}")
            }
        } else {
            rendered
        },
    ))
}
struct DeclaredSheetTable {
    first_col: usize,
    last_col: usize,
    first_row: u32,
    last_row: u32,
    headers: Vec<String>,
    name: String,
}
fn extract_xlsx(package: &mut OfficePackage<'_>, out: &mut Collector<'_>) -> Result<()> {
    let root = package.xml("xl/workbook.xml", out)?;
    let relations = package.relations("xl/workbook.xml", out)?;
    let date1904 =
        descendants(&root, "workbookPr").any(|n| matches!(attr(n, "date1904"), Some("1" | "true")));
    let strings = if let Some(strings) = package.optional("xl/sharedStrings.xml", out)? {
        descendants(&strings, "si")
            .map(|n| {
                let mut s = String::new();
                xml_inline(n, &mut s);
                s
            })
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };
    let style_root = package.optional("xl/styles.xml", out)?;
    let styles = sheet_styles(style_root.as_ref())?;
    for sheet in descendants(&root, "sheet") {
        let name = attr(sheet, "name").ok_or_else(corrupt)?;
        let (path, kind) = relations
            .get(relationship_id(sheet).ok_or_else(corrupt)?)
            .ok_or_else(corrupt)?;
        if !kind.ends_with("/worksheet") {
            return Err(corrupt());
        }
        let xml = package.xml(path, out)?;
        let sheet_relations = package.relations(path, out)?;
        let mut tables = Vec::new();
        for table in descendants(&xml, "tablePart") {
            let (target, kind) = sheet_relations
                .get(relationship_id(table).ok_or_else(corrupt)?)
                .ok_or_else(corrupt)?;
            if !kind.ends_with("/table") {
                return Err(corrupt());
            }
            let table_xml = package.xml(target, out)?;
            let table = descendants(&table_xml, "table")
                .next()
                .ok_or_else(corrupt)?;
            if attr(table, "headerRowCount") == Some("0") {
                continue;
            }
            let range = attr(table, "ref").ok_or_else(corrupt)?;
            let (start, end) = range.split_once(':').unwrap_or((range, range));
            let (a, r1) = cell_reference(start)?;
            let (b, r2) = cell_reference(end)?;
            let headers = descendants(table, "tableColumn")
                .map(|n| attr(n, "name").unwrap_or("").to_owned())
                .collect();
            if b < a || r2 < r1 {
                return Err(corrupt());
            }
            tables.push(DeclaredSheetTable {
                first_col: a,
                last_col: b,
                first_row: r1,
                last_row: r2,
                headers,
                name: attr(table, "displayName").unwrap_or("Table").into(),
            });
        }
        let mut inferred = Vec::new();
        let mut next_row = 1u32;
        for row in descendants(&xml, "row") {
            check_cancel(out.cancel)?;
            let row_index = attr(row, "r")
                .map(|s| s.parse().map_err(|_| corrupt()))
                .transpose()?
                .unwrap_or(next_row);
            if row_index == 0 || row_index > 1_048_576 {
                return Err(corrupt());
            }
            next_row = row_index.checked_add(1).ok_or_else(limit)?;
            let mut values = Vec::new();
            let mut plain = Vec::new();
            let mut next_column = 0usize;
            for cell in row.children.iter().filter(|n| n.name == "c") {
                let column = if let Some(reference) = attr(cell, "r") {
                    let (c, r) = cell_reference(reference)?;
                    if r != row_index {
                        return Err(corrupt());
                    }
                    c
                } else {
                    next_column
                };
                if column >= 16384 || column < next_column {
                    return Err(corrupt());
                }
                next_column = column + 1;
                let (p, v) = cell_value(cell, &strings, &styles, date1904)?;
                values.resize(next_column, String::new());
                plain.resize(next_column, String::new());
                values[column] = v;
                plain[column] = p;
            }
            if plain.iter().all(|s| s.trim().is_empty())
                && values.iter().all(|s| s.trim().is_empty())
            {
                continue;
            }
            if inferred.is_empty() {
                inferred = plain.clone();
            }
            // Emit separate fields to preserve sparse A1 addresses and per-table headers.
            for column in 0..values.len() {
                if values[column].is_empty() {
                    continue;
                }
                let table = tables.iter().find(|t| {
                    column >= t.first_col
                        && column <= t.last_col
                        && row_index >= t.first_row
                        && row_index <= t.last_row
                });
                let generic = format!("Column {}", column_name(column));
                let label = if let Some(t) = table {
                    t.headers.get(column - t.first_col)
                } else {
                    inferred.get(column)
                }
                .filter(|s| !s.is_empty())
                .unwrap_or(&generic);
                let context = if let Some(t) = table {
                    format!("Sheet {name}; table={}; header=declared", t.name)
                } else {
                    format!("Sheet {name}; header=first-row inferred")
                };
                let key = plain
                    .iter()
                    .find(|s| !s.is_empty())
                    .map(String::as_str)
                    .unwrap_or("");
                let address = format!("{}{}", column_name(column), row_index);
                emit_cell(
                    &values[column],
                    label,
                    key,
                    &address,
                    &context,
                    row_index,
                    None,
                    out,
                )?;
            }
        }
        // Merged ranges are metadata, not duplicated invented values.
        for merged in descendants(&xml, "mergeCell") {
            let range = attr(merged, "ref").ok_or_else(corrupt)?;
            let (start, end) = range.split_once(':').ok_or_else(corrupt)?;
            let (_, r) = cell_reference(start)?;
            cell_reference(end)?;
            out.heading = Some(format!("Sheet {name}; merged cells {range}"));
            out.push(
                &format!("Merged range {range}; value belongs to anchor {start}"),
                None,
                r,
            )?;
        }
        out.heading = None;
    }
    Ok(())
}
fn odf_inline(node: &XmlNode, text: &mut String, out: &Collector<'_>) -> Result<()> {
    check_cancel(out.cancel)?;
    match node.name.as_str() {
        "#text" => {
            text.push_str(&node.text);
            return Ok(());
        }
        "s" => {
            let count = repeat_count(node, "c")?;
            let remaining = out.options.max_text_bytes.saturating_sub(text.len());
            if count as u64 > remaining as u64 {
                return Err(limit());
            }
            text.extend(std::iter::repeat_n(' ', count as usize));
            return Ok(());
        }
        "tab" => {
            text.push('\t');
            return Ok(());
        }
        "line-break" => {
            text.push('\n');
            return Ok(());
        }
        "annotation" | "tracked-changes" | "binary-data" | "scripts" | "script" => return Ok(()),
        _ => {}
    }
    for child in &node.children {
        odf_inline(child, text, out)?;
        if text.len() > out.options.max_text_bytes {
            return Err(limit());
        }
    }
    Ok(())
}
fn odf_cell_text(node: &XmlNode, out: &Collector<'_>) -> Result<String> {
    let mut result = String::new();
    for child in &node.children {
        if matches!(child.name.as_str(), "p" | "h") {
            if !result.is_empty() {
                result.push('\n');
            }
            odf_inline(child, &mut result, out)?;
        }
        // Nested tables have their own explicit boundary, rather than merging cells.
        else if child.name == "table" {
            result.push_str("\n[nested table]\n");
            for row in descendants(child, "table-row") {
                let mut cells = Vec::new();
                for cell in row.children.iter().filter(|n| n.name == "table-cell") {
                    cells.push(odf_cell_text(cell, out)?);
                }
                result.push_str(&cells.join("\t"));
                result.push('\n');
            }
        }
    }
    Ok(result)
}
fn repeat_count(node: &XmlNode, key: &str) -> Result<u32> {
    let count = attr(node, key)
        .unwrap_or("1")
        .parse::<u32>()
        .map_err(|_| corrupt())?;
    if count == 0 {
        return Err(corrupt());
    }
    Ok(count)
}
fn odf_value(cell: &XmlNode, out: &Collector<'_>) -> Result<(String, String)> {
    let display = odf_cell_text(cell, out)?;
    let kind = attr(cell, "value-type").unwrap_or("string");
    let raw = match kind {
        "float" | "percentage" | "currency" => attr(cell, "value"),
        "date" => attr(cell, "date-value"),
        "time" => attr(cell, "time-value"),
        "boolean" => attr(cell, "boolean-value"),
        "string" => attr(cell, "string-value"),
        "void" => None,
        _ => return Err(corrupt()),
    };
    let plain = if display.is_empty() {
        raw.unwrap_or("").to_owned()
    } else {
        display
    };
    if plain.is_empty() && attr(cell, "formula").is_none() {
        return Ok((String::new(), String::new()));
    }
    let detail = if let Some(raw) = raw {
        format!(
            "[{kind}; raw={raw}{}] {plain}",
            attr(cell, "currency")
                .map(|s| format!("; currency={s}"))
                .unwrap_or_default()
        )
    } else {
        format!("[{kind}] {plain}")
    };
    Ok((
        plain,
        if attr(cell, "formula").is_some() {
            format!("[formula cached result; not recalculated] {detail}")
        } else {
            detail
        },
    ))
}
fn emit_cell(
    value: &str,
    label: &str,
    key: &str,
    address: &str,
    context: &str,
    row: u32,
    page: Option<u32>,
    out: &mut Collector<'_>,
) -> Result<()> {
    if value.trim().is_empty() {
        return Ok(());
    }
    let previous = out.heading.clone();
    out.heading = Some(format!(
        "{context}; row={row}; cell={address}; column={label}; row key={}",
        excerpt(key, 120)
    ));
    let prefix = format!(
        "[row {row}; key={}; cell={address}; {}]: ",
        excerpt(key, 80),
        excerpt(label, 100)
    );
    let chars: Vec<char> = value.chars().collect();
    let size = 1000usize.saturating_sub(prefix.chars().count()).max(200);
    let mut start = 0;
    loop {
        let end = (start + size).min(chars.len());
        let part: String = chars[start..end].iter().collect();
        out.push(&(prefix.clone() + &part), page, row)?;
        if end == chars.len() {
            break;
        }
        start = end.saturating_sub(150);
    }
    out.heading = previous;
    Ok(())
}
fn odf_table(table: &XmlNode, page: Option<u32>, out: &mut Collector<'_>) -> Result<()> {
    fn rows<'a>(node: &'a XmlNode, header: bool, items: &mut Vec<(&'a XmlNode, bool)>) {
        for child in &node.children {
            match child.name.as_str() {
                "table-row" => items.push((child, header)),
                "table" => {}
                _ => rows(child, header || child.name == "table-header-rows", items),
            }
        }
    }
    let mut source_rows = Vec::new();
    rows(table, false, &mut source_rows);
    let mut headers: BTreeMap<usize, String> = BTreeMap::new();
    let explicit = source_rows.iter().any(|(_, h)| *h);
    let mut row_index = 1u32;
    let name = attr(table, "name").unwrap_or("Table");
    let previous = out.heading.clone();
    let context = format!(
        "{} / Sheet/table {name}; header={}",
        previous.as_deref().unwrap_or("Document"),
        if explicit {
            "declared"
        } else {
            "first-row inferred"
        }
    );
    for (row, is_header) in source_rows {
        check_cancel(out.cancel)?;
        let repeat = repeat_count(row, "number-rows-repeated")?;
        let next = row_index.checked_add(repeat).ok_or_else(limit)?;
        let mut cells: Vec<(usize, String, String)> = Vec::new();
        let mut column = 0usize;
        for cell in row
            .children
            .iter()
            .filter(|n| matches!(n.name.as_str(), "table-cell" | "covered-table-cell"))
        {
            let count = repeat_count(cell, "number-columns-repeated")? as usize;
            let end = column.checked_add(count).ok_or_else(limit)?;
            if cell.name == "table-cell" {
                let (plain, value) = odf_value(cell, out)?;
                if !value.is_empty() {
                    if count > out.options.max_sections
                        || cells.len().saturating_add(count) > out.options.max_sections
                    {
                        return Err(limit());
                    }
                    for c in column..end {
                        cells.push((c, plain.clone(), value.clone()));
                    }
                }
            }
            column = end;
        }
        // Large empty repeat tails are a logical position, not expanded allocations.
        if !cells.is_empty() {
            if repeat as usize > out.options.max_sections.saturating_sub(out.sections.len()) {
                return Err(limit());
            }
            if is_header || (!explicit && headers.is_empty()) {
                for (c, plain, _) in &cells {
                    let entry = headers.entry(*c).or_default();
                    if !entry.is_empty() && entry != plain {
                        entry.push_str(" / ");
                    }
                    if !entry.ends_with(plain) {
                        entry.push_str(plain);
                    }
                }
            }
            let key = cells.first().map(|(_, p, _)| p.as_str()).unwrap_or("");
            for row_number in row_index..next {
                for (c, _, value) in &cells {
                    let generic = format!("Column {}", column_name(*c));
                    let label = headers.get(c).filter(|v| !v.is_empty()).unwrap_or(&generic);
                    let address = format!("{}{}", column_name(*c), row_number);
                    emit_cell(value, label, key, &address, &context, row_number, page, out)?;
                }
            }
        }
        row_index = next;
    }
    out.heading = previous;
    Ok(())
}
fn odf_body(node: &XmlNode, page: Option<u32>, out: &mut Collector<'_>) -> Result<()> {
    check_cancel(out.cancel)?;
    match node.name.as_str() {
        "annotation" | "tracked-changes" | "binary-data" | "scripts" | "script" => return Ok(()),
        "table" => return odf_table(node, page, out),
        "p" | "h" => {
            let mut text = String::new();
            odf_inline(node, &mut text, out)?;
            let p = out.next_paragraph();
            if node.name == "h" {
                out.heading = Some(text.clone());
            }
            out.push(&text, page, p)?;
            return Ok(());
        }
        _ => {}
    }
    for child in &node.children {
        odf_body(child, page, out)?;
    }
    Ok(())
}
fn extract_odf(
    package: &mut OfficePackage<'_>,
    format: &str,
    out: &mut Collector<'_>,
) -> Result<()> {
    if let Some(manifest) = package.optional("META-INF/manifest.xml", out)? {
        if descendants(&manifest, "encryption-data").next().is_some() {
            return Err(encrypted());
        }
    }
    let root = package.xml("content.xml", out)?;
    let body = descendants(&root, "body").next().ok_or_else(corrupt)?;
    if format == "odp" {
        let presentation = body
            .children
            .iter()
            .find(|n| n.name == "presentation")
            .ok_or_else(corrupt)?;
        for (index, page) in presentation
            .children
            .iter()
            .filter(|n| n.name == "page")
            .enumerate()
        {
            let page_number = u32::try_from(index + 1).map_err(|_| limit())?;
            out.paragraph = 0;
            out.heading = Some(format!(
                "Slide {page_number}; {}",
                attr(page, "name").unwrap_or("")
            ));
            for child in &page.children {
                if child.name == "notes" {
                    out.heading = Some(format!("Slide {page_number} / Notes"));
                    odf_body(child, Some(page_number), out)?;
                    out.heading = Some(format!("Slide {page_number}"));
                } else {
                    odf_body(child, Some(page_number), out)?;
                }
            }
        }
        Ok(())
    } else {
        let expected = if format == "ods" {
            "spreadsheet"
        } else {
            "text"
        };
        let document = body
            .children
            .iter()
            .find(|n| n.name == expected)
            .ok_or_else(corrupt)?;
        odf_body(document, None, out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn extract(name: &str, bytes: &[u8]) -> Result<ExtractedDocument> {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(name);
        fs::write(&path, bytes).unwrap();
        extract_document(&path, &ExtractOptions::default(), &AtomicBool::new(false))
    }
    fn zip_file(parts: &[(&str, &str)]) -> Vec<u8> {
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for (name, text) in parts {
            writer
                .start_file(*name, zip::write::SimpleFileOptions::default())
                .unwrap();
            writer.write_all(text.as_bytes()).unwrap();
        }
        writer.finish().unwrap().into_inner()
    }
    fn utf16(text: &str) -> Vec<u8> {
        text.encode_utf16().flat_map(u16::to_le_bytes).collect()
    }
    fn record(tag: u32, level: u32, data: &[u8]) -> Vec<u8> {
        let mut bytes = ((data.len() as u32) << 20 | level << 10 | tag)
            .to_le_bytes()
            .to_vec();
        bytes.extend_from_slice(data);
        bytes
    }
    fn hwp_file(flags: u32, records: &[u8]) -> Vec<u8> {
        let mut compound = cfb::CompoundFile::create(Cursor::new(Vec::new())).unwrap();
        let mut header = vec![0u8; 256];
        header[..17].copy_from_slice(b"HWP Document File");
        header[35] = 5;
        header[36..40].copy_from_slice(&flags.to_le_bytes());
        compound
            .create_stream("/FileHeader")
            .unwrap()
            .write_all(&header)
            .unwrap();
        compound.create_storage("/BodyText").unwrap();
        let body = if flags & 1 != 0 {
            let mut encoder =
                flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
            encoder.write_all(records).unwrap();
            encoder.finish().unwrap()
        } else {
            records.to_vec()
        };
        compound
            .create_stream("/BodyText/Section0")
            .unwrap()
            .write_all(&body)
            .unwrap();
        compound.into_inner().into_inner()
    }
    fn pdf(pages: &[&str]) -> Vec<u8> {
        // Self-authored, tiny valid PDF fixture with classic cross-reference table.
        let mut objects = vec![
            "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
            String::new(),
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_string(),
        ];
        let mut kids = Vec::new();
        for page in pages {
            let page_id = objects.len() + 1;
            let stream_id = page_id + 1;
            kids.push(format!("{page_id} 0 R"));
            objects.push(format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 3 0 R >> >> /Contents {stream_id} 0 R >>"));
            let stream = if page.is_empty() {
                String::new()
            } else {
                format!("BT /F1 12 Tf 72 720 Td ({page}) Tj ET")
            };
            objects.push(format!(
                "<< /Length {} >>\nstream\n{stream}\nendstream",
                stream.len()
            ));
        }
        objects[1] = format!(
            "<< /Type /Pages /Count {} /Kids [{}] >>",
            pages.len(),
            kids.join(" ")
        );
        let mut bytes = b"%PDF-1.4\n".to_vec();
        let mut offsets = vec![0];
        for (index, object) in objects.iter().enumerate() {
            offsets.push(bytes.len());
            bytes.extend_from_slice(format!("{} 0 obj\n{object}\nendobj\n", index + 1).as_bytes());
        }
        let xref = bytes.len();
        bytes.extend_from_slice(
            format!("xref\n0 {}\n0000000000 65535 f \n", offsets.len()).as_bytes(),
        );
        for offset in &offsets[1..] {
            bytes.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
        }
        bytes.extend_from_slice(
            format!(
                "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
                offsets.len()
            )
            .as_bytes(),
        );
        bytes
    }

    #[test]
    fn korean_utf8_fingerprint_and_original_line_location() {
        let bytes = "# 연구\n\n분기 매출: 42\n".as_bytes();
        let doc = extract("sample.md", bytes).unwrap();
        assert_eq!(
            doc.fingerprint.sha256,
            format!("{:x}", Sha256::digest(bytes))
        );
        assert_eq!(doc.fingerprint.size_bytes, bytes.len() as u64);
        assert_eq!(doc.sections[1].paragraph, Some(3));
        assert_eq!(doc.sections[1].heading.as_deref(), Some("연구"));
        assert_eq!(doc.sections[1].text, "분기 매출: 42");
        assert!(doc.fingerprint.modified_unix_ms.is_some());
    }
    #[test]
    fn unicode_and_cp949_decoding() {
        let text = "한국어 확장 똠\n둘째 줄";
        for bom in [vec![0xff, 0xfe], vec![0xfe, 0xff]] {
            let le = bom[0] == 0xff;
            let mut bytes = bom;
            for unit in text.encode_utf16() {
                bytes.extend_from_slice(&if le {
                    unit.to_le_bytes()
                } else {
                    unit.to_be_bytes()
                });
            }
            assert_eq!(
                extract("sample.txt", &bytes).unwrap().sections[0].text,
                "한국어 확장 똠"
            );
        }
        let (bytes, _, bad) = EUC_KR.encode(text);
        assert!(!bad);
        assert_eq!(
            extract("sample.txt", &bytes).unwrap().sections[0].text,
            "한국어 확장 똠"
        );
    }
    #[test]
    fn html_excludes_active_content_and_keeps_cells() {
        let doc = extract("sample.html", b"<html><head><title>secret title</title></head><body><p>A &amp; B</p><script>secret script</script><style>secret style</style><table><tr><td>Name</td><td>Value</td></tr></table></body></html>").unwrap();
        assert!(doc.sections.iter().any(|s| s.text == "A & B"));
        assert!(doc.sections.iter().any(|s| s.text.contains("Name\tValue")));
        assert!(!doc.sections.iter().any(|s| s.text.contains("secret")));
    }
    #[test]
    fn docx_runs_entities_headings_and_table_rows() {
        let bytes = zip_file(&[(
            "word/document.xml",
            r#"<w:document xmlns:w="urn:test"><w:body><w:p><w:pPr><w:pStyle w:val="Heading1"/></w:pPr><w:r><w:t>Summary</w:t></w:r></w:p><w:p><w:r><w:t>A&amp;B</w:t></w:r><w:r><w:t> costs 42</w:t></w:r></w:p><w:tbl><w:tr><w:tc><w:p><w:r><w:t>Item</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>Cost</w:t></w:r></w:p></w:tc></w:tr></w:tbl></w:body></w:document>"#,
        )]);
        let doc = extract("sample.docx", &bytes).unwrap();
        assert_eq!(doc.sections.len(), 4);
        assert_eq!(doc.sections[1].text, "A&B costs 42");
        assert_eq!(doc.sections[1].heading.as_deref(), Some("Summary"));
        assert!(doc.sections[2].text.ends_with(": Item"));
        assert!(doc.sections[3].text.ends_with(": Cost"));
        assert!(doc.sections[3]
            .heading
            .as_deref()
            .unwrap()
            .contains("column=Cost"));
    }
    #[test]
    fn hwpx_sections_sort_numerically_and_keep_nested_table() {
        let bytes = zip_file(&[
            ("Contents/section10.xml", "<sec><p><run><t>tenth</t></run></p></sec>"),
            ("Contents/section2.xml", "<sec><p><run><t>second</t><tbl><tr><tc><subList><p><run><t>A</t></run></p></subList></tc><tc><subList><p><run><t>B</t></run></p></subList></tc></tr></tbl></run></p></sec>"),
        ]);
        let doc = extract("sample.hwpx", &bytes).unwrap();
        assert_eq!(doc.sections.first().unwrap().text, "second");
        assert_eq!(doc.sections.last().unwrap().text, "tenth");
        assert!(doc.sections.iter().any(|s| s.text.ends_with(": A")));
        assert!(doc.sections.iter().any(|s| s.text.ends_with(": B")));
        assert!(doc.sections.iter().all(|s| s.page.is_none()));
    }
    #[test]
    fn pdf_preserves_actual_page_numbers() {
        let doc = extract("sample.pdf", &pdf(&["First page", "", "Third page"])).unwrap();
        assert_eq!(doc.sections[0].page, Some(1));
        assert_eq!(doc.sections.last().unwrap().page, Some(3));
        assert!(doc.sections.last().unwrap().text.contains("Third page"));
    }
    #[test]
    fn textless_pdf_is_ocr_required_and_broken_pdf_is_corrupt() {
        assert_eq!(
            extract("scan.pdf", &pdf(&[""])).unwrap_err().code,
            ExtractErrorCode::OcrRequired
        );
        assert_eq!(
            extract("broken.pdf", b"%PDF-1.4 garbage").unwrap_err().code,
            ExtractErrorCode::CorruptDocument
        );
    }

    #[test]
    fn disabled_ocr_does_not_read_images_and_keeps_typed_pdf_pages() {
        let missing = std::env::temp_dir().join("prism-ocr-never-created.png");
        assert_eq!(
            extract_document(
                &missing,
                &ExtractOptions::default(),
                &AtomicBool::new(false)
            )
            .unwrap_err()
            .code,
            ExtractErrorCode::OcrRequired
        );
        let document = extract(
            "mixed.pdf",
            include_bytes!("../../tests/fixtures/ocr/mixed.pdf"),
        )
        .unwrap();
        assert_eq!(document.sections.len(), 2);
        assert_eq!(document.sections[0].page, Some(1));
        assert!(document.sections[0]
            .text
            .contains("typed original page one"));
        assert_eq!(document.sections[1].page, Some(3));
        assert!(document.sections[1]
            .text
            .contains("typed original page three"));
    }

    /// Opt-in integration: real installed OS engine, public independent pixels,
    /// no substitute recognizer and no network or user documents.
    #[cfg(windows)]
    #[test]
    #[ignore = "requires Windows native OCR with the installed Korean language pack"]
    fn windows_native_ocr_pixels_pdf_passwords_and_resource_errors() {
        let capabilities = crate::ocr::capabilities();
        assert!(
            capabilities.available && capabilities.pdf_available,
            "{capabilities:?}"
        );
        assert!(capabilities
            .languages
            .iter()
            .any(|l| l.tag.split('-').next() == Some("ko")));
        let dir = tempfile::tempdir().unwrap();
        let mut options = ExtractOptions::default();
        options.ocr.enabled = true;
        options.ocr.language = "ko".into();
        let cancel = AtomicBool::new(false);
        for (name, bytes, expected_page) in [
            (
                "bilingual.png",
                include_bytes!("../../tests/fixtures/ocr/bilingual.png").as_slice(),
                1,
            ),
            (
                "transparent.png",
                include_bytes!("../../tests/fixtures/ocr/transparent.png").as_slice(),
                1,
            ),
            (
                "scan.pdf",
                include_bytes!("../../tests/fixtures/ocr/scan.pdf").as_slice(),
                1,
            ),
            (
                "mixed.pdf",
                include_bytes!("../../tests/fixtures/ocr/mixed.pdf").as_slice(),
                2,
            ),
            (
                "scan-locked.pdf",
                include_bytes!("../../tests/fixtures/ocr/scan-locked.pdf").as_slice(),
                1,
            ),
        ] {
            let path = dir.path().join(name);
            fs::write(&path, bytes).unwrap();
            let passwords = if name == "scan-locked.pdf" {
                vec![Some("ocr-fixture-password"), Some("ocr-fixture-owner")]
            } else {
                vec![None]
            };
            for password in passwords {
                println!(
                    "OCR public fixture: {name}, owner={}",
                    password == Some("ocr-fixture-owner")
                );
                let document = match password {
                    Some(password) => {
                        extract_document_with_password(&path, &options, &cancel, password)
                    }
                    None => extract_document(&path, &options, &cancel),
                }
                .unwrap();
                println!("OCR recognized public fixture: {name}");
                let text: String = document
                    .sections
                    .iter()
                    .filter(|s| s.page == Some(expected_page))
                    .flat_map(|s| s.text.chars())
                    .filter(|c| !c.is_whitespace())
                    .flat_map(char::to_lowercase)
                    .collect();
                assert!(text.contains("prismlocalsearch74829"), "{name}: {text}");
                assert!(text.contains("프리즘문서검색74829"), "{name}: {text}");
                assert_eq!(
                    document.fingerprint.sha256,
                    format!("{:x}", Sha256::digest(bytes))
                );
                assert_eq!(fs::read(&path).unwrap(), bytes);
                assert!(document.sections.windows(2).all(|p| p[0].page <= p[1].page));
                if name == "mixed.pdf" {
                    assert!(document.sections[0]
                        .text
                        .contains("typed original page one"));
                    assert!(
                        document
                            .sections
                            .iter()
                            .any(|s| s.page == Some(3)
                                && s.text.contains("typed original page three"))
                    );
                    assert!(document.sections.iter().all(|s| s.page != Some(4)));
                }
            }
        }
        let locked = dir.path().join("scan-locked.pdf");
        assert_eq!(
            extract_document_with_password(&locked, &options, &cancel, "wrong")
                .unwrap_err()
                .code,
            ExtractErrorCode::PasswordRejected
        );
        assert_eq!(
            fs::read_dir(dir.path()).unwrap().count(),
            5,
            "no decrypted PDF file may be written"
        );
        let image = dir.path().join("bilingual.png");
        options.ocr.language = "zz-Unknown".into();
        assert_eq!(
            extract_document(&image, &options, &cancel)
                .unwrap_err()
                .code,
            ExtractErrorCode::OcrLanguageUnavailable
        );
        options.ocr.language = "ko".into();
        options.ocr_limits.max_source_pixels = 1;
        assert_eq!(
            extract_document(&image, &options, &cancel)
                .unwrap_err()
                .code,
            ExtractErrorCode::LimitExceeded
        );
        options.ocr_limits = crate::ocr::Limits::default();
        options.ocr_limits.max_text_bytes = 3;
        assert_eq!(
            extract_document(&image, &options, &cancel)
                .unwrap_err()
                .code,
            ExtractErrorCode::LimitExceeded
        );
        options.ocr_limits = crate::ocr::Limits::default();
        options.ocr_limits.max_duration = std::time::Duration::from_nanos(1);
        assert_eq!(
            extract_document(&image, &options, &cancel)
                .unwrap_err()
                .code,
            ExtractErrorCode::OcrTimedOut
        );
    }
    #[test]
    fn hwp_compressed_and_plain_body_and_inline_controls() {
        let mut text = utf16("한글");
        let mut control = [0u16; 8];
        control[0] = 9;
        control[7] = 9;
        text.extend(control.into_iter().flat_map(u16::to_le_bytes));
        text.extend(utf16("값\r"));
        let mut records = record(66, 0, &[0u8; 24]);
        records.extend(record(67, 1, &text));
        for flags in [0, 1] {
            let doc = extract("sample.hwp", &hwp_file(flags, &records)).unwrap();
            assert_eq!(doc.sections[0].text, "한글\t값");
            assert_eq!(doc.sections[0].paragraph, Some(1));
        }
    }
    #[test]
    fn hwp_table_cells_become_tab_separated_rows() {
        let mut records = record(66, 0, &[0u8; 24]);
        records.extend(record(71, 1, b" lbt"));
        records.extend(record(77, 2, &[0u8; 24]));
        for (row, col, text) in [
            (0u16, 0u16, "Item"),
            (0, 1, "Cost"),
            (1, 0, "Tea"),
            (1, 1, "42"),
        ] {
            let mut cell = [0u8; 34];
            cell[8..10].copy_from_slice(&col.to_le_bytes());
            cell[10..12].copy_from_slice(&row.to_le_bytes());
            records.extend(record(72, 2, &cell));
            records.extend(record(66, 3, &[0u8; 24]));
            records.extend(record(67, 4, &utf16(text)));
        }
        let doc = extract("table.hwp", &hwp_file(1, &records)).unwrap();
        assert_eq!(
            doc.sections
                .iter()
                .map(|s| s.text.as_str())
                .collect::<Vec<_>>(),
            ["Item\tCost", "Tea\t42"]
        );
        assert_eq!(doc.sections[0].paragraph, Some(2));
    }
    #[test]
    fn hwp_protection_flags_and_damaged_controls_are_separate() {
        for flags in [2, 4, 16, 256, 1024] {
            assert_eq!(
                extract("protected.hwp", &hwp_file(flags, &[]))
                    .unwrap_err()
                    .code,
                ExtractErrorCode::Encrypted
            );
        }
        let records = record(67, 1, &[9, 0]);
        assert_eq!(
            extract("damaged.hwp", &hwp_file(0, &records))
                .unwrap_err()
                .code,
            ExtractErrorCode::CorruptDocument
        );
        assert_eq!(
            extract("damaged.hwp", &hwp_file(0, &[0xff; 7]))
                .unwrap_err()
                .code,
            ExtractErrorCode::CorruptDocument
        );
    }
    #[test]
    fn malformed_xml_and_dtd_are_rejected_without_leaking_text() {
        for xml in [
            "<document><p>private text</document>",
            "<!DOCTYPE foo [<!ENTITY x SYSTEM 'file:///secret'>]><document><p>&x;</p></document>",
            "<!DOCTYPE root [<!ENTITY broken",
            "private text<document/>",
        ] {
            let error =
                extract("broken.docx", &zip_file(&[("word/document.xml", xml)])).unwrap_err();
            assert_eq!(error.code, ExtractErrorCode::CorruptDocument);
            assert!(!error.message.contains("private"));
        }
    }
    #[test]
    fn resource_budgets_and_unsupported_format_are_distinct() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("huge.txt");
        fs::write(&path, "too long").unwrap();
        let options = ExtractOptions {
            max_file_bytes: 3,
            ..ExtractOptions::default()
        };
        assert_eq!(
            extract_document(&path, &options, &AtomicBool::new(false))
                .unwrap_err()
                .code,
            ExtractErrorCode::LimitExceeded
        );
        let options = ExtractOptions {
            max_text_bytes: 3,
            ..ExtractOptions::default()
        };
        assert_eq!(
            extract_document(&path, &options, &AtomicBool::new(false))
                .unwrap_err()
                .code,
            ExtractErrorCode::LimitExceeded
        );
        assert_eq!(
            extract("unknown.bin", b"text").unwrap_err().code,
            ExtractErrorCode::UnsupportedFormat
        );
        assert_eq!(
            extract("binary.txt", &[0, 1, 2]).unwrap_err().code,
            ExtractErrorCode::CorruptDocument
        );
    }
    #[test]
    fn archive_expansion_budget_and_unsafe_paths() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bomb.docx");
        fs::write(&path, zip_file(&[("word/document.xml", "<root/>")])).unwrap();
        let options = ExtractOptions {
            max_expanded_bytes: 3,
            ..ExtractOptions::default()
        };
        assert_eq!(
            extract_document(&path, &options, &AtomicBool::new(false))
                .unwrap_err()
                .code,
            ExtractErrorCode::LimitExceeded
        );
        assert_eq!(
            extract(
                "unsafe.docx",
                &zip_file(&[("../outside", "x"), ("word/document.xml", "<root/>")])
            )
            .unwrap_err()
            .code,
            ExtractErrorCode::UnsafePath
        );
    }
    #[test]
    fn cancellation_precedes_file_open() {
        let cancelled = AtomicBool::new(true);
        assert_eq!(
            extract_document(
                Path::new("absent.txt"),
                &ExtractOptions::default(),
                &cancelled
            )
            .unwrap_err()
            .code,
            ExtractErrorCode::Cancelled
        );
    }
    #[test]
    fn json_contract_is_camel_case_and_locations_optional() {
        let doc = extract("sample.txt", b"hello").unwrap();
        let json = serde_json::to_value(doc).unwrap();
        assert!(json.get("filePath").is_some());
        assert!(json["fingerprint"].get("sizeBytes").is_some());
        assert_eq!(json["sections"][0]["page"], serde_json::Value::Null);
        assert_eq!(json["sections"][0]["paragraph"], 1);
    }

    #[test]
    fn csv_quoted_cells_multiline_records_and_original_line() {
        let doc = extract(
            "table.csv",
            b"name,note\r\nTea,\"two\nlines, with \"\"quote\"\"\"\r\nCoffee,42",
        )
        .unwrap();
        assert_eq!(doc.sections.len(), 6);
        let note = doc
            .sections
            .iter()
            .find(|s| s.text.ends_with("two\nlines, with \"quote\""))
            .unwrap();
        assert!(note.text.contains("note"));
        assert!(note.text.contains("key=Tea"));
        assert_eq!(note.paragraph, Some(2));
        assert_eq!(doc.sections.last().unwrap().paragraph, Some(4));
        assert_eq!(
            extract("broken.csv", b"a,\"open").unwrap_err().code,
            ExtractErrorCode::CorruptDocument
        );
    }

    #[test]
    fn nested_docx_table_keeps_inner_cell_boundaries() {
        let bytes = zip_file(&[("word/document.xml", "<document><tbl><tr><tc><p><r><t>Outer</t></r></p><tbl><tr><tc><p><r><t>Inner A</t></r></p></tc><tc><p><r><t>Inner B</t></r></p></tc></tr></tbl></tc><tc><p><r><t>42</t></r></p></tc></tr></tbl></document>")]);
        let doc = extract("nested.docx", &bytes).unwrap();
        assert_eq!(doc.sections.len(), 2);
        assert!(doc.sections[0].text.ends_with("Outer\nInner A\tInner B"));
        assert!(doc.sections[1].text.ends_with(": 42"));
    }

    #[test]
    fn supplied_pdf_password_preserves_raw_fingerprint_and_page_numbers() {
        for (bytes, password) in [
            (
                include_bytes!("../../tests/fixtures/encrypted-pdf/rc4-40.pdf").as_slice(),
                "fixture-user-password",
            ),
            (
                include_bytes!("../../tests/fixtures/encrypted-pdf/rc4-128.pdf").as_slice(),
                "fixture-user-password",
            ),
            (
                include_bytes!("../../tests/fixtures/encrypted-pdf/aes128.pdf").as_slice(),
                "fixture-user-password",
            ),
            (
                include_bytes!("../../tests/fixtures/encrypted-pdf/aes256.pdf").as_slice(),
                "암호-fixture-密碼",
            ),
        ] {
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("암호.PDF");
            fs::write(&path, bytes).unwrap();
            let options = ExtractOptions::default();
            let cancel = AtomicBool::new(false);
            assert_eq!(
                extract_document(&path, &options, &cancel).unwrap_err().code,
                ExtractErrorCode::Encrypted
            );
            let rejected = extract_document_with_password(
                &path,
                &options,
                &cancel,
                "not-the-fixture-password",
            )
            .unwrap_err();
            assert_eq!(rejected.code, ExtractErrorCode::PasswordRejected);
            assert!(!rejected.message.contains("not-the-fixture-password"));
            for secret in [password, "fixture-owner-password"] {
                let extracted =
                    extract_document_with_password(&path, &options, &cancel, secret).unwrap();
                assert_eq!(
                    extracted
                        .sections
                        .iter()
                        .filter_map(|section| section.page)
                        .collect::<Vec<_>>(),
                    vec![1, 3]
                );
                assert!(extracted.sections[1].text.contains("unlockcanarythird"));
                assert_eq!(
                    extracted.fingerprint.sha256,
                    format!("{:x}", Sha256::digest(bytes))
                );
                assert!(!serde_json::to_string(&extracted).unwrap().contains(secret));
            }
            assert_eq!(fs::read(&path).unwrap(), bytes);
            // An unopened PDF may load as a partial encrypted Document instead
            // of a parser error. Our extraction contract still requires a password.
            assert_eq!(
                extract_document(&path, &options, &cancel).unwrap_err().code,
                ExtractErrorCode::Encrypted
            );
            let limited = ExtractOptions {
                max_text_bytes: 2,
                ..Default::default()
            };
            assert_eq!(
                extract_document_with_password(&path, &limited, &cancel, password)
                    .unwrap_err()
                    .code,
                ExtractErrorCode::LimitExceeded
            );
            assert_eq!(
                extract_document_with_password(&path, &options, &AtomicBool::new(true), password)
                    .unwrap_err()
                    .code,
                ExtractErrorCode::Cancelled
            );
        }
    }
    #[test]
    fn encrypted_pdf_and_ooxml_are_classified() {
        let mut doc = pdf_extract::Document::load_mem(&pdf(&["confidential fixture"])).unwrap();
        doc.trailer.set(
            "ID",
            pdf_extract::Object::Array(vec![
                pdf_extract::Object::string_literal("fixture-id-first"),
                pdf_extract::Object::string_literal("fixture-id-second"),
            ]),
        );
        let state = pdf_extract::EncryptionState::try_from(pdf_extract::EncryptionVersion::V2 {
            document: &doc,
            owner_password: "fixture-owner",
            user_password: "fixture-user",
            key_length: 128,
            permissions: pdf_extract::Permissions::all(),
        })
        .unwrap();
        doc.encrypt(&state).unwrap();
        let mut bytes = Vec::new();
        doc.save_to(&mut bytes).unwrap();
        assert_eq!(
            extract("encrypted.pdf", &bytes).unwrap_err().code,
            ExtractErrorCode::Encrypted
        );
        let mut compound = cfb::CompoundFile::create(Cursor::new(Vec::new())).unwrap();
        compound
            .create_stream("/EncryptionInfo")
            .unwrap()
            .write_all(b"fixture")
            .unwrap();
        compound
            .create_stream("/EncryptedPackage")
            .unwrap()
            .write_all(b"fixture")
            .unwrap();
        assert_eq!(
            extract("encrypted.docx", &compound.into_inner().into_inner())
                .unwrap_err()
                .code,
            ExtractErrorCode::Encrypted
        );
    }

    #[test]
    fn hwp_extended_record_length_and_compression_budget() {
        let text = utf16(&"본문".repeat(1500));
        let mut records = (0xfff00000u32 | 67 | 1 << 10).to_le_bytes().to_vec();
        records.extend_from_slice(&(text.len() as u32).to_le_bytes());
        records.extend_from_slice(&text);
        assert_eq!(
            extract("extended.hwp", &hwp_file(1, &records))
                .unwrap()
                .sections[0]
                .text,
            "본문".repeat(1500)
        );
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("compressed.hwp");
        fs::write(&path, hwp_file(1, &records)).unwrap();
        let options = ExtractOptions {
            max_expanded_bytes: 100,
            ..ExtractOptions::default()
        };
        assert_eq!(
            extract_document(&path, &options, &AtomicBool::new(false))
                .unwrap_err()
                .code,
            ExtractErrorCode::LimitExceeded
        );
    }
    fn xlsx_parts(sheet: &str, extra: &[(&str, &str)], props: &str) -> Vec<u8> {
        let workbook = format!(
            r#"<workbook xmlns:r="urn:rel"><workbookPr date1904="{props}"/><sheets><sheet name="분기 매출" sheetId="7" r:id="rSheet"/></sheets></workbook>"#
        );
        let mut parts = vec![
            ("xl/workbook.xml", workbook.as_str()),
            (
                "xl/_rels/workbook.xml.rels",
                r#"<Relationships><Relationship Id="rSheet" Type="urn:office/worksheet" Target="worksheets/sheet7.xml"/></Relationships>"#,
            ),
            ("xl/worksheets/sheet7.xml", sheet),
        ];
        parts.extend_from_slice(extra);
        zip_file(&parts)
    }
    #[test]
    fn pptx_uses_relationship_order_and_distinct_notes() {
        let bytes = zip_file(&[
            (
                "ppt/presentation.xml",
                r#"<presentation xmlns:r="urn:r"><sldIdLst><sldId id="256" r:id="last"/><sldId id="900" r:id="first"/></sldIdLst></presentation>"#,
            ),
            (
                "ppt/_rels/presentation.xml.rels",
                r#"<Relationships><Relationship Id="first" Type="urn:office/slide" Target="slides/slide1.xml"/><Relationship Id="last" Type="urn:office/slide" Target="slides/slide9.xml"/></Relationships>"#,
            ),
            (
                "ppt/slides/slide9.xml",
                "<sld><p><r><t>첫 번째 한국어</t></r></p></sld>",
            ),
            (
                "ppt/slides/slide1.xml",
                "<sld><p><r><t>두 번째</t></r></p></sld>",
            ),
            (
                "ppt/slides/_rels/slide9.xml.rels",
                r#"<Relationships><Relationship Id="n" Type="urn:office/notesSlide" Target="../notesSlides/notesSlide4.xml"/></Relationships>"#,
            ),
            (
                "ppt/notesSlides/notesSlide4.xml",
                r#"<notes><sp><nvSpPr><nvPr><ph type="body"/></nvPr></nvSpPr><p><t>발표자 메모</t></p></sp><sp><nvSpPr><nvPr><ph type="sldNum"/></nvPr></nvSpPr><p><t>999</t></p></sp></notes>"#,
            ),
        ]);
        let doc = extract("presentation.pptx", &bytes).unwrap();
        assert_eq!(doc.sections.len(), 3);
        assert_eq!(doc.sections[0].page, Some(1));
        assert_eq!(doc.sections[1].heading.as_deref(), Some("Slide 1 / Notes"));
        assert_eq!(doc.sections[2].page, Some(2));
        assert!(!doc.sections.iter().any(|s| s.text == "999"));
        assert_eq!(
            doc.fingerprint.sha256,
            format!("{:x}", Sha256::digest(&bytes))
        );
    }
    #[test]
    fn xlsx_shared_inline_typed_values_dates_and_cached_formulas() {
        let sheet = r#"<worksheet><sheetData><row r="1"><c r="A1" t="s"><v>0</v></c><c r="B1" t="inlineStr"><is><t>증가율</t></is></c><c r="C1" t="inlineStr"><is><t>날짜</t></is></c></row><row r="4"><c r="A4" t="s"><v>1</v></c><c r="B4" s="1"><v>0.25</v></c><c r="C4" s="2"><v>61</v></c><c r="D4" t="b"><v>1</v></c><c r="E4"><f>1+1</f><v>2</v></c><c r="F4" t="e"><v>#DIV/0!</v></c><c r="G4"><f>RAND()</f></c></row></sheetData><mergeCells><mergeCell ref="A6:C6"/></mergeCells></worksheet>"#;
        let bytes = xlsx_parts(
            sheet,
            &[
                (
                    "xl/sharedStrings.xml",
                    "<sst><si><t>상품</t></si><si><r><t>한국</t></r><r><t>차</t></r></si></sst>",
                ),
                (
                    "xl/styles.xml",
                    r#"<styleSheet><cellXfs count="3"><xf numFmtId="0"/><xf numFmtId="9"/><xf numFmtId="14"/></cellXfs></styleSheet>"#,
                ),
            ],
            "0",
        );
        let doc = extract("book.xlsx", &bytes).unwrap();
        let rate = doc
            .sections
            .iter()
            .find(|s| s.text.contains("25%"))
            .unwrap();
        assert!(rate.text.contains("증가율"));
        assert!(rate.text.contains("key=한국차"));
        assert_eq!(rate.paragraph, Some(4));
        assert!(rate.heading.as_deref().unwrap().contains("cell=B4"));
        assert!(doc.sections.iter().any(|s| s.text.contains("1900-03-01")));
        assert!(doc
            .sections
            .iter()
            .any(|s| s.text.contains("[boolean] true")));
        assert!(doc
            .sections
            .iter()
            .any(|s| s.text.contains("cached result; not recalculated")));
        assert!(doc
            .sections
            .iter()
            .any(|s| s.text.contains("cached result unavailable")));
        assert!(doc
            .sections
            .iter()
            .any(|s| s.text.contains("Merged range A6:C6")));
        assert!(doc.sections.iter().all(|s| s.page.is_none()));
    }
    #[test]
    fn xlsx_declared_headers_and_both_date_systems() {
        let sheet = r#"<worksheet xmlns:r="urn:r"><sheetData><row r="3"><c r="C3" t="inlineStr"><is><t>다른 제목</t></is></c></row><row r="8"><c r="B8" t="inlineStr"><is><t>계약</t></is></c><c r="C8" s="0"><v>0</v></c></row></sheetData><tableParts><tablePart r:id="t"/></tableParts></worksheet>"#;
        let bytes = xlsx_parts(
            sheet,
            &[
                (
                    "xl/styles.xml",
                    r#"<styleSheet><cellXfs><xf numFmtId="14"/></cellXfs></styleSheet>"#,
                ),
                (
                    "xl/worksheets/_rels/sheet7.xml.rels",
                    r#"<Relationships><Relationship Id="t" Type="urn:office/table" Target="../tables/table1.xml"/></Relationships>"#,
                ),
                (
                    "xl/tables/table1.xml",
                    r#"<table displayName="계약표" ref="B7:C9"><tableColumns><tableColumn name="품목"/><tableColumn name="체결일"/></tableColumns></table>"#,
                ),
            ],
            "1",
        );
        let doc = extract("dates.xlsx", &bytes).unwrap();
        let date = doc
            .sections
            .iter()
            .find(|s| s.text.contains("1904-01-01"))
            .unwrap();
        assert!(date.text.contains("체결일"));
        assert!(date.heading.as_deref().unwrap().contains("header=declared"));
        assert!(excel_number("60", Some(&(14, String::new())), false)
            .unwrap()
            .contains("1900-02-29 [Excel compatibility day]"));
        assert!(excel_number("0.5", Some(&(20, String::new())), false)
            .unwrap()
            .contains("12:00:00"));
    }
    #[test]
    fn odt_mixed_text_order_utf16_and_declared_table_header() {
        let xml = r#"<document-content><body><text><h>계약</h><p>Hello <span>한국</span>!<s c="2"/>끝<tab/>값<line-break/>다음</p><table name="합계"><table-header-rows><table-row><table-cell><p>제품</p></table-cell><table-cell><p>비용</p></table-cell></table-row></table-header-rows><table-row><table-cell><p>차</p></table-cell><table-cell value-type="float" value="42"><p>42</p></table-cell></table-row></table></text></body></document-content>"#;
        // ZIP fixture contains actual UTF-16 encoded XML bytes.
        let mut cursor = Cursor::new(Vec::new());
        {
            let mut writer = zip::ZipWriter::new(&mut cursor);
            writer
                .start_file("content.xml", zip::write::SimpleFileOptions::default())
                .unwrap();
            use std::io::Write;
            writer.write_all(&[0xff, 0xfe]).unwrap();
            writer.write_all(&utf16(xml)).unwrap();
            writer.finish().unwrap();
        }
        let doc = extract("contract.odt", &cursor.into_inner()).unwrap();
        assert_eq!(doc.sections[1].text, "Hello 한국!  끝\t값\n다음");
        let cost = doc
            .sections
            .iter()
            .find(|s| s.text.ends_with("[float; raw=42] 42"))
            .unwrap();
        assert!(cost.text.contains("비용"));
        assert!(cost.text.contains("key=차"));
        assert_eq!(cost.paragraph, Some(2));
        assert!(cost.heading.as_deref().unwrap().contains("header=declared"));
    }
    #[test]
    fn ods_repeated_cells_rows_typed_cache_and_empty_tails() {
        let xml = r#"<document-content><body><spreadsheet><table name="매출"><table-row><table-cell><p>항목</p></table-cell><table-cell><p>증가율</p></table-cell><table-cell><p>기준일</p></table-cell></table-row><table-row number-rows-repeated="2"><table-cell><p>차</p></table-cell><table-cell value-type="percentage" value="0.25" formula="of:=1/4"><p>25%</p></table-cell><table-cell value-type="date" date-value="2026-11-04"/></table-row><table-row number-rows-repeated="1000000"><table-cell number-columns-repeated="1000000"/></table-row><table-row><table-cell><p>후속</p></table-cell><table-cell value-type="boolean" boolean-value="true" number-columns-repeated="2"/></table-row></table></spreadsheet></body></document-content>"#;
        let doc = extract("report.ods", &zip_file(&[("content.xml", xml)])).unwrap();
        assert_eq!(doc.sections.len(), 12);
        assert!(doc
            .sections
            .iter()
            .any(|s| s.paragraph == Some(3) && s.text.contains("25%")));
        assert!(doc
            .sections
            .iter()
            .any(|s| s.paragraph == Some(1_000_004) && s.text.contains("cell=C1000004")));
        assert!(doc
            .sections
            .iter()
            .any(|s| s.text.contains("date; raw=2026-11-04")));
        assert!(doc
            .sections
            .iter()
            .any(|s| s.text.contains("formula cached result; not recalculated")));
    }
    #[test]
    fn odp_pages_notes_tables_and_unsafe_xml() {
        let xml="<document-content><body><presentation><page name=\"첫 장\"><frame><p>슬라이드 본문</p></frame><notes><p>발표 메모</p></notes></page><page><p>다음 장</p></page></presentation></body></document-content>";
        let doc = extract("talk.odp", &zip_file(&[("content.xml", xml)])).unwrap();
        assert_eq!(doc.sections[0].page, Some(1));
        assert!(doc.sections[1]
            .heading
            .as_deref()
            .unwrap()
            .contains("Notes"));
        assert_eq!(doc.sections[2].page, Some(2));
        let bad = zip_file(&[(
            "content.xml",
            "<!DOCTYPE x [<!ENTITY x SYSTEM 'https://example.invalid/private'>]><x>&x;</x>",
        )]);
        assert_eq!(
            extract("bad.ods", &bad).unwrap_err().code,
            ExtractErrorCode::CorruptDocument
        );
        assert_eq!(
            extract(
                "encrypted.odt",
                &zip_file(&[
                    (
                        "META-INF/manifest.xml",
                        "<manifest><file-entry><encryption-data/></file-entry></manifest>"
                    ),
                    ("content.xml", xml)
                ])
            )
            .unwrap_err()
            .code,
            ExtractErrorCode::Encrypted
        );
    }
    #[test]
    fn tables_repeat_header_and_row_key_in_long_unicode_fragments() {
        let value = "매출한글🙂".repeat(700);
        let doc = extract("long.csv", format!("품목,설명\n한국차,{value}").as_bytes()).unwrap();
        let fragments: Vec<_> = doc
            .sections
            .iter()
            .filter(|s| {
                s.paragraph == Some(2) && s.heading.as_deref().unwrap().contains("column=설명")
            })
            .collect();
        assert!(fragments.len() > 3);
        assert!(fragments.iter().all(|s| s.text.chars().count() <= 1000
            && s.text.contains("key=한국차")
            && s.text.contains("설명")
            && s.text.contains("cell=B2")));
        assert!(fragments.iter().all(|s| s
            .heading
            .as_deref()
            .unwrap()
            .contains("first-row inferred")));
        let docx=zip_file(&[("word/document.xml","<document><tbl><tr><trPr><tblHeader/></trPr><tc><p><t>품목</t></p></tc><tc><p><t>가격</t></p></tc></tr><tr><tc><p><t>차</t></p></tc><tc><p><t>42</t></p></tc></tr></tbl></document>")]);
        let doc = extract("table.docx", &docx).unwrap();
        let amount = doc.sections.last().unwrap();
        assert!(amount.text.contains("가격"));
        assert!(amount.text.contains("key=차"));
        assert!(amount
            .heading
            .as_deref()
            .unwrap()
            .contains("header=declared"));
    }
    #[test]
    fn office_relationship_escape_repeats_metadata_budgets_and_cancellation() {
        let bad = xlsx_parts("<worksheet/>", &[], "0");
        // A syntactically valid workbook relationship cannot leave package root.
        let escape = zip_file(&[
            (
                "xl/workbook.xml",
                r#"<workbook xmlns:r="urn:r"><sheets><sheet name="x" r:id="x"/></sheets></workbook>"#,
            ),
            (
                "xl/_rels/workbook.xml.rels",
                r#"<Relationships><Relationship Id="x" Type="urn:office/worksheet" Target="../../outside.xml"/></Relationships>"#,
            ),
        ]);
        assert_eq!(
            extract("unsafe.xlsx", &escape).unwrap_err().code,
            ExtractErrorCode::UnsafePath
        );
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("limited.ods");
        let repeated=zip_file(&[("content.xml","<document-content><body><spreadsheet><table><table-row number-rows-repeated=\"1000000\"><table-cell><p>반복</p></table-cell></table-row></table></spreadsheet></body></document-content>")]);
        fs::write(&path, repeated).unwrap();
        assert_eq!(
            extract_document(&path, &ExtractOptions::default(), &AtomicBool::new(false))
                .unwrap_err()
                .code,
            ExtractErrorCode::LimitExceeded
        );
        let options = ExtractOptions {
            max_text_bytes: 40,
            ..Default::default()
        };
        fs::write(&path,zip_file(&[("content.xml","<document-content><body><spreadsheet><table><table-row><table-cell><p>표</p></table-cell></table-row></table></spreadsheet></body></document-content>")])).unwrap();
        assert_eq!(
            extract_document(&path, &options, &AtomicBool::new(false))
                .unwrap_err()
                .code,
            ExtractErrorCode::LimitExceeded
        );
        let path = dir.path().join("cancel.xlsx");
        fs::write(&path, bad).unwrap();
        assert_eq!(
            extract_document(&path, &ExtractOptions::default(), &AtomicBool::new(true))
                .unwrap_err()
                .code,
            ExtractErrorCode::Cancelled
        );
    }
    #[test]
    fn hwpx_explicit_cell_columns_and_docx_header_span_stay_aligned() {
        let xml="<sec><tbl><tr><tc><cellSpan colSpan=\"2\"/><p><t>매출</t></p></tc><tc><cellAddr colAddr=\"2\"/><p><t>비용</t></p></tc></tr><tr><tc><cellAddr colAddr=\"1\"/><p><t>100</t></p></tc><tc><cellAddr colAddr=\"2\"/><p><t>42</t></p></tc></tr></tbl></sec>";
        let doc = extract("aligned.hwpx", &zip_file(&[("Contents/section0.xml", xml)])).unwrap();
        let sales = doc
            .sections
            .iter()
            .find(|s| s.text.ends_with(": 100"))
            .unwrap();
        assert!(sales.text.contains("매출"));
        assert!(sales.text.contains("cell=B2"));
        let cost = doc
            .sections
            .iter()
            .find(|s| s.text.ends_with(": 42"))
            .unwrap();
        assert!(cost.text.contains("비용"));
        assert!(cost.text.contains("cell=C2"));
        let xml="<document><tbl><tr><tc><tcPr><gridSpan val=\"2\"/></tcPr><p><t>Revenue</t></p></tc></tr><tr><tc><p><t>Tea</t></p></tc><tc><p><t>42</t></p></tc></tr></tbl></document>";
        let doc = extract("span.docx", &zip_file(&[("word/document.xml", xml)])).unwrap();
        assert!(doc.sections.last().unwrap().text.contains("Revenue"));
    }
}
