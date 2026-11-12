//! Link bound document sources without rewriting literal Markdown or link targets.
use pulldown_cmark::{CodeBlockKind, Event, Options, Parser, Tag, TagEnd};
use std::{borrow::Cow, collections::HashSet, ops::Range};

/// A block may validly end with an unfinished fenced code block. When more
/// Markdown is concatenated, its next block and source definitions must stay
/// outside that code. Let the same parser determine whether an unindented
/// continuation would still belong to a fence, including container boundaries.
/// Only append a matching closing fence; leave every original byte intact.
fn close_trailing_fence(markdown: &str) -> Cow<'_, str> {
    let mut probe = String::with_capacity(markdown.len() + 24);
    probe.push_str(markdown);
    probe.push_str("\n\nprism-block-boundary");
    let boundary = markdown.len() + 2;
    let open = Parser::new_ext(&probe, Options::ENABLE_FOOTNOTES)
        .into_offset_iter()
        .find_map(|(event, range)| match event {
            Event::Start(Tag::CodeBlock(CodeBlockKind::Fenced(_)))
                if range.start < markdown.len() && range.end > boundary =>
            {
                Some(range.start)
            }
            _ => None,
        });
    let Some(start) = open else {
        return Cow::Borrowed(markdown);
    };
    let marker = markdown.as_bytes()[start];
    let length = markdown.as_bytes()[start..]
        .iter()
        .take_while(|byte| **byte == marker)
        .count();
    probe.truncate(markdown.len());
    if !probe.ends_with('\n') {
        probe.push('\n');
    }
    probe.extend(std::iter::repeat_n(char::from(marker), length));
    Cow::Owned(probe)
}

pub(super) fn link_sources(markdown: &str, labels: &[&str]) -> String {
    // Even a block with no bound sources can otherwise swallow a later block's
    // references. This function is called only for source-inclusive exports.
    let markdown = close_trailing_fence(markdown);
    if labels.is_empty() {
        return markdown.into_owned();
    }
    let markdown = markdown.as_ref();
    let bound: HashSet<_> = labels.iter().copied().collect();
    let mut visible: Vec<Range<usize>> = Vec::new();
    let mut opaque_depth = 0usize;
    for (event, range) in Parser::new_ext(markdown, Options::ENABLE_FOOTNOTES).into_offset_iter() {
        match event {
            Event::Start(
                Tag::CodeBlock(_)
                | Tag::HtmlBlock
                | Tag::FootnoteDefinition(_)
                | Tag::Link { .. }
                | Tag::Image { .. },
            ) => {
                opaque_depth += 1;
            }
            Event::End(
                TagEnd::CodeBlock
                | TagEnd::HtmlBlock
                | TagEnd::FootnoteDefinition
                | TagEnd::Link
                | TagEnd::Image,
            ) => opaque_depth = opaque_depth.saturating_sub(1),
            Event::Text(_) | Event::FootnoteReference(_) if opaque_depth == 0 => {
                // Unknown shortcut labels can arrive as adjacent text events.
                if let Some(last) = visible.last_mut().filter(|last| last.end == range.start) {
                    last.end = range.end;
                } else {
                    visible.push(range);
                }
            }
            _ => {}
        }
    }
    let mut output = String::with_capacity(markdown.len());
    let mut referenced = HashSet::new();
    let mut copied = 0;
    for range in visible {
        output.push_str(&markdown[copied..range.start]);
        let mut cursor = range.start;
        while let Some(offset) = markdown[cursor..range.end].find('[') {
            let start = cursor + offset;
            output.push_str(&markdown[cursor..start]);
            let escaped = markdown.as_bytes()[..start]
                .iter()
                .rev()
                .take_while(|byte| **byte == b'\\')
                .count()
                % 2
                != 0;
            if !escaped {
                // A bound label is at most 12 bytes including its brackets;
                // an existing footnote reference adds one caret. Bound the
                // scan so a paragraph of unmatched '[' cannot be quadratic.
                if let Some(length) = markdown.as_bytes()[start..range.end]
                    .iter()
                    .take(13)
                    .position(|byte| *byte == b']')
                {
                    let end = start + length;
                    let token = &markdown[start + 1..end];
                    let label = token.strip_prefix('^').unwrap_or(token);
                    if let Some(label) = bound.get(label) {
                        output.push_str(&format!("[^{label}]"));
                        referenced.insert(*label);
                        cursor = end + 1;
                        continue;
                    }
                }
            }
            output.push('[');
            cursor = start + 1;
        }
        output.push_str(&markdown[cursor..range.end]);
        copied = range.end;
    }
    output.push_str(&markdown[copied..]);
    let missing: Vec<_> = labels
        .iter()
        .filter(|label| referenced.insert(**label))
        .map(|label| format!("[^{label}]"))
        .collect();
    if !missing.is_empty() {
        output.push_str("\n\n");
        output.push_str(&missing.join(" "));
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn complete_fences_and_container_terminated_fences_need_no_added_markup() {
        for markdown in [
            "Plain 한글 `inline`",
            "```text\ncode\n```",
            "~~~text\ncode\n~~~~   ",
            "> ```text\n> code",
            "- ```text\n  code",
            "    indented code",
        ] {
            assert_eq!(link_sources(markdown, &[]), markdown);
        }
    }

    #[test]
    fn unfinished_fences_keep_the_original_bytes_and_close_with_the_opening_marker() {
        for (markdown, closing) in [
            ("```", "\n```"),
            ("```text\n한글", "\n```"),
            ("  ~~~~~text\r\ncode\r\n~~~", "\n~~~~~"),
            ("   ````text\ncode\n", "````"),
            // The pinned renderer parser accepts closing-fence spaces, but
            // not a trailing tab. Its actual block boundary is authoritative.
            ("```text\ncode\n```\t", "\n```"),
        ] {
            assert_eq!(link_sources(markdown, &[]), format!("{markdown}{closing}"));
        }
    }

    #[test]
    fn literal_lookalikes_do_not_hide_or_duplicate_a_visible_source_reference() {
        let markdown = r"한글 `[S1]` and \[S1], then **[S1]** and [E9].";
        assert_eq!(
            link_sources(markdown, &["S1", "E9", "S1"]),
            r"한글 `[S1]` and \[S1], then **[^S1]** and [^E9]."
        );
    }

    #[test]
    fn dense_unmatched_brackets_and_unicode_leave_only_known_sources_linked() {
        let prefix = "[".repeat(64 * 1024);
        let markdown = format!("{prefix}\n한글 [E12], literal `[^S2]`, unknown [S999].");
        assert_eq!(
            link_sources(&markdown, &["E12", "S2"]),
            format!("{prefix}\n한글 [^E12], literal `[^S2]`, unknown [S999].\n\n[^S2]")
        );
    }
}
