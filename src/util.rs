//! Small shared helpers used across INDEX, RESOLVE, RENDER and the site build:
//! flattening inline objects to plain text, slugifying heading text into anchors,
//! and computing relative output URLs between pages.

use camino::{Utf8Path, Utf8PathBuf};

use crate::model::{Element, Keywords, Object, Section, TableRow};

/// The output path for a document, relative to the site root.
///
/// Normally this is the source path with `.org` swapped for `.html`, but a `#+SLUG:`
/// keyword renames the file — which is how the target corpus works: 178 of its 179 files
/// set one, and `2018-11-28-aes-encryption.org` publishes as `aes-encryption.html`. The
/// slug names the *file*, never the directory, so the page stays where its source lives.
pub fn output_path(source: &Utf8Path, keywords: &Keywords) -> Utf8PathBuf {
    let slug = keywords
        .entries
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("SLUG"))
        .map(|(_, v)| sanitize_slug(v))
        .filter(|s| !s.is_empty());

    match slug {
        Some(slug) => {
            let dir = source.parent().unwrap_or_else(|| Utf8Path::new(""));
            dir.join(format!("{slug}.html"))
        }
        None => source.with_extension("html"),
    }
}

/// Reduce a slug to a safe single filename component.
///
/// A slug is author-controlled text that becomes a path we write to, so `../../etc/x`
/// has to be impossible by construction rather than by convention: separators and dots
/// are folded to `-`, which cannot traverse and cannot produce a hidden file.
fn sanitize_slug(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut prev_dash = false;
    for c in raw.trim().chars() {
        if c.is_ascii_alphanumeric() || c == '_' {
            out.extend(c.to_lowercase());
            prev_dash = false;
        } else if !prev_dash {
            out.push('-');
            prev_dash = true;
        }
    }
    out.trim_matches('-').to_string()
}

/// Flatten inline objects to their plain-text content (markup stripped). Used to
/// derive heading anchors and `[[*Heading]]` link identities (spec §4.3).
pub fn plain_text(objs: &[Object]) -> String {
    let mut out = String::new();
    plain_text_into(objs, &mut out);
    out
}

fn plain_text_into(objs: &[Object], out: &mut String) {
    for obj in objs {
        match obj {
            Object::Text(t) => out.push_str(t),
            Object::Bold(i)
            | Object::Italic(i)
            | Object::Underline(i)
            | Object::StrikeThrough(i) => plain_text_into(i, out),
            Object::Verbatim(s) | Object::Code(s) | Object::Entity(s) => out.push_str(s),
            Object::Link(l) => {
                if let Some(desc) = &l.description {
                    plain_text_into(desc, out);
                }
            }
            Object::FootnoteRef { .. } | Object::Timestamp(_) | Object::LineBreak => {}
        }
    }
}

/// Is this document marked as a draft?
///
/// `#+DRAFT:` counts as true by its mere presence — writing the keyword at all is the
/// signal — unless the value explicitly says otherwise. Someone who types `#+DRAFT: t`,
/// `#+DRAFT: yes` or a bare `#+DRAFT:` means the same thing, and publishing an unfinished
/// post because the value was not the expected spelling is the wrong way to be strict.
pub fn is_draft(keywords: &Keywords) -> bool {
    keywords
        .entries
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("DRAFT"))
        .map(|(_, v)| {
            !matches!(
                v.trim().to_ascii_lowercase().as_str(),
                "nil" | "false" | "no" | "0" | "off"
            )
        })
        .unwrap_or(false)
}

/// The document's prose as plain text, for word counts and excerpts.
///
/// Source and example blocks are excluded on purpose: a reading-time estimate over a
/// post that is mostly a shell transcript should describe the prose someone reads, not
/// the code they skim. Headings are included — they are read.
pub fn document_text(root: &Section) -> String {
    let mut out = String::new();
    section_text(root, &mut out);
    out
}

fn section_text(section: &Section, out: &mut String) {
    if let Some(heading) = &section.heading {
        push_words(&plain_text(&heading.title), out);
    }
    elements_text(&section.content, out);
    for child in &section.children {
        section_text(child, out);
    }
}

fn elements_text(elements: &[Element], out: &mut String) {
    for element in elements {
        match element {
            Element::Paragraph(objs) => push_words(&plain_text(objs), out),
            Element::List(list) => {
                for item in &list.items {
                    if let Some(term) = &item.term {
                        push_words(&plain_text(term), out);
                    }
                    elements_text(&item.content, out);
                }
            }
            Element::Table(table) => {
                for row in &table.rows {
                    if let TableRow::Cells(cells) = row {
                        for cell in cells {
                            push_words(&plain_text(cell), out);
                        }
                    }
                }
            }
            Element::QuoteBlock(inner) | Element::CenterBlock(inner) => elements_text(inner, out),
            Element::Figure { caption, .. } => push_words(&plain_text(caption), out),
            Element::FootnoteDefinition { content, .. } => elements_text(content, out),
            // Code, drawers, comments, keywords and raw export blocks are not prose.
            _ => {}
        }
    }
}

fn push_words(text: &str, out: &mut String) {
    let text = text.trim();
    if text.is_empty() {
        return;
    }
    if !out.is_empty() {
        out.push(' ');
    }
    out.push_str(text);
}

/// The document's first paragraph as plain text — the fallback excerpt for a page with
/// no `#+DESCRIPTION:`.
pub fn first_paragraph(root: &Section) -> Option<String> {
    fn find(section: &Section) -> Option<String> {
        for element in &section.content {
            if let Element::Paragraph(objs) = element {
                let text = plain_text(objs);
                if !text.trim().is_empty() {
                    return Some(text.trim().to_string());
                }
            }
        }
        section.children.iter().find_map(find)
    }
    find(root)
}

/// Turn heading text into a URL-safe anchor slug.
pub fn slugify(text: &str) -> String {
    let mut out = String::new();
    let mut prev_dash = false;
    for c in text.chars() {
        if c.is_alphanumeric() {
            out.extend(c.to_lowercase());
            prev_dash = false;
        } else if !prev_dash {
            out.push('-');
            prev_dash = true;
        }
    }
    out.trim_matches('-').to_string()
}

/// The URL to reach the page output at `to_out` from the page output at `from_out`,
/// honoring an optional `anchor`. Same-page links reduce to a bare `#anchor` fragment;
/// cross-page links become a relative path.
///
/// Both arguments are *output* paths, not source paths, because `#+SLUG:` means the two
/// no longer correspond: deriving the URL here would reintroduce the filename assumption
/// that [`output_path`] exists to remove.
pub fn output_url(from_out: &Utf8Path, to_out: &Utf8Path, anchor: Option<&str>) -> String {
    let path = if from_out == to_out {
        String::new()
    } else {
        let from_dir = from_out.parent().unwrap_or_else(|| Utf8Path::new(""));
        relative_path(from_dir, to_out)
    };
    match anchor {
        Some(a) if !a.is_empty() => {
            if path.is_empty() {
                format!("#{a}")
            } else {
                format!("{path}#{a}")
            }
        }
        _ => {
            if path.is_empty() {
                "#".to_string()
            } else {
                path
            }
        }
    }
}

/// The `../`-prefix that reaches the site root from the page at `from_rel`. Empty for a
/// top-level page. Used for site-global assets like the syntax stylesheet.
pub fn relative_root(from_rel: &Utf8Path) -> String {
    let depth = from_rel
        .parent()
        .map(|p| p.components().count())
        .unwrap_or(0);
    "../".repeat(depth)
}

/// Relative path from `from_dir` to `to`, using `../` where needed. `/`-joined for URLs.
fn relative_path(from_dir: &Utf8Path, to: &Utf8Path) -> String {
    let from_c: Vec<&str> = from_dir.components().map(|c| c.as_str()).collect();
    let to_c: Vec<&str> = to.components().map(|c| c.as_str()).collect();
    let mut i = 0;
    while i < from_c.len() && i < to_c.len() && from_c[i] == to_c[i] {
        i += 1;
    }
    let mut parts: Vec<&str> = vec![".."; from_c.len() - i];
    parts.extend(&to_c[i..]);
    parts.join("/")
}

/// Resolve a `file:` link path (as written) against the linking page's directory,
/// normalizing `.`/`..` so it can be matched against indexed file targets.
pub fn normalize_link_path(from_rel: &Utf8Path, path: &Utf8Path) -> Utf8PathBuf {
    let base = from_rel.parent().unwrap_or_else(|| Utf8Path::new(""));
    let joined = base.join(path);
    let mut stack: Vec<&str> = Vec::new();
    for comp in joined.components() {
        match comp.as_str() {
            "." => {}
            ".." => {
                stack.pop();
            }
            other => stack.push(other),
        }
    }
    Utf8PathBuf::from(stack.join("/"))
}

/// The `YYYY-MM-DD` inside an org date, if there is one. Org dates arrive as
/// `[2025-09-05 Fri 10:21:00]`, `<2024-05-01 Wed>` or bare `2024-05-01`, and a listing
/// needs one key it can sort on.
pub fn iso_date(raw: &str) -> Option<String> {
    let bytes = raw.as_bytes();
    for i in 0..bytes.len().saturating_sub(9) {
        let window = &bytes[i..i + 10];
        let digits = |r: std::ops::Range<usize>| window[r].iter().all(u8::is_ascii_digit);
        if digits(0..4) && window[4] == b'-' && digits(5..7) && window[7] == b'-' && digits(8..10) {
            // Must not be part of a longer number, or `123-45-6789` would parse.
            let before_ok = i == 0 || !bytes[i - 1].is_ascii_digit();
            let after_ok = i + 10 >= bytes.len() || !bytes[i + 10].is_ascii_digit();
            if before_ok && after_ok {
                return Some(raw[i..i + 10].to_string());
            }
        }
    }
    None
}
