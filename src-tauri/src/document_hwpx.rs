//! Pure, bounded HWPX generation; callers own admission, saved revisions and
//! atomic destination publication. This module never resolves document assets.
//! 본 제품은 한컴의 HWP 문서 파일(.hwp) 공개 문서를 참고하여 개발하였습니다.
use hwpforge_core::{metadata::Metadata, ImageStore, Run, RunContent};
use hwpforge_foundation::{Alignment, CharShapeIndex, FontIndex, ParaShapeIndex};
use hwpforge_smithy_hwpx::{
    style_store::HwpxFont, EncodeOptions, HwpxEncoder, HwpxRegistryBridge, HwpxStyleStore,
};
use hwpforge_smithy_md::MdDecoder;
use std::{
    collections::{HashMap, HashSet},
    io::{Cursor, Read},
    sync::atomic::{AtomicBool, Ordering},
    time::Instant,
};
#[path = "document_hwpx_markdown.rs"]
mod markdown;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Code {
    Input,
    Limit,
    Cancelled,
    TimedOut,
    Unsupported,
    Encode,
}
pub type Result<T> = std::result::Result<T, Code>;
pub struct Control<'a> {
    pub cancel: &'a AtomicBool,
    pub deadline: Instant,
}
impl Control<'_> {
    pub fn check(&self) -> Result<()> {
        if self.cancel.load(Ordering::Acquire) {
            Err(Code::Cancelled)
        } else if Instant::now() >= self.deadline {
            Err(Code::TimedOut)
        } else {
            Ok(())
        }
    }
}
pub struct Export {
    pub bytes: Vec<u8>,
    pub omitted_images: usize,
    pub literal_html: usize,
}
fn xml_character(c: char) -> bool {
    matches!(c, '\t' | '\n' | '\r') || (c >= '\u{20}' && !matches!(c, '\u{fffe}' | '\u{ffff}'))
}
fn token(value: &str, prefix: &str) -> Result<(char, usize, usize)> {
    let tail = value.strip_prefix(prefix).ok_or(Code::Encode)?;
    let (body, _) = tail.split_once("END").ok_or(Code::Encode)?;
    let kind = body.chars().next().ok_or(Code::Encode)?;
    let index = body
        .get(1..)
        .ok_or(Code::Encode)?
        .parse()
        .map_err(|_| Code::Encode)?;
    Ok((kind, index, prefix.len() + body.len() + 3))
}
struct Styles {
    mono: FontIndex,
    codes: HashMap<usize, CharShapeIndex>,
    alignments: HashMap<(usize, u8), ParaShapeIndex>,
}
impl Styles {
    fn code(&mut self, base: CharShapeIndex, store: &mut HwpxStyleStore) -> Result<CharShapeIndex> {
        if let Some(id) = self.codes.get(&base.get()) {
            return Ok(*id);
        }
        if self.codes.len() >= 256 {
            return Err(Code::Limit);
        }
        let mut shape = store.char_shape(base).map_err(|_| Code::Encode)?.clone();
        // HWPX fontRef indices belong to their language group, not the complete
        // font vector. Keep CJK references and clone the surrounding formatting.
        shape.font_ref.latin = self.mono;
        let id = store.push_char_shape(shape);
        self.codes.insert(base.get(), id);
        Ok(id)
    }
    fn align(
        &mut self,
        base: ParaShapeIndex,
        align: pulldown_cmark::Alignment,
        store: &mut HwpxStyleStore,
    ) -> Result<ParaShapeIndex> {
        let (kind, alignment) = match align {
            pulldown_cmark::Alignment::None | pulldown_cmark::Alignment::Left => {
                (0, Alignment::Left)
            }
            pulldown_cmark::Alignment::Center => (1, Alignment::Center),
            pulldown_cmark::Alignment::Right => (2, Alignment::Right),
        };
        if let Some(id) = self.alignments.get(&(base.get(), kind)) {
            return Ok(*id);
        }
        if self.alignments.len() >= 512 {
            return Err(Code::Limit);
        }
        let mut shape = store.para_shape(base).map_err(|_| Code::Encode)?.clone();
        shape.alignment = alignment;
        let id = store.push_para_shape(shape);
        self.alignments.insert((base.get(), kind), id);
        Ok(id)
    }
}
fn rewrite_runs(
    paragraph: &mut hwpforge_core::Paragraph,
    prepared: &markdown::Prepared,
    store: &mut HwpxStyleStore,
    styles: &mut Styles,
    seen_codes: &mut HashSet<usize>,
    seen_tables: &mut HashSet<usize>,
) -> Result<()> {
    let old = std::mem::take(&mut paragraph.runs);
    let empty_style = old
        .first()
        .map_or(CharShapeIndex::new(0), |r| r.char_shape_id);
    for run in old {
        let RunContent::Text(text) = &run.content else {
            paragraph.runs.push(run);
            continue;
        };
        let mut rest = text.as_str();
        while let Some(start) = rest.find(&prepared.prefix) {
            if start > 0 {
                paragraph
                    .runs
                    .push(Run::text(&rest[..start], run.char_shape_id));
            }
            let (kind, index, consumed) = token(&rest[start..], &prepared.prefix)?;
            match kind {
                'C' => {
                    let slot = prepared.codes.get(index).ok_or(Code::Encode)?;
                    if !rest[start..].starts_with(&slot.token) {
                        return Err(Code::Encode);
                    }
                    let style = styles.code(run.char_shape_id, store)?;
                    paragraph.runs.push(Run::text(slot.text.clone(), style));
                    seen_codes.insert(index);
                }
                'T' => {
                    let table = prepared.tables.get(index).ok_or(Code::Encode)?;
                    if !rest[start..].starts_with(&table.token) {
                        return Err(Code::Encode);
                    }
                    seen_tables.insert(index);
                }
                _ => return Err(Code::Encode),
            }
            rest = &rest[start + consumed..];
        }
        if !rest.is_empty() {
            paragraph.runs.push(Run::text(rest, run.char_shape_id));
        }
    }
    if paragraph.runs.is_empty() {
        paragraph.runs.push(Run::text("", empty_style));
    }
    Ok(())
}
fn validate_package(bytes: &[u8], prefix: &str, control: &Control<'_>) -> Result<()> {
    if bytes.len() > 32 << 20 {
        return Err(Code::Limit);
    }
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).map_err(|_| Code::Encode)?;
    if zip.len() > 64 {
        return Err(Code::Limit);
    }
    let mut total = 0usize;
    for i in 0..zip.len() {
        control.check()?;
        let entry = zip.by_index(i).map_err(|_| Code::Encode)?;
        if entry.size() > 8 << 20 {
            return Err(Code::Limit);
        }
        let mut content = Vec::new();
        entry
            .take((8 << 20) + 1)
            .read_to_end(&mut content)
            .map_err(|_| Code::Encode)?;
        total = total.checked_add(content.len()).ok_or(Code::Limit)?;
        if content.len() > 8 << 20 || total > 32 << 20 {
            return Err(Code::Limit);
        }
        if let Ok(text) = std::str::from_utf8(&content) {
            if text.contains(prefix) {
                return Err(Code::Unsupported);
            }
        }
    }
    Ok(())
}
pub fn render(title: &str, text: &str, language: &str, control: &Control<'_>) -> Result<Export> {
    control.check()?;
    if title.chars().count() > 200 || text.len() > 512 << 10 {
        return Err(Code::Limit);
    }
    if title.trim().is_empty() || !title.chars().chain(text.chars()).all(xml_character) {
        return Err(Code::Input);
    }
    let prepared = markdown::prepare(text, language, control)?;
    if title.contains(&prepared.prefix) {
        return Err(Code::Input);
    }
    // An initial empty line disables the codec's YAML frontmatter feature. A
    // saved draft is Markdown content, never a template/configuration file.
    let input = format!("\n{}", prepared.text);
    let parsed = MdDecoder::decode_with_default(&input).map_err(|_| Code::Unsupported)?;
    control.check()?;
    // Never silently publish content that the codec reports as lost.
    if !parsed.warnings.is_empty() {
        return Err(Code::Unsupported);
    }
    let bridge =
        HwpxRegistryBridge::from_registry(&parsed.style_registry).map_err(|_| Code::Encode)?;
    let mut document = bridge
        .rebind_draft_document(parsed.document)
        .map_err(|_| Code::Encode)?;
    *document.metadata_mut() = Metadata::default();
    document.metadata_mut().title = Some(title.to_owned());
    document.metadata_mut().author = Some("Prism".into());
    let mut store = bridge.into_style_store();
    let mono = store
        .iter_fonts()
        .filter(|font| font.lang == "LATIN")
        .count();
    store.push_font(HwpxFont::new(mono as u32, "Noto Sans Mono", "LATIN"));
    let mut styles = Styles {
        mono: FontIndex::new(mono),
        codes: HashMap::new(),
        alignments: HashMap::new(),
    };
    let mut failure = None;
    for section in document.sections_mut() {
        for paragraph in &mut section.paragraphs {
            paragraph.for_each_table_mut(|table| {
                if failure.is_some() {
                    return;
                }
                let result = (|| {
                    control.check()?;
                    let first = table
                        .rows
                        .first()
                        .and_then(|r| r.cells.first())
                        .ok_or(Code::Encode)?;
                    let text = first
                        .paragraphs
                        .iter()
                        .flat_map(|p| &p.runs)
                        .filter_map(|r| {
                            if let RunContent::Text(s) = &r.content {
                                Some(s.as_str())
                            } else {
                                None
                            }
                        })
                        .collect::<String>();
                    let start = text.find(&prepared.prefix).ok_or(Code::Encode)?;
                    let (kind, index, _) = token(&text[start..], &prepared.prefix)?;
                    if kind != 'T' {
                        return Err(Code::Encode);
                    }
                    let definition = prepared.tables.get(index).ok_or(Code::Encode)?;
                    for row in &mut table.rows {
                        if row.cells.len() != definition.alignments.len() {
                            return Err(Code::Encode);
                        }
                        for (cell, align) in row.cells.iter_mut().zip(&definition.alignments) {
                            for p in &mut cell.paragraphs {
                                p.para_shape_id =
                                    styles.align(p.para_shape_id, *align, &mut store)?;
                            }
                        }
                    }
                    Ok(())
                })();
                if let Err(code) = result {
                    failure = Some(code);
                }
            });
        }
    }
    if let Some(code) = failure {
        return Err(code);
    }
    let mut seen_codes = HashSet::new();
    let mut seen_tables = HashSet::new();
    for section in document.sections_mut() {
        section.for_each_paragraph_mut(|paragraph| {
            if failure.is_some() {
                return;
            }
            let result = control.check().and_then(|_| {
                rewrite_runs(
                    paragraph,
                    &prepared,
                    &mut store,
                    &mut styles,
                    &mut seen_codes,
                    &mut seen_tables,
                )
            });
            if let Err(code) = result {
                failure = Some(code);
            }
        });
    }
    if let Some(code) = failure {
        return Err(code);
    }
    if seen_codes.len() != prepared.codes.len() || seen_tables.len() != prepared.tables.len() {
        return Err(Code::Unsupported);
    }
    let validated = document.validate().map_err(|_| Code::Encode)?;
    control.check()?;
    let output = HwpxEncoder::encode_with_diagnostics(
        &validated,
        &store,
        &ImageStore::new(),
        EncodeOptions::default(),
    )
    .map_err(|_| Code::Encode)?;
    control.check()?;
    if !output.warnings.is_empty() {
        return Err(Code::Unsupported);
    }
    validate_package(&output.bytes, &prepared.prefix, control)?;
    Ok(Export {
        bytes: output.bytes,
        omitted_images: prepared.omitted_images,
        literal_html: prepared.literal_html,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    fn control(cancel: &AtomicBool) -> Control<'_> {
        Control {
            cancel,
            deadline: Instant::now() + Duration::from_secs(30),
        }
    }
    fn part(export: &Export, path: &str) -> String {
        let mut zip = zip::ZipArchive::new(Cursor::new(&export.bytes)).unwrap();
        let mut text = String::new();
        zip.by_name(path)
            .unwrap()
            .read_to_string(&mut text)
            .unwrap();
        text
    }
    #[test]
    fn actual_package_keeps_code_tables_notes_and_visible_classification() {
        let cancel = AtomicBool::new(false);
        let source = "# 보고서\n\n<!-- prism:fact; verified=false -->\n\n한국어 中文 日本語 English 😀 `exact <code>`\n\n근거[^s], 다시[^s].\n\n[^s]: 원본.md `note code`\n\n| 항목 | 수치 |\n|---|---:|\n| `cell code` | 120000원 |\n";
        let output = render("보고서", source, "ko", &control(&cancel)).unwrap();
        let body = part(&output, "Contents/section0.xml");
        let header = part(&output, "Contents/header.xml");
        for value in [
            "한국어",
            "中文",
            "日本語",
            "English",
            "😀",
            "exact &lt;code&gt;",
            "note code",
            "cell code",
            "120000원",
            "원본.md",
            "검증되지 않음",
        ] {
            assert!(body.contains(value), "Missing output: {value}");
        }
        assert!(!body.contains('`') && !body.contains("PRISMHWPX"));
        assert!(body.contains("<hp:tbl") && body.contains("<hp:footNote"));
        assert!(header.contains("Noto Sans Mono") && header.contains("horizontal=\"RIGHT\""));
        // Resolve the actual output references. A default unused RIGHT style
        // or an unused font declaration alone would not prove either behavior.
        let body_tree = roxmltree::Document::parse(&body).unwrap();
        let header_tree = roxmltree::Document::parse(&header).unwrap();
        let amount = body_tree
            .descendants()
            .find(|n| n.tag_name().name() == "t" && n.text() == Some("120000원"))
            .unwrap();
        let paragraph = amount
            .ancestors()
            .find(|n| n.tag_name().name() == "p")
            .unwrap();
        let paragraph_id = paragraph.attribute("paraPrIDRef").unwrap();
        let shape = header_tree
            .descendants()
            .find(|n| n.tag_name().name() == "paraPr" && n.attribute("id") == Some(paragraph_id))
            .unwrap();
        assert_eq!(
            shape
                .descendants()
                .find(|n| n.tag_name().name() == "align")
                .unwrap()
                .attribute("horizontal"),
            Some("RIGHT")
        );
        let code = body_tree
            .descendants()
            .find(|n| n.tag_name().name() == "t" && n.text() == Some("cell code"))
            .unwrap();
        let run = code
            .ancestors()
            .find(|n| n.tag_name().name() == "run")
            .unwrap();
        let char_id = run.attribute("charPrIDRef").unwrap();
        let shape = header_tree
            .descendants()
            .find(|n| n.tag_name().name() == "charPr" && n.attribute("id") == Some(char_id))
            .unwrap();
        let font_id = shape
            .descendants()
            .find(|n| n.tag_name().name() == "fontRef")
            .unwrap()
            .attribute("latin")
            .unwrap();
        let latin = header_tree
            .descendants()
            .find(|n| n.tag_name().name() == "fontface" && n.attribute("lang") == Some("LATIN"))
            .unwrap();
        assert_eq!(
            latin
                .children()
                .find(|n| n.tag_name().name() == "font" && n.attribute("id") == Some(font_id))
                .unwrap()
                .attribute("face"),
            Some("Noto Sans Mono")
        );
        let decoded = hwpforge_smithy_hwpx::HwpxDecoder::decode(&output.bytes).unwrap();
        assert_eq!(decoded.document.sections().len(), 1);
    }
    #[test]
    fn image_urls_html_and_unsafe_links_are_literals_not_resources() {
        let cancel = AtomicBool::new(false);
        let output = render("Literal", "![private](file:///private/secret.png)\n\n<script>do not execute</script>\n\n[caption](javascript:alert(1))", "en", &control(&cancel)).unwrap();
        let body = part(&output, "Contents/section0.xml");
        assert_eq!(output.omitted_images, 1);
        assert!(output.literal_html > 0);
        assert!(body.contains("Image omitted") && body.contains("&lt;script&gt;"));
        assert!(body.contains("caption"));
        assert!(
            !body.contains("file:///")
                && !body.contains("javascript:")
                && !body.contains("<hp:pic")
        );
    }
    #[test]
    fn empty_cells_and_code_in_links_preserve_content_without_private_markers() {
        let cancel = AtomicBool::new(false);
        let output = render("Cells","| | right |\n|---|---:|\n| | `body` |\n\n[link `code`](https://example.invalid)\n\n[^unused]: `not rendered`", "en", &control(&cancel)).unwrap();
        let body = part(&output, "Contents/section0.xml");
        assert!(body.contains("body") && body.contains("code"));
        assert!(!body.contains("PRISMHWPX") && !body.contains("not rendered"));
    }
    #[test]
    fn input_deadline_depth_and_repeated_note_expansion_are_bounded_before_decode() {
        let cancel = AtomicBool::new(true);
        assert_eq!(
            render("a", "b", "en", &control(&cancel)).err(),
            Some(Code::Cancelled)
        );
        let cancel = AtomicBool::new(false);
        let expired = Control {
            cancel: &cancel,
            deadline: Instant::now() - Duration::from_secs(1),
        };
        assert_eq!(render("a", "b", "en", &expired).err(), Some(Code::TimedOut));
        assert_eq!(
            render("a", "bad\0text", "en", &control(&cancel)).err(),
            Some(Code::Input)
        );
        assert_eq!(
            render("a", &"x".repeat((512 << 10) + 1), "en", &control(&cancel)).err(),
            Some(Code::Limit)
        );
        assert_eq!(
            render(
                "a",
                &format!("{}deep", "> ".repeat(40)),
                "en",
                &control(&cancel)
            )
            .err(),
            Some(Code::Limit)
        );
        let repeated = format!(
            "{}\n\n[^a]: {}",
            "again[^a] ".repeat(200),
            "x".repeat(20000)
        );
        assert_eq!(
            render("a", &repeated, "en", &control(&cancel)).err(),
            Some(Code::Limit)
        );
        assert_eq!(
            render("a", "loop[^a]\n\n[^a]: loop[^a]", "en", &control(&cancel)).err(),
            Some(Code::Input)
        );
    }
}
