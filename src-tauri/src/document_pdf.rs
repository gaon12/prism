//! Portable, offline PDF typesetting of a saved document revision. The World
//! exposes only generated source and bundled fonts, never filesystem/network
//! resolvers, system fonts, packages, environment values or a system clock.
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, LazyLock,
    },
    time::Instant,
};
use typst::{
    diag::{FileError, FileResult},
    foundations::{Bytes, Datetime, Duration},
    syntax::{FileId, RootedPath, Source, VirtualPath, VirtualRoot},
    text::{Font, FontBook},
    utils::LazyHash,
    Library, LibraryExt, World,
};
#[path = "document_pdf_markdown.rs"]
mod markdown;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Code {
    Busy,
    Input,
    Limit,
    Cancelled,
    TimedOut,
    Render,
    Storage,
}
pub type Result<T> = std::result::Result<T, Code>;
pub struct Control {
    pub cancel: Arc<AtomicBool>,
    pub deadline: Instant,
}
impl Control {
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
pub struct Pdf {
    pub bytes: Vec<u8>,
    pub pages: usize,
    pub omitted_images: usize,
}
const MAX_MARKDOWN: usize = 512 << 10;
const MAX_SOURCE: usize = 4 << 20;
pub const MAX_PDF: usize = 32 << 20;
struct Fonts {
    book: LazyHash<FontBook>,
    fonts: Vec<Font>,
}
static FONTS: LazyLock<Fonts> = LazyLock::new(|| {
    let fonts = [
        include_bytes!("../assets/document-fonts/NotoSansCJKkr-Regular.otf").as_slice(),
        include_bytes!("../assets/document-fonts/NotoSansCJKkr-Bold.otf").as_slice(),
        include_bytes!("../assets/document-fonts/NotoSansMono.ttf").as_slice(),
        include_bytes!("../assets/document-fonts/NotoEmoji.ttf").as_slice(),
    ]
    .into_iter()
    .map(|data| Font::new(Bytes::new(data), 0).expect("Pinned bundled document font must be valid"))
    .collect::<Vec<_>>();
    Fonts {
        book: LazyHash::new(FontBook::from_fonts(&fonts)),
        fonts,
    }
});
static LIBRARY: LazyLock<LazyHash<Library>> = LazyLock::new(|| LazyHash::new(Library::default()));
struct MemoryWorld<'a> {
    source: Source,
    control: &'a Control,
}
impl World for MemoryWorld<'_> {
    fn library(&self) -> &LazyHash<Library> {
        &LIBRARY
    }
    fn book(&self) -> &LazyHash<FontBook> {
        &FONTS.book
    }
    fn main(&self) -> FileId {
        self.source.id()
    }
    fn source(&self, id: FileId) -> FileResult<Source> {
        if id == self.main() && self.control.check().is_ok() {
            Ok(self.source.clone())
        } else {
            Err(FileError::AccessDenied)
        }
    }
    fn file(&self, id: FileId) -> FileResult<Bytes> {
        if id == self.main() && self.control.check().is_ok() {
            Ok(Bytes::from_string(self.source.text().to_owned()))
        } else {
            Err(FileError::AccessDenied)
        }
    }
    fn font(&self, index: usize) -> Option<Font> {
        self.control.check().ok()?;
        FONTS.fonts.get(index).cloned()
    }
    fn today(&self, _: Option<Duration>) -> Option<Datetime> {
        None
    }
}
struct Evict;
impl Drop for Evict {
    fn drop(&mut self) {
        typst::comemo::evict(0);
    }
}

pub fn render(title: &str, text: &str, language: &str, control: &Control) -> Result<Pdf> {
    control.check()?;
    if title.chars().count() > 200
        || title.contains('\0')
        || text.len() > MAX_MARKDOWN
        || text.contains('\0')
    {
        return Err(Code::Input);
    }
    let _evict = Evict;
    let body = markdown::convert(text, language)?;
    let language = match language {
        "ko" => "ko",
        "ja" => "ja",
        "zh-CN" | "zh-TW" => "zh",
        _ => "en",
    };
    let source = format!(
        r##"#set document(title: {}, author: "Prism", date: none)
#set page(paper: "a4", margin: (x: 17mm, y: 18mm), numbering: "1 / 1")
#set text(font: ("Noto Sans CJK KR", "Noto Emoji", "Noto Sans Mono"), size: 10pt, lang: "{}", fallback: false)
#set par(leading: 0.65em, spacing: 0.8em)
#show raw: set text(font: ("Noto Sans Mono", "Noto Sans CJK KR"), size: 9pt)
#set heading(numbering: none)
#set table(stroke: 0.4pt + luma(70%), inset: 5pt)
{}
"##,
        markdown::string(title),
        language,
        body.source
    );
    if source.len() > MAX_SOURCE {
        return Err(Code::Limit);
    }
    control.check()?;
    let world = MemoryWorld {
        source: Source::new(
            FileId::new(RootedPath::new(
                VirtualRoot::Project,
                VirtualPath::new("/prism-document.typ").unwrap(),
            )),
            source,
        ),
        control,
    };
    let document = typst::compile::<typst_layout::PagedDocument>(&world).output;
    control.check()?;
    let document = document.map_err(|_| Code::Render)?;
    let pages = document.pages().len();
    if pages == 0 || pages > 500 {
        return Err(Code::Limit);
    }
    let output = typst_pdf::pdf(&document, &typst_pdf::PdfOptions::default());
    control.check()?;
    let bytes = output.map_err(|_| Code::Render)?;
    if bytes.len() > MAX_PDF || !bytes.starts_with(b"%PDF-") {
        return Err(Code::Limit);
    }
    Ok(Pdf {
        bytes,
        pages,
        omitted_images: body.omitted_images,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration as StdDuration;
    fn control() -> Control {
        Control {
            cancel: Arc::new(AtomicBool::new(false)),
            deadline: Instant::now() + StdDuration::from_secs(30),
        }
    }
    #[test]
    fn actual_portable_pdf_embeds_multilingual_text_tables_code_and_notes() {
        let markdown = "# 프리즘 보고서\n\n한국어 中文 日本語 English 😀 74829\n\n\
| 항목 | 수치 |\n|---|---:|\n| 예산 | 12,345 |\n\n\
**강조**와 *기울임*, `#read(\"private\")` 코드. 근거[^1]\n\n\
[^1]: original source, page 2; claim not verified\n\n\
```rust\nlet text = \"한글\";\n```\n\n\
![설명](https://example.invalid/private.png)\n";
        let pdf = render("프리즘 보고서", markdown, "ko", &control()).unwrap();
        assert_eq!(pdf.pages, 1);
        assert_eq!(pdf.omitted_images, 1);
        let document = pdf_extract::Document::load_mem(&pdf.bytes).unwrap();
        let text = pdf_extract::extract_text_from_mem(&pdf.bytes).unwrap();
        for expected in [
            "한국어",
            "中文",
            "日本語",
            "English",
            "74829",
            "😀",
            "12,345",
            "private",
            "original source",
        ] {
            assert!(text.contains(expected), "Missing text {expected}: {text}");
        }
        assert_eq!(document.get_pages().len(), 1);
    }
    #[test]
    fn source_access_is_closed_and_cancellation_precedes_typesetting() {
        let control = control();
        let world = MemoryWorld {
            source: Source::new(
                FileId::new(RootedPath::new(
                    VirtualRoot::Project,
                    VirtualPath::new("/prism-document.typ").unwrap(),
                )),
                "safe".into(),
            ),
            control: &control,
        };
        let other = FileId::new(RootedPath::new(
            VirtualRoot::Project,
            VirtualPath::new("/private/file").unwrap(),
        ));
        assert!(world.file(other).is_err() && world.source(other).is_err());
        control.cancel.store(true, Ordering::Release);
        assert_eq!(
            render("a", "a", "en", &control).err(),
            Some(Code::Cancelled)
        );
    }
    #[test]
    fn actual_multipage_report_keeps_lists_quotes_notes_and_repeated_table_headers() {
        let mut text="# 보고서\n\n> quoted evidence\n\n- first item\n  - nested item\n\n1. ordered item\n2. next item\n\n~~removed~~\n\n---\n\nReference[^a] and again[^a].\n\n[^a]: Saved source note\n\n| 반복 머리글 | 수치 |\n|---|---|\n".to_string();
        for i in 0..180 {
            text.push_str(&format!("| 자료 {i} 한국어 中文 日本語 | {i} |\n"));
        }
        let pdf = render("보고서", &text, "ko", &control()).unwrap();
        assert!(pdf.pages >= 3);
        let extracted = pdf_extract::extract_text_from_mem(&pdf.bytes).unwrap();
        for value in [
            "quoted evidence",
            "first item",
            "nested item",
            "ordered item",
            "Saved source note",
            "자료 179",
        ] {
            assert!(extracted.contains(value), "Missing report content: {value}");
        }
        assert!(extracted.matches("반복 머리글").count() >= pdf.pages);
    }
}
