//! Reference-only EPUB container/spine and self-contained, unscripted documents.
//! Resources stay in memory. No ZIP member becomes a filesystem or network URL.
use crate::collection_import::check;
use crate::collection_store::{error, ErrorCode, PageKind, PageLocation, Result};
use base64::Engine;
#[path = "collection_epub_document.rs"]
mod document;
#[path = "collection_epub_styles.rs"]
mod styles;
use quick_xml::{events::Event, Reader};
use serde::Serialize;
use sha1::{Digest, Sha1};
use std::{
    collections::HashMap,
    io::{Cursor, Read},
    sync::atomic::AtomicBool,
};

const MAX_TEXT: usize = 4 << 20;
const MAX_DOCUMENT: usize = 32 << 20;
#[derive(Clone, Debug)]
pub(crate) struct Chapter {
    pub entry: String,
    fixed: bool,
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Document {
    pub html: String,
    pub width: u32,
    pub height: u32,
    pub fixed: bool,
}
struct XmlNode {
    name: String,
    attrs: HashMap<String, String>,
    text: String,
    parent: Option<usize>,
}
impl XmlNode {
    fn attr(&self, name: &str) -> Option<&str> {
        self.attrs.get(name).map(String::as_str)
    }
}
fn xml(bytes: &[u8]) -> Result<Vec<XmlNode>> {
    let text = std::str::from_utf8(bytes).map_err(|_| error(ErrorCode::Unsupported))?;
    let mut reader = Reader::from_str(text);
    let mut nodes: Vec<XmlNode> = Vec::new();
    let mut stack = Vec::new();
    loop {
        let event = reader
            .read_event()
            .map_err(|_| error(ErrorCode::Unsupported))?;
        let start = matches!(&event, Event::Start(_));
        match event {
            Event::Start(ref e) | Event::Empty(ref e) => {
                if nodes.len() >= 50_000 || stack.len() >= 64 {
                    return Err(error(ErrorCode::Limit));
                }
                let mut attrs = HashMap::new();
                for attr in e.attributes() {
                    let attr = attr.map_err(|_| error(ErrorCode::Unsupported))?;
                    let key = String::from_utf8(attr.key.as_ref().to_vec())
                        .map_err(|_| error(ErrorCode::Unsupported))?;
                    let value = attr
                        .decode_and_unescape_value(reader.decoder())
                        .map_err(|_| error(ErrorCode::Unsupported))?
                        .into_owned();
                    if attrs.insert(key, value).is_some() {
                        return Err(error(ErrorCode::Input));
                    }
                }
                nodes.push(XmlNode {
                    name: String::from_utf8(e.local_name().as_ref().to_vec())
                        .map_err(|_| error(ErrorCode::Unsupported))?,
                    attrs,
                    text: String::new(),
                    parent: stack.last().copied(),
                });
                if start {
                    stack.push(nodes.len() - 1);
                }
            }
            Event::End(_) => {
                if stack.pop().is_none() {
                    return Err(error(ErrorCode::Unsupported));
                }
            }
            Event::Text(e) => {
                if let Some(index) = stack.last().copied() {
                    let decoded = e.decode().map_err(|_| error(ErrorCode::Unsupported))?;
                    nodes[index].text.push_str(
                        &quick_xml::escape::unescape(&decoded)
                            .map_err(|_| error(ErrorCode::Unsupported))?,
                    );
                }
            }
            Event::CData(e) => {
                if let Some(index) = stack.last().copied() {
                    nodes[index]
                        .text
                        .push_str(&e.decode().map_err(|_| error(ErrorCode::Unsupported))?);
                }
            }
            Event::GeneralRef(e) => {
                if let Some(index) = stack.last().copied() {
                    let entity = format!(
                        "&{};",
                        e.decode().map_err(|_| error(ErrorCode::Unsupported))?
                    );
                    nodes[index].text.push_str(
                        &quick_xml::escape::unescape(&entity)
                            .map_err(|_| error(ErrorCode::Unsupported))?,
                    );
                }
            }
            Event::DocType(_) => return Err(error(ErrorCode::Unsupported)),
            Event::Eof => break,
            _ => {}
        }
    }
    if !stack.is_empty() || nodes.iter().filter(|n| n.parent.is_none()).count() != 1 {
        return Err(error(ErrorCode::Unsupported));
    }
    Ok(nodes)
}
fn member(value: &str) -> Result<()> {
    PageLocation {
        kind: PageKind::Epub,
        entry: Some(value.into()),
        number: 1,
    }
    .validate()
}
/// URL resolution stays inside the ZIP root, including percent-encoded '..'.
fn resolve(base: &str, href: &str) -> Result<String> {
    let href = href.split('#').next().unwrap_or("");
    if href.is_empty() || href.contains(['?', '\\', '\0', ':']) || href.starts_with('/') {
        return Err(error(ErrorCode::UnsafePath));
    }
    let mut bytes = Vec::new();
    let input = href.as_bytes();
    let mut i = 0;
    while i < input.len() {
        if input[i] == b'%' {
            let pair = input
                .get(i + 1..i + 3)
                .ok_or_else(|| error(ErrorCode::UnsafePath))?;
            let digit = |b: u8| {
                (b as char)
                    .to_digit(16)
                    .map(|v| v as u8)
                    .ok_or_else(|| error(ErrorCode::UnsafePath))
            };
            bytes.push(digit(pair[0])? * 16 + digit(pair[1])?);
            i += 3;
        } else {
            bytes.push(input[i]);
            i += 1;
        }
    }
    let decoded = String::from_utf8(bytes).map_err(|_| error(ErrorCode::UnsafePath))?;
    if decoded.contains(['\\', '\0', ':', '?']) || decoded.starts_with('/') {
        return Err(error(ErrorCode::UnsafePath));
    }
    let mut parts: Vec<_> = base
        .rsplit_once('/')
        .map(|(parent, _)| parent.split('/').collect())
        .unwrap_or_default();
    for part in decoded.split('/') {
        match part {
            "." => {}
            ".." => {
                if parts.pop().is_none() {
                    return Err(error(ErrorCode::UnsafePath));
                }
            }
            "" => return Err(error(ErrorCode::UnsafePath)),
            _ => parts.push(part),
        }
    }
    let result = parts.join("/");
    member(&result)?;
    Ok(result)
}
struct Item {
    media: String,
    entry: String,
}
pub(crate) struct Book<'a> {
    archive: zip::ZipArchive<Cursor<&'a [u8]>>,
    entries: HashMap<String, usize>,
    items: HashMap<String, String>,
    pub chapters: Vec<Chapter>,
    direction: String,
    fonts: HashMap<String, (Vec<u8>, usize)>,
    resources: HashMap<String, String>,
    active: Vec<String>,
    used: usize,
    cancel: &'a AtomicBool,
}
impl<'a> Book<'a> {
    pub fn open(bytes: &'a [u8], cancel: &'a AtomicBool) -> Result<Self> {
        check(cancel)?;
        let mut archive =
            zip::ZipArchive::new(Cursor::new(bytes)).map_err(|_| error(ErrorCode::Unsupported))?;
        if archive.len() > 50_000 {
            return Err(error(ErrorCode::Limit));
        }
        let mut entries = HashMap::new();
        let mut expanded = 0u64;
        for i in 0..archive.len() {
            check(cancel)?;
            let file = archive
                .by_index(i)
                .map_err(|_| error(ErrorCode::Encrypted))?;
            let name = file
                .name()
                .map_err(|_| error(ErrorCode::Unsupported))?
                .into_owned();
            member(name.trim_end_matches('/'))?;
            if file.is_dir() {
                continue;
            }
            if file
                .unix_mode()
                .is_some_and(|mode| mode & 0o170000 == 0o120000)
            {
                return Err(error(ErrorCode::UnsafePath));
            }
            expanded = expanded.saturating_add(file.size());
            if file.size() > 128 << 20 || expanded > 512 << 20 {
                return Err(error(ErrorCode::Limit));
            }
            if entries.insert(name, i).is_some() {
                return Err(error(ErrorCode::Input));
            }
        }
        let mut book = Self {
            archive,
            entries,
            items: HashMap::new(),
            chapters: Vec::new(),
            direction: "ltr".into(),
            fonts: HashMap::new(),
            resources: HashMap::new(),
            active: Vec::new(),
            used: 0,
            cancel,
        };
        if book.read("mimetype", 64)? != b"application/epub+zip" {
            return Err(error(ErrorCode::Unsupported));
        }
        let container = xml(&book.read("META-INF/container.xml", MAX_TEXT)?)?;
        let package = container
            .iter()
            .find(|n| {
                n.name == "rootfile"
                    && n.attr("media-type") == Some("application/oebps-package+xml")
            })
            .and_then(|n| n.attr("full-path"))
            .ok_or_else(|| error(ErrorCode::Unsupported))?;
        let package = resolve("", package)?;
        let nodes = xml(&book.read(&package, MAX_TEXT)?)?;
        if nodes.first().is_none_or(|n| n.name != "package") {
            return Err(error(ErrorCode::Unsupported));
        }
        let package_root = &nodes[0];
        let metadata = nodes
            .iter()
            .position(|n| n.name == "metadata" && n.parent == Some(0));
        let fixed = nodes.iter().any(|n| {
            n.parent == metadata
                && n.name == "meta"
                && n.attr("property") == Some("rendition:layout")
                && n.text.trim() == "pre-paginated"
        });
        let manifest = nodes
            .iter()
            .position(|n| n.name == "manifest" && n.parent == Some(0))
            .ok_or_else(|| error(ErrorCode::Unsupported))?;
        let spine = nodes
            .iter()
            .position(|n| n.name == "spine" && n.parent == Some(0))
            .ok_or_else(|| error(ErrorCode::Unsupported))?;
        if nodes[spine].attr("page-progression-direction") == Some("rtl") {
            book.direction = "rtl".into();
        }
        let mut ids = HashMap::new();
        for node in nodes
            .iter()
            .filter(|n| n.parent == Some(manifest) && n.name == "item")
        {
            let id = node.attr("id").ok_or_else(|| error(ErrorCode::Input))?;
            let href = node.attr("href").ok_or_else(|| error(ErrorCode::Input))?;
            let remote = href.contains(':') || href.starts_with("//");
            let entry = if remote {
                href.to_string()
            } else {
                resolve(&package, href)?
            };
            if !remote && !book.entries.contains_key(&entry) {
                return Err(error(ErrorCode::Unsupported));
            }
            let media = node
                .attr("media-type")
                .ok_or_else(|| error(ErrorCode::Input))?
                .to_string();
            if book.items.insert(entry.clone(), media.clone()).is_some()
                || ids.insert(id, Item { media, entry }).is_some()
            {
                return Err(error(ErrorCode::Input));
            }
        }
        for node in nodes
            .iter()
            .filter(|n| n.parent == Some(spine) && n.name == "itemref")
        {
            let item = ids
                .get(node.attr("idref").unwrap_or(""))
                .ok_or_else(|| error(ErrorCode::Input))?;
            if item.entry.contains(':') || item.entry.starts_with("//") {
                return Err(error(ErrorCode::Unsupported));
            }
            if !matches!(
                item.media.as_str(),
                "application/xhtml+xml" | "text/html" | "image/svg+xml"
            ) {
                return Err(error(ErrorCode::Unsupported));
            }
            let properties: Vec<_> = node
                .attr("properties")
                .unwrap_or("")
                .split_whitespace()
                .collect();
            book.chapters.push(Chapter {
                entry: item.entry.clone(),
                fixed: if properties.contains(&"rendition:layout-reflowable") {
                    false
                } else {
                    fixed || properties.contains(&"rendition:layout-pre-paginated")
                },
            });
        }
        if book.chapters.is_empty() {
            return Err(error(ErrorCode::Unsupported));
        }
        if book.entries.contains_key("META-INF/encryption.xml") {
            let identifier = package_root
                .attr("unique-identifier")
                .and_then(|id| {
                    nodes.iter().find(|n| {
                        n.parent == metadata && n.name == "identifier" && n.attr("id") == Some(id)
                    })
                })
                .map(|n| n.text.clone())
                .unwrap_or_default();
            let encrypted = xml(&book.read("META-INF/encryption.xml", MAX_TEXT)?)?;
            for (i, _) in encrypted
                .iter()
                .enumerate()
                .filter(|(_, n)| n.name == "EncryptedData")
            {
                let descendant = |n: &XmlNode| {
                    let mut p = n.parent;
                    while let Some(parent) = p {
                        if parent == i {
                            return true;
                        }
                        p = encrypted[parent].parent;
                    }
                    false
                };
                let algorithm = encrypted
                    .iter()
                    .find(|n| n.name == "EncryptionMethod" && descendant(n))
                    .and_then(|n| n.attr("Algorithm"))
                    .unwrap_or("");
                let entry = encrypted
                    .iter()
                    .find(|n| n.name == "CipherReference" && descendant(n))
                    .and_then(|n| n.attr("URI"))
                    .ok_or_else(|| error(ErrorCode::Encrypted))?;
                let entry = resolve("", entry)?;
                let media = book.items.get(&entry).map(String::as_str).unwrap_or("");
                if !font_media(media) {
                    return Err(error(ErrorCode::Encrypted));
                }
                let (key, count) = if algorithm == "http://www.idpf.org/2008/embedding"
                    && !identifier.is_empty()
                {
                    let identifier: String = identifier
                        .chars()
                        .filter(|c| !matches!(c, ' ' | '\t' | '\r' | '\n'))
                        .collect();
                    (Sha1::digest(identifier.as_bytes()).to_vec(), 1040)
                } else if algorithm == "http://ns.adobe.com/pdf/enc#RC" {
                    let id = identifier
                        .trim()
                        .strip_prefix("urn:uuid:")
                        .unwrap_or(identifier.trim())
                        .replace('-', "");
                    if id.len() != 32 || !id.bytes().all(|b| b.is_ascii_hexdigit()) {
                        return Err(error(ErrorCode::Encrypted));
                    }
                    (
                        (0..16)
                            .map(|n| u8::from_str_radix(&id[n * 2..n * 2 + 2], 16).unwrap())
                            .collect(),
                        1024,
                    )
                } else {
                    return Err(error(ErrorCode::Encrypted));
                };
                if book.fonts.insert(entry, (key, count)).is_some() {
                    return Err(error(ErrorCode::Input));
                }
            }
        }
        Ok(book)
    }
    fn read(&mut self, entry: &str, limit: usize) -> Result<Vec<u8>> {
        check(self.cancel)?;
        let index = *self
            .entries
            .get(entry)
            .ok_or_else(|| error(ErrorCode::Unsupported))?;
        let file = self
            .archive
            .by_index(index)
            .map_err(|_| error(ErrorCode::Encrypted))?;
        if file.size() > limit as u64 {
            return Err(error(ErrorCode::Limit));
        }
        let mut bytes = Vec::new();
        let mut reader = file.take(limit as u64 + 1);
        let mut buffer = [0u8; 64 << 10];
        loop {
            check(self.cancel)?;
            let n = reader
                .read(&mut buffer)
                .map_err(|_| error(ErrorCode::Unsupported))?;
            if n == 0 {
                break;
            }
            bytes.extend_from_slice(&buffer[..n]);
            if bytes.len() > limit {
                return Err(error(ErrorCode::Limit));
            }
        }
        self.used = self.used.saturating_add(bytes.len());
        if self.used > MAX_DOCUMENT {
            return Err(error(ErrorCode::Limit));
        }
        Ok(bytes)
    }
    fn resource(&mut self, base: &str, reference: &str, stylesheet: bool) -> Result<String> {
        check(self.cancel)?;
        if reference.starts_with('#') {
            return Ok(reference.into());
        }
        // Remote resources never become a request, even when the book declares
        // them in its manifest. They are not replaced with invented content.
        if reference.contains(':') || reference.starts_with("//") {
            if !stylesheet && reference.starts_with("data:image/") && reference.len() <= 16 << 20 {
                return Ok(reference.into());
            }
            return Ok("data:,".into());
        }
        let entry = resolve(base, reference)?;
        if let Some(uri) = self.resources.get(&entry) {
            return Ok(uri.clone());
        }
        if self.active.contains(&entry) || self.active.len() >= 16 {
            return Err(error(ErrorCode::Limit));
        }
        let media = self
            .items
            .get(&entry)
            .ok_or_else(|| error(ErrorCode::Unsupported))?
            .clone();
        let is_css = media == "text/css";
        if is_css != stylesheet
            || !(is_css
                || font_media(&media)
                || matches!(
                    media.as_str(),
                    "image/png"
                        | "image/jpeg"
                        | "image/gif"
                        | "image/webp"
                        | "image/svg+xml"
                        | "image/bmp"
                        | "image/tiff"
                ))
        {
            return Err(error(ErrorCode::Unsupported));
        }
        let mut bytes = self.read(&entry, if is_css { MAX_TEXT } else { 16 << 20 })?;
        if let Some((key, count)) = self.fonts.get(&entry) {
            for (i, byte) in bytes.iter_mut().take(*count).enumerate() {
                *byte ^= key[i % key.len()];
            }
        }
        self.active.push(entry.clone());
        if is_css {
            let css = std::str::from_utf8(&bytes).map_err(|_| error(ErrorCode::Unsupported))?;
            bytes = self.css(&entry, css)?.into_bytes();
        }
        self.active.pop();
        let uri = format!(
            "data:{media};base64,{}",
            base64::engine::general_purpose::STANDARD.encode(bytes)
        );
        if uri.len() > MAX_DOCUMENT {
            return Err(error(ErrorCode::Limit));
        }
        self.resources.insert(entry, uri.clone());
        Ok(uri)
    }
}
fn font_media(media: &str) -> bool {
    matches!(
        media,
        "font/ttf"
            | "font/otf"
            | "font/woff"
            | "font/woff2"
            | "application/vnd.ms-opentype"
            | "application/font-sfnt"
            | "application/font-woff"
    )
}
fn css_uri(uri: &str) -> Result<String> {
    if uri == "data:," {
        return Ok(String::new());
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(
            uri.strip_prefix("data:text/css;base64,")
                .ok_or_else(|| error(ErrorCode::Unsupported))?,
        )
        .map_err(|_| error(ErrorCode::Unsupported))?;
    String::from_utf8(bytes).map_err(|_| error(ErrorCode::Unsupported))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    const FIXTURE: &[u8] = include_bytes!("../../tests/fixtures/epub/spine.epub");
    fn changed(name: &str, content: &[u8]) -> Vec<u8> {
        changed_in(FIXTURE, name, content)
    }
    fn changed_in(input: &[u8], name: &str, content: &[u8]) -> Vec<u8> {
        let mut source = zip::ZipArchive::new(Cursor::new(input)).unwrap();
        let mut target = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for i in 0..source.len() {
            let mut entry = source.by_index(i).unwrap();
            let path = entry.name().unwrap().into_owned();
            if path == name {
                continue;
            }
            let mut bytes = Vec::new();
            entry.read_to_end(&mut bytes).unwrap();
            target
                .start_file(path, zip::write::SimpleFileOptions::default())
                .unwrap();
            target.write_all(&bytes).unwrap();
        }
        target
            .start_file(name, zip::write::SimpleFileOptions::default())
            .unwrap();
        target.write_all(content).unwrap();
        target.finish().unwrap().into_inner()
    }
    #[test]
    fn independent_spine_preserves_original_styles_pixels_and_script_isolation() {
        let cancel = AtomicBool::new(false);
        let mut book = Book::open(FIXTURE, &cancel).unwrap();
        assert_eq!(
            book.chapters
                .iter()
                .map(|c| c.entry.as_str())
                .collect::<Vec<_>>(),
            ["OPS/chapters/2-first.xhtml", "OPS/chapters/10-second.xhtml"]
        );
        let first = book.document(1, "OPS/chapters/2-first.xhtml").unwrap();
        assert_eq!((first.width, first.height, first.fixed), (600, 800, true));
        assert!(
            first.html.contains("첫 번째 장 — 原文")
                && first.html.contains("PRISM EPUB FIRST 74829")
        );
        assert!(
            first.html.contains("data:image/png;base64,")
                && first.html.contains("font-family:serif")
                && first.html.contains("width:600px")
        );
        assert!(
            !first.html.contains("@import")
                && !first.html.contains("<script")
                && !first.html.contains("<iframe")
                && !first.html.contains("onerror")
                && !first.html.contains("127.0.0.1")
                && !first.html.contains("declared-cover")
        );
        let second = book.document(2, "OPS/chapters/10-second.xhtml").unwrap();
        assert!(
            !second.fixed
                && second.html.contains("<table>")
                && second.html.contains("<ruby>")
                && second.html.contains("원문 단락 100")
        );
        assert_eq!(
            book.document(1, "OPS/chapters/10-second.xhtml")
                .unwrap_err()
                .code,
            ErrorCode::SourceChanged
        );
    }
    #[test]
    fn css_tokenizer_resolves_escaped_urls_import_conditions_and_rejects_cycles() {
        let bytes = changed("OPS/styles/main.css", br#"@\69 mport url('theme.css') layer(book) screen; .panel{background:u\72l('../images/red%20panel.png')}"#);
        let cancel = AtomicBool::new(false);
        let mut book = Book::open(&bytes, &cancel).unwrap();
        let html = book.document(1, "OPS/chapters/2-first.xhtml").unwrap().html;
        let compact: String = html.chars().filter(|c| !c.is_whitespace()).collect();
        assert!(
            compact.contains("@layerbook{")
                && compact.contains("@mediascreen{")
                && html.contains("data:image/png;base64,")
        );
        let bytes = changed("OPS/styles/theme.css", b"@import 'main.css';");
        let mut book = Book::open(&bytes, &cancel).unwrap();
        assert_eq!(
            book.document(1, "OPS/chapters/2-first.xhtml")
                .unwrap_err()
                .code,
            ErrorCode::Limit
        );
    }
    #[test]
    fn archive_and_uri_paths_cannot_escape_the_container() {
        assert_eq!(
            resolve("OPS/chapters/a.xhtml", "../images/red%20panel.png").unwrap(),
            "OPS/images/red panel.png"
        );
        for value in [
            "../../../escape.png",
            "%2e%2e/%2e%2e/%2e%2e/x",
            "%2fescape",
            "../%5cescape",
            "file:/x",
            "https://x/a",
            "//host/a",
            "x%00.png",
            "x%ZZ",
        ] {
            assert!(resolve("OPS/chapters/a.xhtml", value).is_err(), "{value}");
        }
        let bytes = changed("../escape.png", b"outside");
        let cancel = AtomicBool::new(false);
        assert_eq!(
            Book::open(&bytes, &cancel).err().unwrap().code,
            ErrorCode::Input
        );
        let bytes = changed(
            "META-INF/container.xml",
            br#"<!DOCTYPE container SYSTEM 'file:///private-data'><container/>"#,
        );
        assert_eq!(
            Book::open(&bytes, &cancel).err().unwrap().code,
            ErrorCode::Unsupported
        );
        assert_eq!(
            Book::open(FIXTURE, &AtomicBool::new(true))
                .err()
                .unwrap()
                .code,
            ErrorCode::Cancelled
        );
    }
    #[test]
    fn primary_svg_spine_uses_its_intrinsic_viewport_without_html_margins() {
        let cancel = AtomicBool::new(false);
        let mut book = Book::open(
            include_bytes!("../../tests/fixtures/epub/svg.epub"),
            &cancel,
        )
        .unwrap();
        let document = book.document(1, "OPS/chapters/2-first.svg").unwrap();
        assert_eq!(
            (document.width, document.height, document.fixed),
            (200, 300, true)
        );
        assert!(
            document.html.contains("margin:0")
                && document.html.contains("data:image/png;base64,")
                && document.html.contains("SVG FIRST 74829")
        );
    }
    #[test]
    fn encrypted_content_and_oversized_styles_are_explicit_errors() {
        let bytes = changed("META-INF/encryption.xml", br#"<encryption><EncryptedData><EncryptionMethod Algorithm='drm:example'/><CipherData><CipherReference URI='OPS/chapters/2-first.xhtml'/></CipherData></EncryptedData></encryption>"#);
        let cancel = AtomicBool::new(false);
        assert_eq!(
            Book::open(&bytes, &cancel).err().unwrap().code,
            ErrorCode::Encrypted
        );
        let bytes = changed("OPS/styles/main.css", &vec![b' '; MAX_TEXT + 1]);
        let mut book = Book::open(&bytes, &cancel).unwrap();
        assert_eq!(
            book.document(1, "OPS/chapters/2-first.xhtml")
                .unwrap_err()
                .code,
            ErrorCode::Limit
        );
    }
    #[test]
    fn idpf_and_adobe_font_entries_are_unobfuscated_with_the_package_identifier() {
        let cancel = AtomicBool::new(false);
        let mut book = Book::open(FIXTURE, &cancel).unwrap();
        let package = String::from_utf8(book.read("OPS/package.opf", MAX_TEXT).unwrap())
            .unwrap()
            .replace(
                "</manifest>",
                "<item id='font' href='fonts/body.ttf' media-type='font/ttf'/></manifest>",
            );
        let original: Vec<u8> = (0..1600).map(|n| (n % 251) as u8).collect();
        // Deterministic payload tests metadata/key/chunk boundaries only;
        // this is not a substitute for browser rendering of an actual font.
        for (algorithm, key, length) in [
            (
                "http://www.idpf.org/2008/embedding",
                Sha1::digest(b"urn:uuid:00112233-4455-6677-8899-aabbccddeeff").to_vec(),
                1040,
            ),
            (
                "http://ns.adobe.com/pdf/enc#RC",
                vec![
                    0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc,
                    0xdd, 0xee, 0xff,
                ],
                1024,
            ),
        ] {
            let encoded: Vec<_> = original
                .iter()
                .enumerate()
                .map(|(i, b)| b ^ if i < length { key[i % key.len()] } else { 0 })
                .collect();
            let encryption = format!("<encryption><EncryptedData><EncryptionMethod Algorithm='{algorithm}'/><CipherData><CipherReference URI='OPS/fonts/body.ttf'/></CipherData></EncryptedData></encryption>");
            let bytes = changed("OPS/package.opf", package.as_bytes());
            let bytes = changed_in(&bytes, "META-INF/encryption.xml", encryption.as_bytes());
            let bytes = changed_in(&bytes, "OPS/fonts/body.ttf", &encoded);
            let mut book = Book::open(&bytes, &cancel).unwrap();
            let uri = book
                .resource("OPS/chapters/a.xhtml", "../fonts/body.ttf", false)
                .unwrap();
            let decoded = base64::engine::general_purpose::STANDARD
                .decode(uri.split_once(',').unwrap().1)
                .unwrap();
            assert_eq!(decoded, original, "{algorithm}");
            assert_eq!(book.read("OPS/fonts/body.ttf", 1 << 20).unwrap(), encoded);
        }
    }
}
