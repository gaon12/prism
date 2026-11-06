//! Declared EPUB navigation, never a filesystem/network navigation capability.
//! EPUB 3: https://www.w3.org/TR/epub-33/#sec-nav-toc
use super::{check, error, resolve, xml, Book, ErrorCode, Result, XmlNode, MAX_TEXT};
use scraper::{ElementRef, Html, Selector};
use std::collections::HashSet;

const MAX_POINTS: usize = 10_000;
const MAX_DEPTH: u32 = 32;

#[derive(Debug)]
pub(crate) struct NavigationPoint {
    pub title: String,
    pub depth: u32,
    pub entry: Option<String>,
    pub fragment: Option<String>,
}

fn title(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(1024)
        .collect()
}
fn fragment(value: &str) -> Option<String> {
    let mut bytes = Vec::new();
    let mut input = value.as_bytes().iter().copied();
    while let Some(byte) = input.next() {
        bytes.push(if byte == b'%' {
            let hi = (input.next()? as char).to_digit(16)?;
            let lo = (input.next()? as char).to_digit(16)?;
            (hi * 16 + lo) as u8
        } else {
            byte
        });
        if bytes.len() > 4096 {
            return None;
        }
    }
    let text = String::from_utf8(bytes).ok()?;
    (!text.chars().any(char::is_control)).then_some(text)
}
fn toc_type(nodes: &[XmlNode], index: usize) -> bool {
    nodes[index].attrs.iter().any(|(name, value)| {
        let Some(prefix) = name.strip_suffix(":type") else {
            return false;
        };
        if !value.split_whitespace().any(|v| v == "toc") {
            return false;
        }
        let namespace = format!("xmlns:{prefix}");
        let mut parent = Some(index);
        while let Some(i) = parent {
            if let Some(uri) = nodes[i].attr(&namespace) {
                return uri == "http://www.idpf.org/2007/ops";
            }
            parent = nodes[i].parent;
        }
        false
    })
}
impl Book<'_> {
    fn navigation_point(
        &self,
        base: &str,
        spine: &HashSet<&str>,
        href: Option<&str>,
        label: &str,
        depth: u32,
    ) -> NavigationPoint {
        let destination = href.and_then(|href| {
            let (path, id) = href
                .split_once('#')
                .map_or((href, None), |(p, f)| (p, Some(f)));
            let entry = if path.is_empty() {
                base.to_string()
            } else {
                resolve(base, path).ok()?
            };
            if !spine.contains(entry.as_str()) {
                return None;
            }
            let id = match id {
                Some(value) => Some(fragment(value)?),
                None => None,
            };
            Some((entry, id.filter(|v| !v.is_empty())))
        });
        let (entry, fragment) = destination.map_or((None, None), |(entry, id)| (Some(entry), id));
        NavigationPoint {
            title: title(label),
            depth,
            entry,
            fragment,
        }
    }
    fn nav_list(
        &self,
        base: &str,
        spine: &HashSet<&str>,
        list: ElementRef<'_>,
        depth: u32,
        points: &mut Vec<NavigationPoint>,
    ) -> Result<()> {
        check(self.cancel)?;
        if depth > MAX_DEPTH {
            return Err(error(ErrorCode::Limit));
        }
        for li in list
            .children()
            .filter_map(ElementRef::wrap)
            .filter(|e| e.value().name() == "li")
        {
            let label = li
                .children()
                .filter_map(ElementRef::wrap)
                .find(|e| matches!(e.value().name(), "a" | "span"));
            if let Some(label) = label {
                if points.len() >= MAX_POINTS {
                    return Err(error(ErrorCode::Limit));
                }
                let text = label.text().collect::<String>();
                let text = if text.trim().is_empty() {
                    label
                        .attr("aria-label")
                        .or(label.attr("title"))
                        .unwrap_or("")
                } else {
                    &text
                };
                let point = self.navigation_point(base, spine, label.attr("href"), text, depth);
                if !point.title.is_empty() {
                    points.push(point);
                }
            }
            for nested in li
                .children()
                .filter_map(ElementRef::wrap)
                .filter(|e| e.value().name() == "ol")
            {
                self.nav_list(base, spine, nested, depth + 1, points)?;
            }
        }
        Ok(())
    }
    fn ncx_list(
        &self,
        base: &str,
        spine: &HashSet<&str>,
        tree: (&[XmlNode], &[Vec<usize>]),
        parent: usize,
        depth: u32,
        points: &mut Vec<NavigationPoint>,
    ) -> Result<()> {
        let (nodes, children) = tree;
        check(self.cancel)?;
        if depth > MAX_DEPTH {
            return Err(error(ErrorCode::Limit));
        }
        for &index in &children[parent] {
            if nodes[index].name != "navPoint" {
                continue;
            }
            if points.len() >= MAX_POINTS {
                return Err(error(ErrorCode::Limit));
            }
            let label = children[index]
                .iter()
                .find(|&&i| nodes[i].name == "navLabel")
                .and_then(|&i| children[i].iter().find(|&&j| nodes[j].name == "text"))
                .map(|&i| nodes[i].text.as_str())
                .unwrap_or("");
            let href = children[index]
                .iter()
                .find(|&&i| nodes[i].name == "content")
                .and_then(|&i| nodes[i].attr("src"));
            let point = self.navigation_point(base, spine, href, label, depth);
            if !point.title.is_empty() {
                points.push(point);
            }
            if children[index].iter().any(|&i| nodes[i].name == "navPoint") {
                self.ncx_list(base, spine, tree, index, depth + 1, points)?;
            }
        }
        Ok(())
    }
    pub(crate) fn navigation(&mut self) -> Result<Vec<NavigationPoint>> {
        let mut points = Vec::new();
        if let Some(entry) = self.navigation_entry.clone() {
            let bytes = self.read(&entry, MAX_TEXT)?;
            // Validate well-formed XML/DTD boundaries, then use the HTML text
            // iterator to preserve mixed inline label order ("1 <em>A</em> end").
            let nodes = xml(&bytes)?;
            let spine = self.chapters.iter().map(|c| c.entry.as_str()).collect();
            if let Some(ordinal) = nodes
                .iter()
                .enumerate()
                .filter(|(_, n)| n.name == "nav")
                .position(|(i, _)| toc_type(&nodes, i))
            {
                let document = Html::parse_document(
                    std::str::from_utf8(&bytes).map_err(|_| error(ErrorCode::Unsupported))?,
                );
                let selector = Selector::parse("nav").unwrap();
                if let Some(nav) = document.select(&selector).nth(ordinal) {
                    if let Some(list) = nav
                        .children()
                        .filter_map(ElementRef::wrap)
                        .find(|e| e.value().name() == "ol")
                    {
                        self.nav_list(&entry, &spine, list, 0, &mut points)?;
                    }
                }
            }
        } else if let Some(entry) = self.ncx_entry.clone() {
            let nodes = xml(&self.read(&entry, MAX_TEXT)?)?;
            let spine = self.chapters.iter().map(|c| c.entry.as_str()).collect();
            let mut children = vec![Vec::new(); nodes.len()];
            for (i, n) in nodes.iter().enumerate() {
                if let Some(parent) = n.parent {
                    children[parent].push(i);
                }
            }
            if let Some(index) = nodes.iter().position(|n| n.name == "navMap") {
                self.ncx_list(&entry, &spine, (&nodes, &children), index, 0, &mut points)?;
            }
        }
        if points.is_empty() {
            if self.chapters.len() > MAX_POINTS {
                return Err(error(ErrorCode::Limit));
            }
            points = self
                .chapters
                .iter()
                .map(|chapter| NavigationPoint {
                    title: title(chapter.entry.rsplit('/').next().unwrap_or(&chapter.entry)),
                    depth: 0,
                    entry: Some(chapter.entry.clone()),
                    fragment: None,
                })
                .collect();
        }
        Ok(points)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{Cursor, Read, Write},
        sync::atomic::{AtomicBool, Ordering},
    };
    const NAV: &[u8] = include_bytes!("../../tests/fixtures/epub/navigation.epub");
    fn replaced(nav: &str) -> Vec<u8> {
        let mut source = zip::ZipArchive::new(Cursor::new(NAV)).unwrap();
        let mut target = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for i in 0..source.len() {
            let mut file = source.by_index(i).unwrap();
            let name = file.name().unwrap().into_owned();
            let mut bytes = Vec::new();
            file.read_to_end(&mut bytes).unwrap();
            target
                .start_file(&name, zip::write::SimpleFileOptions::default())
                .unwrap();
            target
                .write_all(if name == "OPS/nav.xhtml" {
                    nav.as_bytes()
                } else {
                    &bytes
                })
                .unwrap();
        }
        target.finish().unwrap().into_inner()
    }
    #[test]
    fn independent_nav_preserves_hierarchy_inline_labels_and_literal_unicode_ids() {
        let cancel = AtomicBool::new(false);
        let mut book = Book::open(NAV, &cancel).unwrap();
        let points = book.navigation().unwrap();
        assert_eq!(points.len(), 9);
        assert_eq!(points[0].title, "제1부");
        assert!(points[0].entry.is_none());
        assert_eq!(
            (points[1].title.as_str(), points[1].depth),
            ("1 시작 뒤", 1)
        );
        assert_eq!(points[1].entry.as_deref(), Some("OPS/chapters/first.xhtml"));
        assert_eq!(points[1].fragment.as_deref(), Some("시작"));
        assert_eq!(points[2].fragment.as_deref(), Some("후반:위치"));
        assert!(points[3].fragment.is_none());
        assert_eq!(points[4].fragment.as_deref(), Some("missing"));
        assert!(
            points[5..].iter().all(|p| p.entry.is_none()),
            "external, escaped, non-spine and malformed references never become destinations"
        );
        cancel.store(true, Ordering::Relaxed);
        assert_eq!(book.navigation().unwrap_err().code, ErrorCode::Cancelled);
    }
    #[test]
    fn legacy_ncx_and_absent_contents_follow_declared_spine() {
        let cancel = AtomicBool::new(false);
        let ncx = include_bytes!("../../tests/fixtures/epub/navigation-ncx.epub");
        let points = Book::open(ncx, &cancel).unwrap().navigation().unwrap();
        assert_eq!(points.len(), 2);
        assert_eq!((points[0].title.as_str(), points[0].depth), ("NCX 시작", 0));
        assert_eq!((points[1].title.as_str(), points[1].depth), ("NCX 후반", 1));
        assert_eq!(points[1].fragment.as_deref(), Some("후반:위치"));
        let fallback = include_bytes!("../../tests/fixtures/epub/navigation-spine.epub");
        let points = Book::open(fallback, &cancel).unwrap().navigation().unwrap();
        assert_eq!(
            points.iter().map(|p| p.title.as_str()).collect::<Vec<_>>(),
            vec!["first.xhtml", "second.xhtml"]
        );
    }
    #[test]
    fn malformed_or_unbounded_navigation_does_not_replace_readable_content() {
        let cancel = AtomicBool::new(false);
        let large = format!("<html xmlns:e='http://www.idpf.org/2007/ops'><body><nav e:type='toc'><ol>{}</ol></nav></body></html>", "<li><a href='chapters/first.xhtml'>one</a></li>".repeat(MAX_POINTS + 1));
        let bytes = replaced(&large);
        let mut book = Book::open(&bytes, &cancel).unwrap();
        assert_eq!(book.navigation().unwrap_err().code, ErrorCode::Limit);
        assert!(book
            .document(1, "OPS/chapters/first.xhtml")
            .unwrap()
            .html
            .contains("NAV FIRST CHAPTER"));
        let bytes = replaced("<!DOCTYPE html><html/>");
        assert_eq!(
            Book::open(&bytes, &cancel)
                .unwrap()
                .navigation()
                .unwrap_err()
                .code,
            ErrorCode::Unsupported
        );
        let bytes = replaced("<html xmlns:e='https://example.invalid/spoof'><body><nav e:type='toc'><ol><li><a href='chapters/second.xhtml'>wrong namespace</a></li></ol></nav></body></html>");
        assert_eq!(
            Book::open(&bytes, &cancel).unwrap().navigation().unwrap()[0].title,
            "first.xhtml"
        );
        assert!(fragment("%FF").is_none());
        assert!(fragment("trailing%").is_none());
    }
}
