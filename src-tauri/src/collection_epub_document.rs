//! Unscripted author documents, with local CSS/images and bounded viewports.
use super::{check, css_uri, error, Book, Document, ErrorCode, Result, MAX_DOCUMENT, MAX_TEXT};
use scraper::{ElementRef, Html, Selector};
use std::collections::HashMap;
impl Book<'_> {
    fn element(&mut self, element: ElementRef<'_>, base: &str, depth: usize) -> Result<String> {
        check(self.cancel)?;
        if depth > 128 {
            return Err(error(ErrorCode::Limit));
        }
        let tag = element.value().name();
        if matches!(
            tag,
            "script"
                | "iframe"
                | "object"
                | "embed"
                | "form"
                | "input"
                | "button"
                | "select"
                | "textarea"
                | "audio"
                | "video"
                | "base"
                | "meta"
                | "title"
        ) {
            return Ok(String::new());
        }
        if tag == "style" {
            let css = self.css(base, &element.text().collect::<String>())?;
            return Ok(format!("<style>{}</style>", css.replace('<', "\\3c ")));
        }
        if tag == "link" {
            if element.attr("rel").is_some_and(|value| {
                value
                    .split_whitespace()
                    .any(|s| s.eq_ignore_ascii_case("stylesheet"))
            }) {
                let reference = element
                    .attr("href")
                    .ok_or_else(|| error(ErrorCode::Unsupported))?;
                let uri = self.resource(base, reference, true)?;
                return Ok(format!(
                    "<style>{}</style>",
                    css_uri(&uri)?.replace('<', "\\3c ")
                ));
            }
            return Ok(String::new());
        }
        let allowed = matches!(
            tag,
            "html"
                | "body"
                | "article"
                | "section"
                | "main"
                | "header"
                | "footer"
                | "nav"
                | "aside"
                | "div"
                | "span"
                | "p"
                | "h1"
                | "h2"
                | "h3"
                | "h4"
                | "h5"
                | "h6"
                | "br"
                | "hr"
                | "wbr"
                | "em"
                | "strong"
                | "b"
                | "i"
                | "u"
                | "s"
                | "del"
                | "ins"
                | "blockquote"
                | "pre"
                | "code"
                | "kbd"
                | "samp"
                | "small"
                | "sub"
                | "sup"
                | "ul"
                | "ol"
                | "li"
                | "dl"
                | "dt"
                | "dd"
                | "table"
                | "tbody"
                | "thead"
                | "tfoot"
                | "tr"
                | "td"
                | "th"
                | "caption"
                | "col"
                | "colgroup"
                | "img"
                | "figure"
                | "figcaption"
                | "ruby"
                | "rb"
                | "rt"
                | "rp"
                | "rtc"
                | "a"
                | "abbr"
                | "address"
                | "cite"
                | "q"
                | "time"
                | "mark"
                | "svg"
                | "g"
                | "path"
                | "rect"
                | "circle"
                | "ellipse"
                | "line"
                | "polyline"
                | "polygon"
                | "text"
                | "tspan"
                | "image"
                | "defs"
                | "symbol"
                | "use"
                | "clipPath"
                | "mask"
                | "linearGradient"
                | "radialGradient"
                | "stop"
                | "pattern"
                | "foreignObject"
        );
        let mut result = String::new();
        if allowed {
            result.push('<');
            result.push_str(tag);
            for (name, value) in element.value().attrs() {
                let value = if name == "style" {
                    Some(self.css(base, value)?)
                } else if matches!(name, "src" | "href" | "xlink:href")
                    && matches!(tag, "img" | "image" | "use")
                {
                    Some(self.resource(base, value, false)?)
                } else if (tag == "a" && name == "href" && value.starts_with('#'))
                    || matches!(
                        name,
                        "id" | "class"
                            | "title"
                            | "lang"
                            | "xml:lang"
                            | "dir"
                            | "role"
                            | "alt"
                            | "width"
                            | "height"
                            | "colspan"
                            | "rowspan"
                            | "start"
                            | "value"
                            | "type"
                            | "reversed"
                            | "scope"
                            | "viewBox"
                            | "preserveAspectRatio"
                            | "x"
                            | "y"
                            | "x1"
                            | "x2"
                            | "y1"
                            | "y2"
                            | "cx"
                            | "cy"
                            | "r"
                            | "rx"
                            | "ry"
                            | "points"
                            | "d"
                            | "transform"
                            | "offset"
                            | "gradientUnits"
                            | "gradientTransform"
                            | "patternUnits"
                            | "patternTransform"
                            | "fill-rule"
                            | "text-anchor"
                            | "dominant-baseline"
                            | "font-size"
                            | "font-family"
                            | "font-weight"
                    )
                    || name.starts_with("aria-")
                {
                    Some(value.into())
                } else if matches!(
                    name,
                    "fill"
                        | "stroke"
                        | "clip-path"
                        | "mask"
                        | "filter"
                        | "stop-color"
                        | "stop-opacity"
                        | "stroke-width"
                        | "opacity"
                ) {
                    Some(self.css(base, value)?)
                } else {
                    None
                };
                if let Some(value) = value {
                    result.push(' ');
                    result.push_str(name);
                    result.push_str("=\"");
                    result.push_str(&escaped(&value));
                    result.push('"');
                }
                if result.len() > MAX_DOCUMENT {
                    return Err(error(ErrorCode::Limit));
                }
            }
            result.push('>');
        }
        for child in element.children() {
            if let Some(child) = ElementRef::wrap(child) {
                result.push_str(&self.element(child, base, depth + 1)?);
            } else if let scraper::Node::Text(text) = child.value() {
                result.push_str(&escaped(text));
            }
            if result.len() > MAX_DOCUMENT {
                return Err(error(ErrorCode::Limit));
            }
        }
        if allowed && !matches!(tag, "img" | "br" | "hr" | "wbr" | "col") {
            result.push_str("</");
            result.push_str(tag);
            result.push('>');
        }
        Ok(result)
    }
    pub(crate) fn document(&mut self, number: u32, entry: &str) -> Result<Document> {
        let chapter = self
            .chapters
            .get(number.saturating_sub(1) as usize)
            .filter(|chapter| chapter.entry == entry && number != 0)
            .cloned()
            .ok_or_else(|| error(ErrorCode::SourceChanged))?;
        let bytes = self.read(entry, MAX_TEXT)?;
        let source = std::str::from_utf8(&bytes).map_err(|_| error(ErrorCode::Unsupported))?;
        let svg = self.items.get(entry).map(String::as_str) == Some("image/svg+xml");
        let source = if svg {
            format!("<html><body style=\"margin:0\">{source}</body></html>")
        } else {
            source.into()
        };
        let parsed = Html::parse_document(&source);
        let mut width = 600;
        let mut height = 800;
        if svg {
            let root = parsed
                .select(&Selector::parse("svg").unwrap())
                .next()
                .ok_or_else(|| error(ErrorCode::Unsupported))?;
            let view_box: Vec<_> = root
                .attr("viewBox")
                .unwrap_or("")
                .split([' ', ','])
                .filter(|v| !v.is_empty())
                .map(str::parse::<f64>)
                .collect();
            let dimensions = root
                .attr("width")
                .zip(root.attr("height"))
                .and_then(|(w, h)| {
                    Some((
                        w.trim_end_matches("px").parse::<f64>().ok()?,
                        h.trim_end_matches("px").parse::<f64>().ok()?,
                    ))
                })
                .or_else(|| {
                    if view_box.len() == 4
                        && view_box
                            .iter()
                            .all(|n| n.as_ref().is_ok_and(|v| v.is_finite()))
                    {
                        Some((*view_box[2].as_ref().ok()?, *view_box[3].as_ref().ok()?))
                    } else {
                        None
                    }
                });
            if let Some((w, h)) = dimensions {
                if !w.is_finite()
                    || !h.is_finite()
                    || !(1.0..=8192.0).contains(&w)
                    || !(1.0..=8192.0).contains(&h)
                    || w * h > 32_000_000.0
                {
                    return Err(error(ErrorCode::Limit));
                }
                width = w.ceil() as u32;
                height = h.ceil() as u32;
            } else {
                return Err(error(ErrorCode::Unsupported));
            }
        }
        for meta in parsed.select(&Selector::parse("meta[name=viewport]").unwrap()) {
            let mut dimensions = HashMap::new();
            for pair in meta.attr("content").unwrap_or("").split([',', ';']) {
                if let Some((key, value)) = pair.trim().split_once('=') {
                    if let Ok(number) = value.trim().parse::<f64>() {
                        dimensions.insert(key.trim(), number);
                    }
                }
            }
            if let (Some(&w), Some(&h)) = (dimensions.get("width"), dimensions.get("height")) {
                if w.is_finite()
                    && h.is_finite()
                    && (1.0..=8192.0).contains(&w)
                    && (1.0..=8192.0).contains(&h)
                    && w * h <= 32_000_000.0
                {
                    width = w.ceil() as u32;
                    height = h.ceil() as u32;
                } else {
                    return Err(error(ErrorCode::Limit));
                }
            }
        }
        let mut head = parsed
            .select(&Selector::parse("head").unwrap())
            .next()
            .map(|e| self.element(e, entry, 0))
            .transpose()?
            .unwrap_or_default();
        if svg {
            head.push_str(&format!(
                "<style>body{{width:{width}px;height:{height}px;margin:0}}</style>"
            ));
        }
        let body = parsed
            .select(&Selector::parse("body").unwrap())
            .next()
            .ok_or_else(|| error(ErrorCode::Unsupported))?;
        let direction = body.attr("dir").unwrap_or(&self.direction).to_string();
        let body = self.element(body, entry, 0)?;
        // A second, frame-local CSP complements the iframe sandbox and native
        // URL rewriting. HTML never inherits Prism's IPC/script capabilities.
        let html = format!("<!doctype html><html xmlns=\"http://www.w3.org/1999/xhtml\" dir=\"{}\"><head><meta charset=\"utf-8\"/><meta http-equiv=\"Content-Security-Policy\" content=\"default-src 'none'; script-src 'none'; connect-src 'none'; img-src data:; style-src 'unsafe-inline' data:; font-src data:; base-uri 'none'; form-action 'none'\"/><style>html{{background:white;color:black;font-family:serif;overflow-wrap:break-word}}body{{margin:24px}}img,svg{{max-width:100%}}*{{animation:none!important;transition:none!important}}</style>{head}</head>{body}</html>", escaped(&direction));
        if html.len() > MAX_DOCUMENT {
            return Err(error(ErrorCode::Limit));
        }
        check(self.cancel)?;
        Ok(Document {
            html,
            width,
            height,
            fixed: chapter.fixed || svg,
        })
    }
}

fn escaped(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}
