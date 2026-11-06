//! Filling an existing DOCX/HWPX form in place (requirement Q46).
//!
//! A form has two kinds of fields:
//! - placeholders written as `{{name}}` (the same name may appear several times), and
//! - blanks: runs of three or more underscores (`___`, or full-width `＿＿＿`), numbered in
//!   document order as `blank:1`, `blank:2`, ...
//!
//! Filling replaces only those characters. Everything else in the package — styles, tables,
//! images, untouched XML parts, even each zip entry's compression — is copied unchanged, so
//! the result keeps the original formatting.
//!
//! Word processors often split one visible word over several runs (`{{na` + `me}}`), each with
//! its own formatting. Text is therefore matched per paragraph across runs; the replacement
//! goes into the run where the field starts, and the field's remaining characters are removed
//! from the following runs. Text nodes that contain markup (rare in HWPX) are left alone and
//! act as a boundary.

use std::{
    collections::{HashMap, HashSet},
    fs::{self, OpenOptions},
    io::{Cursor, Read, Write},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

/// Packages larger than this are refused (zip bomb guard), per XML part and in total.
const MAX_PART_BYTES: u64 = 32 * 1024 * 1024;
const MAX_TOTAL_BYTES: u64 = 256 * 1024 * 1024;
const MAX_ENTRIES: usize = 4096;
const MAX_KEY_CHARS: usize = 64;
/// Fields listed for one form (more than this is not a form a person fills by hand).
const MAX_FIELDS: usize = 2048;
/// Values sent for one save: how many, how long each, and all together.
const MAX_VALUES: usize = 2048;
const MAX_VALUE_BYTES: usize = 64 * 1024;
const MAX_VALUES_TOTAL: usize = 1024 * 1024;
/// Output limits, checked *while* building, so a value repeated in many places cannot make
/// Prism allocate a huge file: one changed XML part, and the whole new package.
const MAX_OUTPUT_PART: usize = 64 * 1024 * 1024;
const MAX_OUTPUT_TOTAL: usize = MAX_TOTAL_BYTES as usize;
/// One open or save never runs longer than this.
const DEADLINE: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Docx,
    Hwpx,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum FieldKind {
    Placeholder,
    Blank,
}

/// One fillable spot, listed in document order. Placeholders with the same key are listed once.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Field {
    pub key: String,
    pub kind: FieldKind,
    /// A little of the surrounding paragraph text, to show the user where the field is.
    pub context: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FillError {
    /// Not a DOCX/HWPX package.
    Unsupported,
    /// Damaged zip or XML part.
    Damaged,
    /// Over the size/entry limits above.
    TooLarge,
    /// A value contains characters XML 1.0 does not allow (control characters, U+FFFE/FFFF).
    Invalid,
    /// Took longer than `DEADLINE`.
    TimedOut,
}

/// Fails with `TimedOut` once the deadline has passed.
fn check(deadline: Instant) -> Result<(), FillError> {
    if Instant::now() >= deadline {
        Err(FillError::TimedOut)
    } else {
        Ok(())
    }
}

/// The parts of each format that hold body text, and their tag names.
struct Dialect {
    paragraph: &'static str,
    text: &'static str,
    /// Markup that ends the text node, adds a line break and reopens it, or None to use a space.
    line_break: Option<&'static str>,
    /// Attribute needed on a text node whose leading/trailing spaces must survive.
    preserve_space: Option<&'static str>,
}

const DOCX: Dialect = Dialect {
    paragraph: "w:p",
    text: "w:t",
    line_break: Some("</w:t><w:br/><w:t xml:space=\"preserve\">"),
    preserve_space: Some(" xml:space=\"preserve\""),
};
const HWPX: Dialect = Dialect {
    paragraph: "hp:p",
    text: "hp:t",
    line_break: None,
    preserve_space: None,
};

fn dialect(format: Format) -> &'static Dialect {
    match format {
        Format::Docx => &DOCX,
        Format::Hwpx => &HWPX,
    }
}

/// Which zip entries hold body text, in reading order (body first, then headers/footers).
fn text_parts(format: Format, names: &[String]) -> Vec<String> {
    let mut parts: Vec<String> = match format {
        Format::Docx => names
            .iter()
            .filter(|name| {
                *name == "word/document.xml"
                    || ((name.starts_with("word/header") || name.starts_with("word/footer"))
                        && name.ends_with(".xml"))
            })
            .cloned()
            .collect(),
        Format::Hwpx => names
            .iter()
            .filter(|name| name.starts_with("Contents/section") && name.ends_with(".xml"))
            .cloned()
            .collect(),
    };
    // document.xml / section0 first, then the rest in a stable natural order.
    parts.sort_by_key(|name| (name != "word/document.xml", name.len(), name.clone()));
    parts
}

fn detect(names: &[String]) -> Option<Format> {
    if names.iter().any(|name| name == "word/document.xml") {
        Some(Format::Docx)
    } else if names
        .iter()
        .any(|name| name.starts_with("Contents/section") && name.ends_with(".xml"))
    {
        Some(Format::Hwpx)
    } else {
        None
    }
}

/// Lists the fields of a form, in document order.
pub fn fields(package: &[u8]) -> Result<(Format, Vec<Field>), FillError> {
    let deadline = Instant::now() + DEADLINE;
    let (format, parts) = read_parts(package)?;
    let mut found = Vec::new();
    // Placeholder names already listed (the same name is filled everywhere at once).
    let mut seen: HashSet<String> = HashSet::new();
    let mut too_many = false;
    let mut blanks = 0usize;
    for (_, xml) in &parts {
        check(deadline)?;
        scan_part(
            xml,
            dialect(format),
            &mut blanks,
            usize::MAX,
            deadline,
            &mut |key, kind, context| {
                let new = kind == FieldKind::Blank || seen.insert(key.to_string());
                if new && found.len() < MAX_FIELDS {
                    found.push(Field {
                        key: key.to_string(),
                        kind,
                        context: context.to_string(),
                    });
                } else if new {
                    too_many = true;
                }
                None
            },
        )?;
    }
    if too_many {
        return Err(FillError::TooLarge);
    }
    Ok((format, found))
}

/// Checks the values before any work: counts, sizes and XML 1.0 characters.
fn validate_values(values: &HashMap<String, String>) -> Result<(), FillError> {
    if values.len() > MAX_VALUES {
        return Err(FillError::TooLarge);
    }
    let mut total = 0usize;
    for (key, value) in values {
        if key.len() > MAX_KEY_CHARS * 4 || value.len() > MAX_VALUE_BYTES {
            return Err(FillError::TooLarge);
        }
        total += key.len() + value.len();
        let allowed = |c: char| {
            matches!(c, '\t' | '\n' | '\r') || (c >= ' ' && c != '\u{FFFE}' && c != '\u{FFFF}')
        };
        if !value.chars().all(allowed) {
            return Err(FillError::Invalid);
        }
    }
    if total > MAX_VALUES_TOTAL {
        return Err(FillError::TooLarge);
    }
    Ok(())
}

/// Returns a copy of the form with the given fields filled. Keys without a value stay as they
/// were, so a partly filled form can be filled again later.
pub fn fill(package: &[u8], values: &HashMap<String, String>) -> Result<Vec<u8>, FillError> {
    let deadline = Instant::now() + DEADLINE;
    validate_values(values)?;
    let (format, parts) = read_parts(package)?;
    let mut blanks = 0usize;
    let mut changed: HashMap<String, String> = HashMap::new();
    // The new package is at most the old one plus every rewritten part.
    let mut output = package.len();
    for (name, xml) in &parts {
        check(deadline)?;
        let updated = scan_part(
            xml,
            dialect(format),
            &mut blanks,
            MAX_OUTPUT_PART,
            deadline,
            &mut |key, _, _| values.get(key).cloned(),
        )?;
        if updated != *xml {
            output = output.saturating_add(updated.len());
            if output > MAX_OUTPUT_TOTAL {
                return Err(FillError::TooLarge);
            }
            changed.insert(name.clone(), updated);
        }
    }
    check(deadline)?;
    rewrite(package, &changed)
}

/// What the native commands return for a picked form.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FormInfo {
    pub path: String,
    /// "docx" or "hwpx".
    pub format: &'static str,
    pub fields: Vec<Field>,
    /// SHA-256 of the file as it was read. Saving requires the same value, so values typed for
    /// `blank:3` can never land in a different spot after the form was edited elsewhere.
    pub fingerprint: String,
}

/// Error codes shown to the user (the frontend has a message for each).
pub fn error_code(error: FillError) -> &'static str {
    match error {
        FillError::Unsupported => "unsupported",
        FillError::Damaged => "damaged",
        FillError::TooLarge => "tooLarge",
        FillError::Invalid => "invalid",
        FillError::TimedOut => "timedOut",
    }
}

/// Reads a form file from disk (never more than the size limit).
fn read_file(path: &Path) -> Result<Vec<u8>, &'static str> {
    let file = fs::File::open(path).map_err(|_| "io")?;
    let size = file.metadata().map_err(|_| "io")?.len();
    if size > MAX_TOTAL_BYTES {
        return Err("tooLarge");
    }
    let mut bytes = Vec::with_capacity(size as usize);
    file.take(MAX_TOTAL_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "io")?;
    if bytes.len() as u64 > MAX_TOTAL_BYTES {
        return Err("tooLarge");
    }
    Ok(bytes)
}

fn fingerprint(bytes: &[u8]) -> String {
    use sha2::Digest;
    sha2::Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// "docx" or "hwpx" from the file name, or None for anything else.
fn form_extension(path: &Path) -> Option<&'static str> {
    let extension = path.extension()?.to_str()?.to_ascii_lowercase();
    match extension.as_str() {
        "docx" => Some("docx"),
        "hwpx" => Some("hwpx"),
        _ => None,
    }
}

/// Lists the fields of the form at `path`.
pub fn read_form(path: &Path) -> Result<FormInfo, &'static str> {
    if !path.is_absolute() || form_extension(path).is_none() {
        return Err("input");
    }
    let bytes = read_file(path)?;
    let (format, fields) = fields(&bytes).map_err(error_code)?;
    Ok(FormInfo {
        path: path.to_string_lossy().into_owned(),
        format: match format {
            Format::Docx => "docx",
            Format::Hwpx => "hwpx",
        },
        fields,
        fingerprint: fingerprint(&bytes),
    })
}

/// A staging file this save created. Dropping it removes the file, unless it was published.
struct Stage {
    path: PathBuf,
    published: bool,
}
impl Drop for Stage {
    fn drop(&mut self) {
        if !self.published {
            let _ = fs::remove_file(&self.path);
        }
    }
}

/// Fills the form at `source` and saves the result to `destination`.
///
/// - `expected` is the fingerprint shown to the user; a form changed since then is refused
///   with "changed" (checked before filling and again right before publishing).
/// - The destination must be a different, absolute path with the same form extension, and
///   if it already exists it must be a plain file (not a folder or link).
/// - The result goes to a new staging file with a random name, created exclusively (never
///   reusing or truncating an existing entry), then renamed over the destination. On any
///   failure only that staging file is removed; the original and an old destination stay.
///   Renaming replaces the destination *entry*, so even a destination that is a hard link to
///   the original leaves the original's contents untouched.
pub fn save_filled(
    source: &Path,
    destination: &Path,
    values: &HashMap<String, String>,
    expected: &str,
) -> Result<(), &'static str> {
    let kind = form_extension(source).ok_or("input")?;
    if !source.is_absolute()
        || !destination.is_absolute()
        || form_extension(destination) != Some(kind)
        || destination.as_os_str().as_encoded_bytes().contains(&0)
    {
        return Err("input");
    }
    let parent = fs::canonicalize(destination.parent().ok_or("input")?).map_err(|_| "io")?;
    let target = parent.join(destination.file_name().ok_or("input")?);
    match fs::symlink_metadata(&target) {
        Ok(meta) if !meta.is_file() || meta.file_type().is_symlink() => return Err("input"),
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => return Err("io"),
        _ => {}
    }
    if fs::canonicalize(source).ok() == Some(target.clone()) {
        // Never overwrite the original form.
        return Err("input");
    }

    let bytes = read_file(source)?;
    if fingerprint(&bytes) != expected {
        return Err("changed");
    }
    let filled = fill(&bytes, values).map_err(error_code)?;
    drop(bytes);

    let mut nonce = [0u8; 16];
    getrandom::fill(&mut nonce).map_err(|_| "io")?;
    let path = parent.join(format!(
        ".prism-form-{:032x}.tmp",
        u128::from_le_bytes(nonce)
    ));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    // Only after this succeeds does the staging file belong to us (and get cleaned up).
    let mut file = options.open(&path).map_err(|_| "io")?;
    let mut stage = Stage {
        path,
        published: false,
    };
    file.write_all(&filled)
        .and_then(|_| file.sync_all())
        .map_err(|_| "io")?;
    drop(file);

    // Last check: the original must still be the form the user filled in.
    if fingerprint(&read_file(source)?) != expected {
        return Err("changed");
    }
    fs::rename(&stage.path, &target).map_err(|_| "io")?;
    stage.published = true;
    Ok(())
}

/// Reads the text parts (validated against the limits) and detects the format.
fn read_parts(package: &[u8]) -> Result<(Format, Vec<(String, String)>), FillError> {
    let mut zip = zip::ZipArchive::new(Cursor::new(package)).map_err(|_| FillError::Unsupported)?;
    if zip.len() > MAX_ENTRIES {
        return Err(FillError::TooLarge);
    }
    let mut total = 0u64;
    let mut names = Vec::with_capacity(zip.len());
    for index in 0..zip.len() {
        let entry = zip.by_index_raw(index).map_err(|_| FillError::Damaged)?;
        total = total.saturating_add(entry.size());
        // zip 9 reports undecodable names as an error instead of guessing.
        let name = entry.name().map_err(|_| FillError::Damaged)?.into_owned();
        names.push(name);
    }
    if total > MAX_TOTAL_BYTES {
        return Err(FillError::TooLarge);
    }
    let format = detect(&names).ok_or(FillError::Unsupported)?;
    let mut parts = Vec::new();
    for name in text_parts(format, &names) {
        let entry = zip.by_name(&name).map_err(|_| FillError::Damaged)?;
        if entry.size() > MAX_PART_BYTES {
            return Err(FillError::TooLarge);
        }
        let mut xml = String::new();
        // Read one byte past the limit so a lying size header is still caught.
        entry
            .take(MAX_PART_BYTES + 1)
            .read_to_string(&mut xml)
            .map_err(|_| FillError::Damaged)?;
        if xml.len() as u64 > MAX_PART_BYTES {
            return Err(FillError::TooLarge);
        }
        parts.push((name, xml));
    }
    Ok((format, parts))
}

/// Copies the package, replacing the changed parts. Unchanged entries are copied raw, which
/// keeps their bytes and compression (HWPX needs its first `mimetype` entry stored as-is).
fn rewrite(package: &[u8], changed: &HashMap<String, String>) -> Result<Vec<u8>, FillError> {
    let mut zip = zip::ZipArchive::new(Cursor::new(package)).map_err(|_| FillError::Damaged)?;
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for index in 0..zip.len() {
        let entry = zip.by_index_raw(index).map_err(|_| FillError::Damaged)?;
        let name = entry.name().map_err(|_| FillError::Damaged)?.into_owned();
        match changed.get(&name) {
            Some(xml) => {
                let options = zip::write::SimpleFileOptions::default()
                    .compression_method(entry.compression());
                drop(entry);
                writer
                    .start_file(name, options)
                    .map_err(|_| FillError::Damaged)?;
                writer
                    .write_all(xml.as_bytes())
                    .map_err(|_| FillError::Damaged)?;
            }
            None => writer
                .raw_copy_file(entry)
                .map_err(|_| FillError::Damaged)?,
        }
    }
    Ok(writer
        .finish()
        .map_err(|_| FillError::Damaged)?
        .into_inner())
}

/// One text node: where its inner text sits in the XML, and that text decoded.
struct TextNode {
    open_start: usize,
    open_end: usize,
    inner_end: usize,
    text: Vec<char>,
    /// Contains markup or is self-closing: never matched or edited.
    opaque: bool,
}

/// Finds the fields in one XML part. For each field `on_field(key, kind, context)` may return
/// a replacement; the part is returned with those replacements applied.
///
/// `limit` caps the returned part's length; growth is counted as replacements are chosen, so
/// the limit is hit before any large string is built.
fn scan_part(
    xml: &str,
    dialect: &Dialect,
    blanks: &mut usize,
    limit: usize,
    deadline: Instant,
    on_field: &mut dyn FnMut(&str, FieldKind, &str) -> Option<String>,
) -> Result<String, FillError> {
    let nodes = text_nodes(xml, dialect.text);
    // Group consecutive text nodes into paragraphs: a paragraph tag between two nodes splits them.
    let mut groups: Vec<Vec<usize>> = Vec::new();
    for (index, node) in nodes.iter().enumerate() {
        let new_group = match index.checked_sub(1).map(|previous| &nodes[previous]) {
            None => true,
            Some(previous) => {
                let gap = &xml[previous.inner_end..node.open_start];
                node.opaque || previous.opaque || has_tag(gap, dialect.paragraph)
            }
        };
        if new_group {
            groups.push(Vec::new());
        }
        groups.last_mut().expect("group exists").push(index);
    }

    // New text for each node that changes.
    let mut replaced: HashMap<usize, String> = HashMap::new();
    // Upper bound of the output length so far: the original plus every encoded value.
    let mut size = xml.len();
    for group in groups {
        check(deadline)?;
        if group.iter().any(|&index| nodes[index].opaque) {
            continue;
        }
        // The paragraph's text, with the node each character came from.
        let mut chars = Vec::new();
        let mut owner = Vec::new();
        for &index in &group {
            for &ch in &nodes[index].text {
                chars.push(ch);
                owner.push(index);
            }
        }
        let context: String = chars
            .iter()
            .collect::<String>()
            .trim()
            .chars()
            .take(80)
            .collect();
        let mut edits: Vec<(usize, usize, String)> = Vec::new();
        for (start, end, key, kind) in find_fields(&chars, blanks) {
            if let Some(value) = on_field(&key, kind, &context) {
                // Encoded size (entities, line-break markup) plus a possible xml:space.
                size = size.saturating_add(encode(&value, dialect.line_break).len() + 32);
                if size > limit {
                    return Err(FillError::TooLarge);
                }
                edits.push((start, end, value));
            }
        }
        if edits.is_empty() {
            continue;
        }
        // Rebuild each node's text: drop matched characters and put each value where its
        // field started.
        let mut texts: HashMap<usize, String> = group.iter().map(|&i| (i, String::new())).collect();
        let mut edit = 0;
        for (position, &ch) in chars.iter().enumerate() {
            while edit < edits.len() && position >= edits[edit].1 {
                edit += 1;
            }
            match edits.get(edit) {
                Some((start, _, value)) if position == *start => {
                    texts
                        .get_mut(&owner[position])
                        .expect("owner")
                        .push_str(value);
                }
                Some((start, end, _)) if position > *start && position < *end => {}
                _ => texts.get_mut(&owner[position]).expect("owner").push(ch),
            }
        }
        replaced.extend(
            texts
                .into_iter()
                .filter(|(index, text)| *text != nodes[*index].text.iter().collect::<String>()),
        );
    }
    if replaced.is_empty() {
        return Ok(xml.to_string());
    }

    let mut out = String::with_capacity(xml.len() + 64);
    let mut cursor = 0;
    for (index, node) in nodes.iter().enumerate() {
        let Some(text) = replaced.get(&index) else {
            continue;
        };
        out.push_str(&xml[cursor..node.open_start]);
        let open = &xml[node.open_start..node.open_end];
        match dialect.preserve_space {
            Some(attribute) if !open.contains("xml:space") => {
                out.push_str(&open[..open.len() - 1]);
                out.push_str(attribute);
                out.push('>');
            }
            _ => out.push_str(open),
        }
        out.push_str(&encode(text, dialect.line_break));
        cursor = node.inner_end;
    }
    out.push_str(&xml[cursor..]);
    Ok(out)
}

/// Placeholders `{{key}}` and underscore blanks in one paragraph: (start, end, key, kind)
/// as character positions, in order, never overlapping.
fn find_fields(chars: &[char], blanks: &mut usize) -> Vec<(usize, usize, String, FieldKind)> {
    let mut found = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '{' && chars.get(i + 1) == Some(&'{') {
            if let Some(close) = (i + 2..chars.len().saturating_sub(1))
                .find(|&j| chars[j] == '}' && chars[j + 1] == '}')
            {
                let key: String = chars[i + 2..close]
                    .iter()
                    .collect::<String>()
                    .trim()
                    .to_string();
                let valid = !key.is_empty()
                    && key.chars().count() <= MAX_KEY_CHARS
                    && !key.contains(['{', '}', '\n']);
                if valid {
                    // User names and positional blanks share one values map.
                    // Escape both reserved prefixes so {{blank:1}}, ___ and
                    // {{field:blank:1}} can receive three independent values.
                    let key = if key.starts_with("blank:") || key.starts_with("field:") {
                        format!("field:{key}")
                    } else {
                        key
                    };
                    found.push((i, close + 2, key, FieldKind::Placeholder));
                    i = close + 2;
                    continue;
                }
            }
        }
        if chars[i] == '_' || chars[i] == '＿' {
            let end = (i..chars.len())
                .find(|&j| chars[j] != chars[i])
                .unwrap_or(chars.len());
            if end - i >= 3 {
                *blanks += 1;
                found.push((i, end, format!("blank:{blanks}"), FieldKind::Blank));
            }
            i = end;
            continue;
        }
        i += 1;
    }
    found
}

/// All `<tag>` / `<tag attr>` text nodes in document order.
fn text_nodes(xml: &str, tag: &str) -> Vec<TextNode> {
    let mut nodes = Vec::new();
    let open_prefix = format!("<{tag}");
    let close = format!("</{tag}>");
    let mut at = 0;
    while let Some(found) = xml[at..].find(&open_prefix) {
        let open_start = at + found;
        let after = open_start + open_prefix.len();
        // `<w:t` must be followed by `>`, `/` or whitespace (not `<w:tab`, `<w:tbl`, ...).
        let next = xml[after..].chars().next();
        if !matches!(next, Some('>' | '/' | ' ' | '\t' | '\r' | '\n')) {
            at = after;
            continue;
        }
        let Some(gt) = xml[after..].find('>') else {
            break;
        };
        let open_end = after + gt + 1;
        if xml[..open_end].ends_with("/>") {
            nodes.push(TextNode {
                open_start,
                open_end,
                inner_end: open_end,
                text: Vec::new(),
                opaque: true,
            });
            at = open_end;
            continue;
        }
        let Some(close_at) = xml[open_end..].find(&close) else {
            break;
        };
        let inner_end = open_end + close_at;
        let inner = &xml[open_end..inner_end];
        let opaque = inner.contains('<');
        nodes.push(TextNode {
            open_start,
            open_end,
            inner_end,
            text: if opaque {
                Vec::new()
            } else {
                decode(inner).chars().collect()
            },
            opaque,
        });
        at = inner_end + close.len();
    }
    nodes
}

/// Whether `<tag ...>` or `</tag>` appears in `text` (exact tag name).
fn has_tag(text: &str, tag: &str) -> bool {
    [format!("<{tag}>"), format!("<{tag} "), format!("</{tag}>")]
        .iter()
        .any(|pattern| text.contains(pattern.as_str()))
}

fn decode(text: &str) -> String {
    if !text.contains('&') {
        return text.to_string();
    }
    quick_xml::escape::unescape(text)
        .map(|value| value.into_owned())
        .unwrap_or_else(|_| text.to_string())
}

fn encode(text: &str, line_break: Option<&str>) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\r' => {}
            '\n' => out.push_str(line_break.unwrap_or(" ")),
            // Control characters are not allowed in XML 1.0 text.
            c if (c as u32) < 0x20 && c != '\t' => {}
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn package(parts: &[(&str, &str, zip::CompressionMethod)]) -> Vec<u8> {
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for (name, text, method) in parts {
            writer
                .start_file(
                    *name,
                    zip::write::SimpleFileOptions::default().compression_method(*method),
                )
                .unwrap();
            writer.write_all(text.as_bytes()).unwrap();
        }
        writer.finish().unwrap().into_inner()
    }
    fn docx(body: &str) -> Vec<u8> {
        let document = format!(
            "<?xml version=\"1.0\"?><w:document xmlns:w=\"x\"><w:body>{body}</w:body></w:document>"
        );
        package(&[
            (
                "[Content_Types].xml",
                "<Types/>",
                zip::CompressionMethod::Deflated,
            ),
            (
                "word/document.xml",
                &document,
                zip::CompressionMethod::Deflated,
            ),
            (
                "word/styles.xml",
                "<w:styles>keep</w:styles>",
                zip::CompressionMethod::Deflated,
            ),
        ])
    }
    fn part(package: &[u8], name: &str) -> String {
        let mut zip = zip::ZipArchive::new(Cursor::new(package)).unwrap();
        let mut text = String::new();
        zip.by_name(name)
            .unwrap()
            .read_to_string(&mut text)
            .unwrap();
        text
    }
    fn values(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    /// A fresh folder under the system temp dir, removed when dropped.
    struct Folder(PathBuf);
    impl Folder {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "prism-form-{name}-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).unwrap();
            Folder(fs::canonicalize(path).unwrap())
        }
        fn entries(&self) -> Vec<String> {
            let mut names: Vec<String> = fs::read_dir(&self.0)
                .unwrap()
                .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
                .collect();
            names.sort();
            names
        }
    }
    impl Drop for Folder {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn simple_form(folder: &Folder) -> (PathBuf, Vec<u8>, String) {
        let source = folder.0.join("form.docx");
        let original = docx("<w:p><w:r><w:t>{{name}}</w:t></w:r></w:p>");
        fs::write(&source, &original).unwrap();
        let info = read_form(&source).unwrap();
        (source, original, info.fingerprint)
    }

    #[test]
    fn saves_a_new_copy_and_keeps_the_original_and_no_staging_files() {
        let folder = Folder::new("save");
        let (source, original, print) = simple_form(&folder);
        let target = folder.0.join("form (filled).docx");
        // An old file at the destination is replaced as a whole.
        fs::write(&target, b"old").unwrap();
        save_filled(&source, &target, &values(&[("name", "Kim")]), &print).unwrap();
        assert!(part(&fs::read(&target).unwrap(), "word/document.xml").contains(">Kim</w:t>"));
        assert_eq!(fs::read(&source).unwrap(), original);
        assert_eq!(folder.entries(), ["form (filled).docx", "form.docx"]);

        // Refused before any work: the original itself, another type, relative paths.
        assert_eq!(
            save_filled(&source, &source, &values(&[]), &print),
            Err("input")
        );
        let other = folder.0.join("x.hwpx");
        assert_eq!(
            save_filled(&source, &other, &values(&[]), &print),
            Err("input")
        );
        let relative = Path::new("x.docx");
        assert_eq!(
            save_filled(&source, relative, &values(&[]), &print),
            Err("input")
        );
        assert_eq!(fs::read(&source).unwrap(), original);
    }

    #[test]
    fn a_form_changed_after_opening_is_not_filled() {
        let folder = Folder::new("changed");
        let (source, _, print) = simple_form(&folder);
        // Someone inserts a blank before the field after Prism listed the fields.
        let edited = docx("<w:p><w:r><w:t>___ {{name}}</w:t></w:r></w:p>");
        fs::write(&source, &edited).unwrap();
        let target = folder.0.join("out.docx");
        fs::write(&target, b"previous output").unwrap();
        assert_eq!(
            save_filled(&source, &target, &values(&[("blank:1", "x")]), &print),
            Err("changed")
        );
        assert_eq!(fs::read(&target).unwrap(), b"previous output");
        assert_eq!(fs::read(&source).unwrap(), edited);
        assert_eq!(folder.entries(), ["form.docx", "out.docx"]);
    }

    #[test]
    fn hard_link_to_the_original_and_failed_publish_keep_both_files() {
        let folder = Folder::new("links");
        let (source, original, print) = simple_form(&folder);
        // The destination is another name for the original file: only the name is replaced.
        let alias = folder.0.join("alias.docx");
        fs::hard_link(&source, &alias).unwrap();
        save_filled(&source, &alias, &values(&[("name", "Lee")]), &print).unwrap();
        assert_eq!(
            fs::read(&source).unwrap(),
            original,
            "original contents untouched"
        );
        assert!(part(&fs::read(&alias).unwrap(), "word/document.xml").contains(">Lee</w:t>"));

        // A folder where the output should go: refused, nothing created or removed.
        let blocked = folder.0.join("blocked.docx");
        fs::create_dir(&blocked).unwrap();
        assert_eq!(
            save_filled(&source, &blocked, &values(&[]), &print),
            Err("input")
        );
        assert!(blocked.is_dir());
        // A file already sitting at an old-style staging name is never touched.
        let sentinel = folder
            .0
            .join(".prism-form-00000000000000000000000000000000.tmp");
        fs::write(&sentinel, b"not ours").unwrap();
        save_filled(&source, &folder.0.join("next.docx"), &values(&[]), &print).unwrap();
        assert_eq!(fs::read(&sentinel).unwrap(), b"not ours");
        assert_eq!(fs::read(&source).unwrap(), original);
        assert!(!folder
            .entries()
            .iter()
            .any(|name| name.ends_with(".tmp") && !name.contains("0000")));
    }

    #[test]
    fn concurrent_saves_to_one_destination_each_publish_a_complete_file() {
        let folder = Folder::new("concurrent");
        let (source, _, print) = simple_form(&folder);
        let target = folder.0.join("out.docx");
        let workers: Vec<_> = ["A", "B", "C", "D"]
            .into_iter()
            .map(|name| {
                let (source, target, print) = (source.clone(), target.clone(), print.clone());
                std::thread::spawn(move || {
                    save_filled(&source, &target, &values(&[("name", name)]), &print)
                })
            })
            .collect();
        let results: Vec<_> = workers.into_iter().map(|w| w.join().unwrap()).collect();
        assert!(results.iter().any(Result::is_ok));
        // Windows may refuse a rename while another one replaces the same file; that save
        // fails cleanly. Whatever won, the destination is one complete, valid package.
        assert!(results.iter().all(|r| r.is_ok() || *r == Err("io")));
        let xml = part(&fs::read(&target).unwrap(), "word/document.xml");
        assert!(["A", "B", "C", "D"]
            .iter()
            .any(|n| xml.contains(&format!(">{n}</w:t>"))));
        assert_eq!(folder.entries(), ["form.docx", "out.docx"]);
    }

    #[test]
    fn repeated_placeholders_and_bad_values_are_refused_before_building_output() {
        // 2,000 copies of one placeholder × a 64 KiB value would be ~128 MiB of XML.
        let body = "<w:p><w:r><w:t>{{v}}</w:t></w:r></w:p>".repeat(2000);
        let form = docx(&body);
        let big = "x".repeat(MAX_VALUE_BYTES);
        assert_eq!(
            fill(&form, &values(&[("v", &big)])),
            Err(FillError::TooLarge)
        );
        // A value over the per-value limit, and control characters XML cannot hold.
        let over = "x".repeat(MAX_VALUE_BYTES + 1);
        assert_eq!(
            fill(&form, &values(&[("v", &over)])),
            Err(FillError::TooLarge)
        );
        assert_eq!(
            fill(&form, &values(&[("v", "a\u{0}b")])),
            Err(FillError::Invalid)
        );
        assert_eq!(
            fill(&form, &values(&[("v", "\u{FFFF}")])),
            Err(FillError::Invalid)
        );
        // Tabs and line breaks are fine.
        assert!(fill(
            &docx("<w:p><w:r><w:t>{{v}}</w:t></w:r></w:p>"),
            &values(&[("v", "a\tb\r\nc")])
        )
        .is_ok());

        // Field discovery is capped too.
        let many = (0..MAX_FIELDS + 1)
            .map(|i| format!("<w:p><w:r><w:t>{{{{f{i}}}}}</w:t></w:r></w:p>"))
            .collect::<String>();
        assert_eq!(fields(&docx(&many)).unwrap_err(), FillError::TooLarge);
    }

    #[test]
    fn placeholder_split_across_runs_keeps_the_first_runs_formatting() {
        let form = docx(
            "<w:p><w:r><w:rPr><w:b/></w:rPr><w:t>Name: {{na</w:t></w:r>\
             <w:r><w:t>me}} (signed)</w:t></w:r></w:p>",
        );
        let (format, found) = fields(&form).unwrap();
        assert_eq!(format, Format::Docx);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].key, "name");
        assert_eq!(found[0].context, "Name: {{name}} (signed)");

        let filled = fill(&form, &values(&[("name", "Kim & <Lee>")])).unwrap();
        let xml = part(&filled, "word/document.xml");
        assert!(xml.contains(
            "<w:rPr><w:b/></w:rPr><w:t xml:space=\"preserve\">Name: Kim &amp; &lt;Lee&gt;</w:t>"
        ));
        assert!(xml.contains("<w:t xml:space=\"preserve\"> (signed)</w:t>"));
        // Other parts are byte-identical.
        assert_eq!(
            part(&filled, "word/styles.xml"),
            "<w:styles>keep</w:styles>"
        );
    }

    #[test]
    fn placeholder_names_do_not_alias_positional_blank_keys() {
        let form = docx(
            "<w:p><w:r><w:t>{{blank:1}} / ___ / {{field:blank:1}} / {{plain}}</w:t></w:r></w:p>",
        );
        let (_, found) = fields(&form).unwrap();
        let keys: Vec<_> = found.iter().map(|field| field.key.as_str()).collect();
        assert_eq!(
            keys,
            ["field:blank:1", "blank:1", "field:field:blank:1", "plain"]
        );
        let filled = fill(
            &form,
            &values(&[
                ("field:blank:1", "named"),
                ("blank:1", "positional"),
                ("field:field:blank:1", "prefixed"),
                ("plain", "ordinary"),
            ]),
        )
        .unwrap();
        assert!(part(&filled, "word/document.xml")
            .contains(">named / positional / prefixed / ordinary</w:t>"));
    }

    #[test]
    fn repeated_placeholders_blanks_entities_and_missing_values() {
        let form = docx(
            "<w:p><w:r><w:t>{{date}} / {{date}}</w:t></w:r></w:p>\
             <w:p><w:r><w:t>Amount: _____ won, R&amp;D: ___</w:t></w:r></w:p>\
             <w:p><w:r><w:t>{{left alone}} and __ (too short)</w:t></w:r></w:p>",
        );
        let (_, found) = fields(&form).unwrap();
        let keys: Vec<_> = found.iter().map(|field| field.key.as_str()).collect();
        assert_eq!(keys, ["date", "blank:1", "blank:2", "left alone"]);
        assert_eq!(found[1].kind, FieldKind::Blank);

        let filled = fill(
            &form,
            &values(&[
                ("date", "2026-11-06"),
                ("blank:1", "1,000"),
                ("blank:2", "yes"),
            ]),
        )
        .unwrap();
        let xml = part(&filled, "word/document.xml");
        assert!(xml.contains(">2026-11-06 / 2026-11-06</w:t>"));
        assert!(xml.contains(">Amount: 1,000 won, R&amp;D: yes</w:t>"));
        assert!(
            xml.contains("<w:t>{{left alone}} and __ (too short)</w:t>"),
            "unchanged"
        );
    }

    #[test]
    fn line_breaks_become_word_breaks_and_tabs_are_not_text_nodes() {
        let form = docx("<w:p><w:r><w:tab/><w:t>{{address}}</w:t></w:r></w:p>");
        let filled = fill(&form, &values(&[("address", "Seoul\nGangnam")])).unwrap();
        let xml = part(&filled, "word/document.xml");
        assert!(xml.contains("<w:tab/>"));
        assert!(xml.contains("Seoul</w:t><w:br/><w:t xml:space=\"preserve\">Gangnam</w:t>"));
    }

    #[test]
    fn paragraphs_never_merge_and_headers_are_filled_too() {
        // "{{a" ends one paragraph and "}}" starts the next: that is not a placeholder.
        let document = "<w:document><w:body><w:p><w:r><w:t>{{a</w:t></w:r></w:p>\
                        <w:p><w:r><w:t>}}</w:t></w:r></w:p></w:body></w:document>";
        let form = package(&[
            (
                "word/document.xml",
                document,
                zip::CompressionMethod::Deflated,
            ),
            (
                "word/header1.xml",
                "<w:hdr><w:p><w:r><w:t>{{company}}</w:t></w:r></w:p></w:hdr>",
                zip::CompressionMethod::Deflated,
            ),
        ]);
        let (_, found) = fields(&form).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].key, "company");
        let filled = fill(&form, &values(&[("company", "Prism")])).unwrap();
        assert!(part(&filled, "word/header1.xml").contains(">Prism</w:t>"));
        assert_eq!(part(&filled, "word/document.xml"), document);
    }

    #[test]
    fn hwpx_sections_are_filled_and_the_stored_mimetype_stays_first() {
        let section = "<hs:sec><hp:p><hp:run><hp:t>성명: ＿＿＿＿</hp:t></hp:run></hp:p>\
                       <hp:p><hp:run><hp:t>{{부서}}</hp:t></hp:run></hp:p></hs:sec>";
        let form = package(&[
            (
                "mimetype",
                "application/hwp+zip",
                zip::CompressionMethod::Stored,
            ),
            (
                "Contents/section0.xml",
                section,
                zip::CompressionMethod::Deflated,
            ),
        ]);
        let (format, found) = fields(&form).unwrap();
        assert_eq!(format, Format::Hwpx);
        assert_eq!(found[0].key, "blank:1");
        assert_eq!(found[1].key, "부서");

        let filled = fill(
            &form,
            &values(&[("blank:1", "홍길동"), ("부서", "기획\n팀")]),
        )
        .unwrap();
        let xml = part(&filled, "Contents/section0.xml");
        assert!(xml.contains("<hp:t>성명: 홍길동</hp:t>"));
        assert!(
            xml.contains("<hp:t>기획 팀</hp:t>"),
            "HWPX gets a space for a newline"
        );
        let mut zip = zip::ZipArchive::new(Cursor::new(&filled)).unwrap();
        let first = zip.by_index(0).unwrap();
        assert_eq!(first.name().unwrap(), "mimetype");
        assert_eq!(first.compression(), zip::CompressionMethod::Stored);
    }

    #[test]
    fn text_with_markup_is_left_alone_and_non_forms_are_refused() {
        let section = "<hp:p><hp:run><hp:t>{{a}}<hp:tab/>{{b}}</hp:t></hp:run></hp:p>";
        let form = package(&[(
            "Contents/section0.xml",
            section,
            zip::CompressionMethod::Deflated,
        )]);
        let filled = fill(&form, &values(&[("a", "1"), ("b", "2")])).unwrap();
        assert_eq!(part(&filled, "Contents/section0.xml"), section);

        let other = package(&[("readme.txt", "{{a}}", zip::CompressionMethod::Deflated)]);
        assert_eq!(fields(&other).unwrap_err(), FillError::Unsupported);
        assert_eq!(fields(b"not a zip").unwrap_err(), FillError::Unsupported);
    }
}
