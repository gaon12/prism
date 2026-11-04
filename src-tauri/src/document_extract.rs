//! Local-only document extraction. Callers must authorize the canonical path against
//! content-index scope before invoking this module. No network, OCR, scripts or macros.
use encoding_rs::{EUC_KR, UTF_16BE, UTF_16LE};
use quick_xml::{events::Event, Reader};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
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
    CorruptDocument,
    OcrRequired,
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

/// Reject symlinks/reparse points in every path component. Scope authorization
/// remains the caller's job; this check alone is not a filesystem sandbox.
fn check_path(path: &Path) -> Result<()> {
    for ancestor in path.ancestors() {
        if ancestor.as_os_str().is_empty() {
            continue;
        }
        let meta = fs::symlink_metadata(ancestor)
            .map_err(|_| err(ExtractErrorCode::Io, "Cannot inspect document path"))?;
        let mut link = meta.file_type().is_symlink();
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            link |= meta.file_attributes() & 0x400 != 0;
        }
        if link {
            return Err(err(
                ExtractErrorCode::UnsafePath,
                "Symlink or reparse-point document paths are skipped",
            ));
        }
    }
    Ok(())
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
            | "docx"
            | "hwpx"
            | "pdf"
            | "hwp"
    )
}

pub fn extract_document(
    path: &Path,
    options: &ExtractOptions,
    cancel: &AtomicBool,
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
        "hwp" => extract_hwp(&bytes, &mut output)?,
        "pdf" => extract_pdf(&bytes, &mut output)?,
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

fn extract_pdf(bytes: &[u8], out: &mut Collector<'_>) -> Result<()> {
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
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> Result<()> {
        let doc = pdf_extract::Document::load_mem(bytes).map_err(|_| corrupt())?;
        if doc.is_encrypted() {
            return Err(encrypted());
        }
        let pages = doc.get_pages();
        if pages.len() > out.options.max_sections {
            return Err(limit());
        }
        for number in pages.keys() {
            check_cancel(out.cancel)?;
            let mut writer = TextWriter {
                bytes: Vec::new(),
                max: out.options.max_text_bytes.saturating_sub(out.bytes),
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
            extract_lines(&page_text, false, Some(*number), out)?;
        }
        Ok(())
    }))
    .map_err(|_| corrupt())?;
    result?;
    if out.sections.is_empty() {
        return Err(err(
            ExtractErrorCode::OcrRequired,
            "PDF has no extractable text; OCR is disabled",
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
