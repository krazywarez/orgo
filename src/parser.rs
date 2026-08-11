//! PARSE stage (spec §2.1, §3.1): bytes → tokens → org element tree.
//!
//! Hand-written recursive descent, deliberately two-tier (spec §3.1):
//! 1. [`line_lexer`] — cheap first pass classifying each line, context-free.
//! 2. [`build_document`] — recursive descent over the line stream into `Section`s/`Element`s.
//! 3. [`inline`] — scans an element's text runs into `Vec<Object>`, implementing
//!    org's emphasis pre/post-char rules explicitly.
//!
//! PARSE is a pure function of a single file's bytes (spec §2.1): it never depends on
//! another file, which is what makes content-hash caching sound.
//!
//! Scope is the v1 IN list (README §"v1 scope"): headings with nesting, TODO keywords,
//! priorities, tags and property drawers; paragraphs; plain lists (unordered, ordered,
//! description) with checkboxes and nesting; tables; source/example/quote/center/export
//! blocks; footnotes; `#+` keywords; inline markup, links, timestamps; images with
//! `#+CAPTION`/`#+ATTR_HTML`.
//!
//! Out-of-scope constructs are parsed-and-ignored, never fatal: babel `:results` and
//! `#+TBLFM:` are inert keywords, unknown block types keep their content verbatim as
//! example blocks, generic drawers are captured and dropped at render, and LaTeX,
//! macros and radio targets survive as literal text.

use camino::Utf8Path;
use chrono::{NaiveDate, NaiveDateTime, NaiveTime};

use crate::model::{
    BlockParams, Bullet, Checkbox, ContentHash, Document, Element, Heading, Keywords, Link,
    Diagnostic, LinkTarget, List, ListItem, ListKind, Object, Properties, Section, Table, TableRow,
    Timestamp, TodoKeyword,
};

#[derive(Debug, thiserror::Error)]
pub enum ParseError {
    #[error("parse error at line {line}: {message}")]
    At { line: usize, message: String },
}

/// Classified lines produced by the first pass (spec §3.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Line {
    Heading,
    BlockBegin { kind: String },
    BlockEnd,
    ListItem,
    TableRow,
    Keyword,
    DrawerBegin,
    DrawerEnd,
    Rule,
    Blank,
    Text,
}

/// First pass: classify each raw line. Context-free per line.
pub fn line_lexer(source: &str) -> Vec<Line> {
    source.lines().map(classify_line).collect()
}

fn classify_line(line: &str) -> Line {
    if line.trim().is_empty() {
        return Line::Blank;
    }
    if heading_level(line).is_some() {
        return Line::Heading;
    }
    let t = line.trim_start();
    let upper = t.to_ascii_uppercase();
    if let Some(rest) = upper.strip_prefix("#+BEGIN_") {
        let kind = rest.split_whitespace().next().unwrap_or("").to_string();
        return Line::BlockBegin { kind };
    }
    if upper.starts_with("#+END_") {
        return Line::BlockEnd;
    }
    if keyword_kv(line).is_some() {
        return Line::Keyword;
    }
    if is_rule(line) {
        return Line::Rule;
    }
    if t.eq_ignore_ascii_case(":END:") {
        return Line::DrawerEnd;
    }
    if is_drawer_begin(t) {
        return Line::DrawerBegin;
    }
    if is_list_item(t).is_some() {
        return Line::ListItem;
    }
    if t.starts_with('|') {
        return Line::TableRow;
    }
    Line::Text
}

/// blake3 of raw source bytes — the content hash that drives re-parse decisions (spec §4.1).
pub fn content_hash(bytes: &[u8]) -> ContentHash {
    ContentHash(*blake3::hash(bytes).as_bytes())
}

/// Parse one source file into a [`Document`]. Pure over `(path, source)`.
pub fn parse(path: &Utf8Path, source: &str) -> Result<Document, ParseError> {
    let content_hash = content_hash(source.as_bytes());
    let lines: Vec<&str> = source.lines().collect();
    let classes = line_lexer(source);

    let mut diagnostics: Vec<Diagnostic> = Vec::new();
    let mut keywords = Keywords::default();
    let mut root = Section {
        heading: None,
        content: Vec::new(),
        children: Vec::new(),
    };

    let heading_idxs: Vec<usize> = classes
        .iter()
        .enumerate()
        .filter(|(_, c)| **c == Line::Heading)
        .map(|(i, _)| i)
        .collect();
    let first = heading_idxs.first().copied().unwrap_or(lines.len());

    // Preamble: document-level keywords are *copied* into `keywords`, which is the
    // metadata map. They are not removed from the body — collecting is not deleting.
    // Dropping the lines would merge the paragraphs either side of a keyword and would
    // strand affiliated keywords (`#+CAPTION:`) away from the element they belong to;
    // left in place, `parse_elements` handles both. Affiliated keywords are not document
    // metadata, so they are not copied.
    {
        for (l, c) in lines[..first].iter().zip(&classes[..first]) {
            if *c == Line::Keyword {
                if let Some((k, v)) = keyword_kv(l) {
                    if !is_affiliated(&k) {
                        keywords.entries.push((k, v));
                    }
                }
            }
        }
        root.content = parse_elements(&lines[..first], 0, &mut diagnostics);
    }

    // Each heading segment runs from its own line up to (but excluding) the next heading.
    let mut flat: Vec<(u8, Section)> = Vec::new();
    for (k, &h_idx) in heading_idxs.iter().enumerate() {
        let end = heading_idxs.get(k + 1).copied().unwrap_or(lines.len());
        let heading = parse_heading(lines[h_idx]);
        let level = heading.level;
        let (heading, content) =
            parse_section_body(heading, &lines[h_idx + 1..end], h_idx + 1, &mut diagnostics);
        flat.push((
            level,
            Section {
                heading: Some(heading),
                content,
                children: Vec::new(),
            },
        ));
    }

    let mut pos = 0;
    root.children = build_children(&mut flat, &mut pos, 0);

    diagnostics.sort_by_key(|d| d.line);
    Ok(Document {
        source_path: path.to_owned(),
        content_hash,
        keywords,
        root,
        diagnostics,
    })
}

/// Fold the flat `(level, section)` list into org's nested hierarchy by level.
fn build_children(flat: &mut [(u8, Section)], pos: &mut usize, parent_level: u8) -> Vec<Section> {
    let mut children = Vec::new();
    while *pos < flat.len() {
        let level = flat[*pos].0;
        if level <= parent_level {
            break;
        }
        let mut section = std::mem::replace(&mut flat[*pos].1, empty_section());
        *pos += 1;
        section.children = build_children(flat, pos, level);
        children.push(section);
    }
    children
}

fn empty_section() -> Section {
    Section {
        heading: None,
        content: Vec::new(),
        children: Vec::new(),
    }
}

/// Second-tier: scan an element's text into inline objects, applying org's
/// pre/post-char emphasis rules (spec §3.1, R3 — the highest-divergence area).
pub fn inline(text: &str) -> Vec<Object> {
    let chars: Vec<char> = text.chars().collect();
    parse_inline_run(&chars)
}

// ---------------------------------------------------------------------------
// Headings
// ---------------------------------------------------------------------------

/// `*`-prefixed heading depth, or `None` if the line is not a heading.
fn heading_level(line: &str) -> Option<u8> {
    if !line.starts_with('*') {
        return None;
    }
    let stars = line.chars().take_while(|c| *c == '*').count();
    let after = &line[stars..];
    if after.starts_with(' ') || after.is_empty() {
        Some(stars.min(u8::MAX as usize) as u8)
    } else {
        None
    }
}

/// The default TODO keyword set, matching Emacs' out-of-the-box `org-todo-keywords`
/// (`("TODO" "DONE")`) so our output can be diffed against an `emacs --batch` oracle.
/// Per-file `#+TODO:` sequences are out of scope; the set is a documented [`BuildConfig`]
/// slot for when it becomes configurable.
///
/// [`BuildConfig`]: crate::incremental::BuildConfig
const TODO_KEYWORDS: &[(&str, bool)] = &[("TODO", false), ("DONE", true)];

fn parse_heading(line: &str) -> Heading {
    let level = heading_level(line).unwrap_or(1);
    let rest = line[level as usize..].trim();
    let (title_str, tags) = split_tags(rest);
    let (todo, after_todo) = split_todo(title_str.trim());
    let (priority, title_str) = split_priority(after_todo);
    Heading {
        level,
        todo,
        priority,
        title: inline(title_str.trim()),
        tags,
        properties: Properties::default(),
        id: None,
        custom_id: None,
    }
}

/// A leading TODO keyword: a bare word from the keyword set, followed by whitespace or
/// end of the heading. `*  TODOs are great` is NOT a keyword (no word boundary).
fn split_todo(title: &str) -> (Option<TodoKeyword>, &str) {
    let word_end = title.find(char::is_whitespace).unwrap_or(title.len());
    let word = &title[..word_end];
    for (name, done) in TODO_KEYWORDS {
        if word == *name {
            return (
                Some(TodoKeyword {
                    name: (*name).to_string(),
                    done: *done,
                }),
                title[word_end..].trim_start(),
            );
        }
    }
    (None, title)
}

/// A priority cookie `[#A]` immediately after the TODO keyword.
fn split_priority(title: &str) -> (Option<char>, &str) {
    let Some(rest) = title.strip_prefix("[#") else {
        return (None, title);
    };
    let mut chars = rest.chars();
    let Some(c) = chars.next().filter(|c| c.is_ascii_alphanumeric()) else {
        return (None, title);
    };
    match chars.next() {
        Some(']') => (
            Some(c.to_ascii_uppercase()),
            rest[c.len_utf8() + 1..].trim_start(),
        ),
        _ => (None, title),
    }
}

/// Split a trailing `:tag1:tag2:` cluster off the heading text.
fn split_tags(rest: &str) -> (&str, Vec<String>) {
    let trimmed = rest.trim_end();
    if !trimmed.ends_with(':') {
        return (rest, Vec::new());
    }
    let start = match trimmed.rfind(char::is_whitespace) {
        Some(i) => i + 1,
        None => 0,
    };
    let candidate = &trimmed[start..];
    if is_tag_cluster(candidate) {
        let tags = candidate
            .split(':')
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .collect();
        (&trimmed[..start], tags)
    } else {
        (rest, Vec::new())
    }
}

/// A `:a:b:c:` cluster: colon-delimited, non-empty tag names, colon-bounded.
fn is_tag_cluster(s: &str) -> bool {
    if !s.starts_with(':') || !s.ends_with(':') || s.len() < 3 {
        return false;
    }
    let inner = &s[1..s.len() - 1];
    !inner.is_empty()
        && inner.split(':').all(|part| {
            !part.is_empty()
                && part
                    .chars()
                    .all(|c| c.is_alphanumeric() || matches!(c, '_' | '@' | '#' | '%'))
        })
}

// ---------------------------------------------------------------------------
// Section body: property drawer + block content
// ---------------------------------------------------------------------------

fn parse_section_body(
    mut heading: Heading,
    body: &[&str],
    base: usize,
    diags: &mut Vec<Diagnostic>,
) -> (Heading, Vec<Element>) {
    let mut idx = 0;
    while idx < body.len() && body[idx].trim().is_empty() {
        idx += 1;
    }
    if idx < body.len() && body[idx].trim().eq_ignore_ascii_case(":PROPERTIES:") {
        let opened_at = base + idx;
        let mut terminated = false;
        idx += 1;
        while idx < body.len() {
            let t = body[idx].trim();
            if t.eq_ignore_ascii_case(":END:") {
                idx += 1;
                terminated = true;
                break;
            }
            if let Some((k, v)) = parse_property(t) {
                if k.eq_ignore_ascii_case("CUSTOM_ID") {
                    heading.custom_id = Some(v.clone());
                } else if k.eq_ignore_ascii_case("ID") {
                    heading.id = Some(v.clone());
                }
                heading.properties.entries.push((k, v));
            }
            idx += 1;
        }
        if !terminated {
            diags.push(Diagnostic {
                line: opened_at + 1,
                message: "unterminated :PROPERTIES: drawer (no :END:); the rest of the \
                          section was read as properties"
                    .to_string(),
            });
        }
    }
    let content = parse_elements(&body[idx..], base + idx, diags);
    (heading, content)
}

/// `:KEY: value` inside a drawer.
fn parse_property(line: &str) -> Option<(String, String)> {
    let line = line.trim();
    let line = line.strip_prefix(':')?;
    let end = line.find(':')?;
    let key = line[..end].trim().to_string();
    if key.is_empty() {
        return None;
    }
    let value = line[end + 1..].trim().to_string();
    Some((key, value))
}

// ---------------------------------------------------------------------------
// Block-level element builder
// ---------------------------------------------------------------------------

/// Build the block elements of `lines`. `base` is the absolute 0-based index of
/// `lines[0]` in the source file, so diagnostics can name a real line number however
/// deeply nested the construct is.
fn parse_elements(lines: &[&str], base: usize, diags: &mut Vec<Diagnostic>) -> Vec<Element> {
    let mut out = Vec::new();
    // Affiliated keywords (`#+CAPTION:` and friends) belong to the element that follows
    // them, so they are held aside until that element is built.
    let mut affiliated: Vec<(String, String)> = Vec::new();
    let mut drop_next = false;
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        if line.trim().is_empty() {
            // A blank line ends the association: an affiliated keyword belongs to the
            // element *immediately* below it. Someone who writes `#+CAPTION:` under their
            // image and then leaves a blank line has captioned nothing, and org agrees —
            // silently attaching it to whatever comes next would caption the wrong thing.
            affiliated.clear();
            i += 1;
            continue;
        }
        if let Some((key, value)) = keyword_kv(line) {
            if key.eq_ignore_ascii_case("RESULTS") {
                // Babel is never executed (README §OUT), so a checked-in `#+RESULTS:`
                // block is output from someone else's Emacs session at some other time.
                // Emitting it would put unverifiable content on the page dressed as
                // real content, so the block it labels is dropped.
                drop_next = true;
            } else if is_affiliated(&key) {
                affiliated.push((key, value));
            } else {
                out.push(Element::Keyword { key, value });
            }
            i += 1;
            continue;
        }
        let (element, next) = parse_one_element(lines, i, base, diags);
        i = next;
        if std::mem::take(&mut drop_next) {
            affiliated.clear();
            continue;
        }
        if let Some(element) = element {
            out.push(attach_affiliated(element, std::mem::take(&mut affiliated)));
        }
    }
    out
}

/// Build the single element starting at `lines[start]`, returning it with the index of
/// the first line past it. `None` means the lines were consumed without producing an
/// element. `start` is guaranteed non-blank and not an affiliated keyword.
fn parse_one_element(
    lines: &[&str],
    start: usize,
    base: usize,
    diags: &mut Vec<Diagnostic>,
) -> (Option<Element>, usize) {
    let line = lines[start];
    if let Some(text) = comment_text(line) {
        return (Some(Element::Comment(text)), start + 1);
    }
    if let Some((kind, after)) = block_begin(line) {
        let (el, next) = parse_block(lines, start, &kind, &after, base, diags);
        return (Some(el), next);
    }
    if let Some(name) = drawer_begin_name(line) {
        let (el, next) = parse_drawer(lines, start, name, base, diags);
        return (Some(el), next);
    }
    if is_rule(line) {
        return (Some(Element::HorizontalRule), start + 1);
    }
    if line.trim_start().starts_with('|') {
        let (table, next) = parse_table(lines, start);
        return (Some(Element::Table(table)), next);
    }
    if let Some((label, first_rest)) = footnote_def_label(line) {
        let (def, next) = parse_footnote_def(lines, start, label, first_rest);
        return (Some(def), next);
    }
    if is_list_item(line.trim_start()).is_some() {
        let (list, next) = parse_list(lines, start, base, diags);
        return (Some(Element::List(list)), next);
    }
    // Paragraph: gather consecutive soft-wrapped text lines.
    let mut para = Vec::new();
    let mut i = start;
    while i < lines.len() {
        let l = lines[i];
        if l.trim().is_empty() || is_structural(l) {
            break;
        }
        para.push(l.trim());
        i += 1;
    }
    if para.is_empty() {
        // `is_structural` said this line begins a construct that no branch above claimed.
        // In practice that is a stray `#+END_`: a block terminator with nothing open.
        // Skip it rather than looping forever, but say so — it usually means a `#+BEGIN_`
        // above it is misspelled, and silence would leave the author hunting.
        if is_block_end(line) {
            diags.push(Diagnostic {
                line: base + start + 1,
                message: format!(
                    "stray `{}` with no matching `#+BEGIN_`",
                    line.split_whitespace().next().unwrap_or("#+END_")
                ),
            });
        }
        return (None, start + 1);
    }
    (Some(Element::Paragraph(inline(&para.join(" ")))), i)
}

/// Is this line the start of a non-paragraph construct?
fn is_structural(line: &str) -> bool {
    let t = line.trim_start();
    block_begin(line).is_some()
        || is_block_end(line)
        || is_rule(line)
        || keyword_kv(line).is_some()
        || comment_text(line).is_some()
        || drawer_begin_name(line).is_some()
        || is_list_item(t).is_some()
        || t.starts_with('|')
        || footnote_def_label(line).is_some()
        || heading_level(line).is_some()
}

// ---------------------------------------------------------------------------
// Blocks, drawers, comments, affiliated keywords
// ---------------------------------------------------------------------------

/// Consume `#+BEGIN_<KIND> … #+END_<KIND>`. Matching is on the *specific* kind so a
/// source block can sit inside a quote block; an unterminated block runs to end of
/// input rather than failing.
fn parse_block(
    lines: &[&str],
    start: usize,
    kind: &str,
    after: &str,
    base: usize,
    diags: &mut Vec<Diagnostic>,
) -> (Element, usize) {
    let mut inner: Vec<String> = Vec::new();
    let mut j = start + 1;
    while j < lines.len() && !is_block_end_of(lines[j], kind) {
        inner.push(unescape_block_line(lines[j]));
        j += 1;
    }
    let inner: Vec<&str> = inner.iter().map(String::as_str).collect();
    if j >= lines.len() {
        // Everything to the end of input was swallowed by the block. This is the single
        // most destructive malformation in org: one missing line silently deletes the
        // rest of the document from the output.
        diags.push(Diagnostic {
            line: base + start + 1,
            message: format!(
                "unterminated `#+BEGIN_{}` block (no `#+END_{}`); \
                 everything to the end of the file was read as block content",
                kind.to_ascii_uppercase(),
                kind.to_ascii_uppercase()
            ),
        });
    }
    let next = if j < lines.len() { j + 1 } else { j };
    let element = match kind.to_ascii_uppercase().as_str() {
        "SRC" => {
            let (lang, params) = parse_src_header(after);
            Element::SrcBlock {
                lang,
                params,
                code: inner.join("\n"),
            }
        }
        "EXAMPLE" => Element::ExampleBlock(inner.join("\n")),
        "QUOTE" => Element::QuoteBlock(parse_elements(&inner, base + start + 1, diags)),
        "CENTER" => Element::CenterBlock(parse_elements(&inner, base + start + 1, diags)),
        "EXPORT" => Element::ExportBlock {
            backend: after.split_whitespace().next().unwrap_or("").to_string(),
            raw: inner.join("\n"),
        },
        // Verse keeps its line breaks; that is the whole point of it.
        "VERSE" => Element::VerseBlock(inner.iter().map(|l| l.to_string()).collect()),
        // A comment block is not published, in org or here.
        "COMMENT" => Element::Comment(inner.join("\n")),
        // Any other name is a special block: a div with that class, holding org. Emacs
        // exports unknown block types this way, which is what makes `#+BEGIN_NOTE` a
        // usable convention without the exporter knowing the word "note".
        other => Element::SpecialBlock {
            name: other.to_ascii_lowercase(),
            content: parse_elements(&inner, base + start + 1, diags),
        },
    };
    (element, next)
}

/// Undo org's comma escape on one line of block content.
///
/// A line inside a block that would otherwise look like document structure is written
/// with a leading comma — `,* heading`, `,#+KEYWORD:` — and the exporter removes exactly
/// one comma. Without this, documentation *about* org shows the escape characters its
/// author had to type, which is precisely the audience most likely to notice.
fn unescape_block_line(line: &str) -> String {
    let trimmed = line.trim_start();
    let Some(rest) = trimmed.strip_prefix(',') else {
        return line.to_string();
    };
    if !(rest.starts_with('*') || rest.starts_with("#+") || rest.starts_with(',')) {
        return line.to_string();
    }
    let indent = &line[..line.len() - trimmed.len()];
    format!("{indent}{rest}")
}

/// `:NAME:` … `:END:` at block level. A PROPERTIES drawer directly under a heading is
/// consumed by [`parse_section_body`]; anything reaching here is a generic drawer,
/// which the renderer drops (README §OUT).
fn parse_drawer(
    lines: &[&str],
    start: usize,
    name: String,
    base: usize,
    diags: &mut Vec<Diagnostic>,
) -> (Element, usize) {
    let mut inner: Vec<&str> = Vec::new();
    let mut j = start + 1;
    while j < lines.len() && !lines[j].trim().eq_ignore_ascii_case(":END:") {
        inner.push(lines[j]);
        j += 1;
    }
    if j >= lines.len() {
        // Drawers render to nothing, so an unterminated one deletes the rest of the file
        // from the output just as thoroughly as an unterminated block — and more quietly.
        diags.push(Diagnostic {
            line: base + start + 1,
            message: format!(
                "unterminated `:{name}:` drawer (no `:END:`); everything to the end of \
                 the file was read as drawer content and will not be rendered"
            ),
        });
    }
    let next = if j < lines.len() { j + 1 } else { j };
    (
        Element::Drawer {
            name,
            content: parse_elements(&inner, base + start + 1, diags),
        },
        next,
    )
}

/// The drawer name in a `:NAME:` opening line, if this line is one. `:END:` closes a
/// drawer rather than opening one.
fn drawer_begin_name(line: &str) -> Option<String> {
    let t = line.trim();
    if !is_drawer_begin(t) {
        return None;
    }
    let name = &t[1..t.len() - 1];
    if name.eq_ignore_ascii_case("END") {
        return None;
    }
    Some(name.to_string())
}

/// A comment line: `#` followed by whitespace or nothing. `#+KEY:` is a keyword (checked
/// first) and `#hashtag` is ordinary text.
fn comment_text(line: &str) -> Option<String> {
    let rest = line.trim_start().strip_prefix('#')?;
    if rest.is_empty() {
        return Some(String::new());
    }
    if !rest.starts_with(char::is_whitespace) {
        return None;
    }
    Some(rest.trim().to_string())
}

/// Keywords that attach to the element that follows them rather than standing alone.
fn is_affiliated(key: &str) -> bool {
    let k = key.to_ascii_uppercase();
    matches!(k.as_str(), "CAPTION" | "NAME" | "ATTR_HTML")
}

/// A paragraph holding nothing but an image link becomes a block-level figure when a
/// `#+CAPTION:`/`#+ATTR_HTML:` precedes it. Affiliated keywords on anything else are
/// parsed and dropped (README §IN covers captions for images only).
fn attach_affiliated(element: Element, affiliated: Vec<(String, String)>) -> Element {
    let value = |key: &str| {
        affiliated
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(key))
            .map(|(_, v)| v.clone())
    };
    let caption = value("CAPTION").unwrap_or_default();
    let attrs = value("ATTR_HTML").unwrap_or_default();
    if caption.is_empty() && attrs.is_empty() {
        return element;
    }
    let Element::Paragraph(objs) = &element else {
        return element;
    };
    let [Object::Link(link)] = objs.as_slice() else {
        return element;
    };
    if !is_image_target(&link.target) {
        return element;
    }
    Element::Figure {
        link: link.clone(),
        caption: inline(&caption),
        attrs,
    }
}

/// Does this link point at an image file? Drives both figure promotion and inline
/// `<img>` rendering.
pub fn is_image_target(target: &LinkTarget) -> bool {
    let path = match target {
        LinkTarget::File { path, .. } => path.as_str(),
        LinkTarget::External(url) => url.split(['?', '#']).next().unwrap_or(url),
        _ => return false,
    };
    let Some(ext) = path.rsplit('.').next() else {
        return false;
    };
    matches!(
        ext.to_ascii_lowercase().as_str(),
        "png" | "jpg" | "jpeg" | "gif" | "svg" | "webp" | "avif"
    )
}

// ---------------------------------------------------------------------------
// Tables (spec §1 IN; `#+TBLFM:` formulas are parse-and-ignored via keyword_kv)
// ---------------------------------------------------------------------------

/// Consume a run of consecutive `|`-prefixed lines into a [`Table`]. Rule rows
/// (`|---+---|`) are preserved as [`TableRow::Rule`] so the renderer can locate the
/// header band.
fn parse_table(lines: &[&str], start: usize) -> (Table, usize) {
    let mut rows = Vec::new();
    let mut i = start;
    while i < lines.len() {
        let t = lines[i].trim_start();
        if !t.starts_with('|') {
            break;
        }
        if is_table_rule(t) {
            rows.push(TableRow::Rule);
        } else {
            rows.push(TableRow::Cells(parse_table_cells(t)));
        }
        i += 1;
    }
    (Table { rows }, i)
}

/// A rule row: only `|`, `-`, `+`, whitespace, and at least one `-`.
fn is_table_rule(t: &str) -> bool {
    t.starts_with('|')
        && t.contains('-')
        && t.chars().all(|c| matches!(c, '|' | '-' | '+' | ' '))
}

fn parse_table_cells(t: &str) -> Vec<Vec<Object>> {
    let inner = t.trim().trim_start_matches('|').trim_end_matches('|');
    inner.split('|').map(|cell| inline(cell.trim())).collect()
}

// ---------------------------------------------------------------------------
// Footnote definitions (spec §1 IN; inline refs handled in the inline tokenizer)
// ---------------------------------------------------------------------------

/// A footnote *definition* line: `[fn:LABEL] text...`. Returns the label and the
/// remainder on the same line. `[fn:LABEL:inline]` (a colon inside the label span) is
/// an inline reference, not a definition, so it is rejected here.
fn footnote_def_label(line: &str) -> Option<(String, String)> {
    let t = line.trim_start();
    let r = t.strip_prefix("[fn:")?;
    let end = r.find(']')?;
    let label = &r[..end];
    if label.is_empty() || label.contains(':') {
        return None;
    }
    Some((label.to_string(), r[end + 1..].trim_start().to_string()))
}

/// Gather a footnote definition's content: the remainder of its opening line plus
/// following continuation lines up to the next blank/structural/definition line.
fn parse_footnote_def(
    lines: &[&str],
    start: usize,
    label: String,
    first_rest: String,
) -> (Element, usize) {
    let mut parts: Vec<String> = Vec::new();
    if !first_rest.is_empty() {
        parts.push(first_rest);
    }
    let mut i = start + 1;
    while i < lines.len() {
        let l = lines[i];
        if l.trim().is_empty() || is_structural(l) {
            break;
        }
        parts.push(l.trim().to_string());
        i += 1;
    }
    let content = if parts.is_empty() {
        Vec::new()
    } else {
        vec![Element::Paragraph(inline(&parts.join(" ")))]
    };
    (Element::FootnoteDefinition { label, content }, i)
}

/// Consume one plain list. Items are delimited by bullets at the list's own indent
/// column; everything indented further is that item's body, re-parsed as block content —
/// which is what makes lists nest. A single blank line does not end a list, but a blank
/// line followed by anything that is not a sibling bullet does.
fn parse_list(
    lines: &[&str],
    start: usize,
    base: usize,
    diags: &mut Vec<Diagnostic>,
) -> (List, usize) {
    let base_indent = indent_of(lines[start]);
    let family = bullet_family(&is_list_item(lines[start].trim_start()).expect("list item"));
    // A list is a description list when its FIRST item carries a `::` term separator.
    let kind = match (&family, split_term(item_text(lines[start].trim_start()))) {
        (ListKind::Ordered, _) => ListKind::Ordered,
        (_, Some(_)) => ListKind::Description,
        _ => ListKind::Unordered,
    };

    let mut items = Vec::new();
    let mut i = start;
    loop {
        // Skip blank lines, but only stay in the list if a sibling bullet follows.
        let mut j = i;
        while j < lines.len() && lines[j].trim().is_empty() {
            j += 1;
        }
        if j >= lines.len() || indent_of(lines[j]) != base_indent {
            break;
        }
        let Some(bullet) = is_list_item(lines[j].trim_start()) else {
            break;
        };
        if bullet_family(&bullet) != family {
            break;
        }

        // Body = the text after the bullet, plus every following line indented past the
        // bullet column (blank lines included, so an item can hold several paragraphs).
        let rest = item_body(lines[j].trim_start(), &bullet);
        // `[@4]` comes before the checkbox: `1. [@4] [X] done`.
        let (counter, rest) = split_counter(rest);
        let (checkbox, rest) = split_checkbox(rest);
        let (term, rest) = match kind {
            ListKind::Description => match split_term(rest) {
                Some((term, def)) => (Some(inline(term.trim())), def),
                None => (None, rest),
            },
            _ => (None, rest),
        };

        let mut body: Vec<String> = vec![rest.trim().to_string()];
        i = j + 1;
        while i < lines.len() {
            if lines[i].trim().is_empty() {
                // Trailing blanks belong to the item only if more of it follows.
                let mut k = i;
                while k < lines.len() && lines[k].trim().is_empty() {
                    k += 1;
                }
                if k < lines.len() && indent_of(lines[k]) > base_indent {
                    body.resize(body.len() + (k - i), String::new());
                    i = k;
                    continue;
                }
                break;
            }
            if indent_of(lines[i]) <= base_indent {
                break;
            }
            body.push(lines[i].to_string());
            i += 1;
        }

        items.push(ListItem {
            bullet,
            counter,
            checkbox,
            term,
            // The item body starts at the bullet line, so `base + j` is exact even after
            // the body has been dedented into fresh strings.
            content: parse_elements(&dedent(&body), base + j, diags),
        });
    }
    (List { kind, items }, i)
}

/// Ordered and unordered bullets cannot share a list; description items use unordered
/// bullets, so they are the same family.
fn bullet_family(bullet: &Bullet) -> ListKind {
    match bullet {
        Bullet::Ordered(_) => ListKind::Ordered,
        _ => ListKind::Unordered,
    }
}

fn indent_of(line: &str) -> usize {
    line.len() - line.trim_start().len()
}

/// Strip the common leading indent from an item's body lines so the recursive
/// [`parse_elements`] call sees them at column zero. The first entry is already
/// dedented (it is the text that followed the bullet), so it is excluded from the
/// measurement.
fn dedent(body: &[String]) -> Vec<&str> {
    let common = body
        .iter()
        .skip(1)
        .filter(|l| !l.trim().is_empty())
        .map(|l| indent_of(l))
        .min()
        .unwrap_or(0);
    body.iter()
        .enumerate()
        .map(|(idx, l)| {
            if idx == 0 || l.len() < common {
                l.as_str()
            } else {
                &l[common..]
            }
        })
        .collect()
}

/// The text of a list item line after its bullet, for kind detection.
fn item_text(t: &str) -> &str {
    match is_list_item(t) {
        Some(bullet) => item_body(t, &bullet),
        None => t,
    }
}

/// Split `term :: definition`. The separator must be surrounded by whitespace (or end
/// the line) so `a::b` in code text is not mistaken for one.
fn split_term(text: &str) -> Option<(&str, &str)> {
    let idx = text.find(" :: ").or_else(|| {
        text.strip_suffix(" ::")
            .map(|before| before.len())
    })?;
    let term = &text[..idx];
    if term.trim().is_empty() {
        return None;
    }
    Some((term, text[idx..].trim_start_matches(" ::").trim_start()))
}

/// Text of a list item after its bullet marker.
fn item_body<'a>(item: &'a str, bullet: &Bullet) -> &'a str {
    match bullet {
        Bullet::Dash | Bullet::Plus => item[1..].trim_start(),
        Bullet::Ordered(_) => {
            // Skip digits then the `.`/`)` terminator.
            let after_digits = item.trim_start_matches(|c: char| c.is_ascii_digit());
            after_digits
                .strip_prefix('.')
                .or_else(|| after_digits.strip_prefix(')'))
                .unwrap_or(after_digits)
                .trim_start()
        }
    }
}

/// Detect a leading `[@N]` counter on a list item, which sets its number explicitly.
fn split_counter(text: &str) -> (Option<u32>, &str) {
    let Some(rest) = text.strip_prefix("[@") else {
        return (None, text);
    };
    let Some(end) = rest.find(']') else {
        return (None, text);
    };
    match rest[..end].parse::<u32>() {
        Ok(n) => (Some(n), rest[end + 1..].trim_start()),
        Err(_) => (None, text),
    }
}

/// Detect a leading `[ ]`/`[X]`/`[-]` checkbox on a list item.
fn split_checkbox(text: &str) -> (Option<Checkbox>, &str) {
    let bytes = text.as_bytes();
    if bytes.len() >= 3 && bytes[0] == b'[' && bytes[2] == b']' {
        let cb = match bytes[1] {
            b' ' => Some(Checkbox::Off),
            b'X' | b'x' => Some(Checkbox::On),
            b'-' => Some(Checkbox::Trans),
            _ => None,
        };
        if let Some(cb) = cb {
            return (Some(cb), text[3..].trim_start());
        }
    }
    (None, text)
}

// ---------------------------------------------------------------------------
// Line predicates / small parsers
// ---------------------------------------------------------------------------

fn block_begin(line: &str) -> Option<(String, String)> {
    let t = line.trim_start();
    let upper = t.to_ascii_uppercase();
    let rest_upper = upper.strip_prefix("#+BEGIN_")?;
    let kind_len = rest_upper
        .find(char::is_whitespace)
        .unwrap_or(rest_upper.len());
    // Index back into the original-case string past "#+BEGIN_".
    let base = t.len() - rest_upper.len();
    let kind = t[base..base + kind_len].to_string();
    let after = t[base + kind_len..].trim().to_string();
    Some((kind, after))
}

fn is_block_end(line: &str) -> bool {
    line.trim_start().to_ascii_uppercase().starts_with("#+END_")
}

/// Does this line close a block of exactly `kind`?
fn is_block_end_of(line: &str, kind: &str) -> bool {
    let upper = line.trim().to_ascii_uppercase();
    match upper.strip_prefix("#+END_") {
        Some(rest) => rest.trim() == kind.to_ascii_uppercase(),
        None => false,
    }
}

fn parse_src_header(after: &str) -> (Option<String>, BlockParams) {
    let mut parts = after.splitn(2, char::is_whitespace);
    let lang = parts.next().filter(|s| !s.is_empty()).map(|s| s.to_string());
    let params = BlockParams {
        raw: parts.next().unwrap_or("").trim().to_string(),
    };
    (lang, params)
}

/// `#+KEY: value`, excluding `#+BEGIN_`/`#+END_` block delimiters.
fn keyword_kv(line: &str) -> Option<(String, String)> {
    let t = line.trim_start();
    let rest = t.strip_prefix("#+")?;
    if rest.to_ascii_uppercase().starts_with("BEGIN_")
        || rest.to_ascii_uppercase().starts_with("END_")
    {
        return None;
    }
    let colon = rest.find(':')?;
    let key = rest[..colon].trim().to_string();
    if key.is_empty() {
        return None;
    }
    let value = rest[colon + 1..].trim().to_string();
    Some((key, value))
}

fn is_rule(line: &str) -> bool {
    let t = line.trim();
    t.len() >= 5 && t.chars().all(|c| c == '-')
}

fn is_drawer_begin(t: &str) -> bool {
    if !t.starts_with(':') || !t.ends_with(':') || t.len() < 3 {
        return false;
    }
    let inner = &t[1..t.len() - 1];
    !inner.is_empty()
        && inner
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
}

/// If `t` (already left-trimmed) begins a list item, return its bullet.
fn is_list_item(t: &str) -> Option<Bullet> {
    let bytes = t.as_bytes();
    if bytes.is_empty() {
        return None;
    }
    if (bytes[0] == b'-' || bytes[0] == b'+')
        && (bytes.len() == 1 || bytes[1] == b' ')
    {
        return Some(if bytes[0] == b'-' {
            Bullet::Dash
        } else {
            Bullet::Plus
        });
    }
    let digits: String = t.chars().take_while(|c| c.is_ascii_digit()).collect();
    if !digits.is_empty() {
        let after = &t[digits.len()..];
        if (after.starts_with('.') || after.starts_with(')'))
            && (after.len() == 1 || after.as_bytes()[1] == b' ')
        {
            if let Ok(n) = digits.parse::<u32>() {
                return Some(Bullet::Ordered(n));
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Inline tokenizer (spec §3.1, R3)
// ---------------------------------------------------------------------------

fn parse_inline_run(chars: &[char]) -> Vec<Object> {
    let mut out = Vec::new();
    let mut buf = String::new();
    let mut i = 0;
    let n = chars.len();
    while i < n {
        let c = chars[i];
        if c == '[' && starts_with_at(chars, i, "[fn:") {
            if let Some((obj, next)) = try_footnote_ref(chars, i) {
                flush(&mut buf, &mut out);
                out.push(obj);
                i = next;
                continue;
            }
        }
        if c == '[' && i + 1 < n && chars[i + 1] == '[' {
            if let Some((obj, next)) = try_link(chars, i) {
                flush(&mut buf, &mut out);
                out.push(obj);
                i = next;
                continue;
            }
        }
        if c == '<' || c == '[' {
            if let Some((obj, next)) = try_timestamp(chars, i) {
                flush(&mut buf, &mut out);
                out.push(obj);
                i = next;
                continue;
            }
        }
        if is_scheme_start(chars, i) && boundary_before(chars, i) {
            if let Some((obj, next)) = try_bare_url(chars, i) {
                flush(&mut buf, &mut out);
                out.push(obj);
                i = next;
                continue;
            }
        }
        if is_marker(c) {
            if let Some((obj, next)) = try_emphasis(chars, i) {
                flush(&mut buf, &mut out);
                out.push(obj);
                i = next;
                continue;
            }
        }
        buf.push(c);
        i += 1;
    }
    flush(&mut buf, &mut out);
    out
}

fn flush(buf: &mut String, out: &mut Vec<Object>) {
    if !buf.is_empty() {
        out.push(Object::Text(std::mem::take(buf)));
    }
}

fn starts_with_at(chars: &[char], i: usize, needle: &str) -> bool {
    let n: Vec<char> = needle.chars().collect();
    i + n.len() <= chars.len() && chars[i..i + n.len()] == n[..]
}

/// A footnote reference: `[fn:LABEL]` (referenced) or `[fn:LABEL:text]` (inline
/// definition). Anonymous inline footnotes `[fn::text]` carry an empty label.
fn try_footnote_ref(chars: &[char], i: usize) -> Option<(Object, usize)> {
    let n = chars.len();
    let close = (i + 1..n).find(|&k| chars[k] == ']')?;
    let inner: String = chars[i + 1..close].iter().collect();
    let rest = inner.strip_prefix("fn:")?;
    let (label, inline_objs) = match rest.split_once(':') {
        Some((l, txt)) => {
            let txt_chars: Vec<char> = txt.chars().collect();
            (l.to_string(), Some(parse_inline_run(&txt_chars)))
        }
        None => (rest.to_string(), None),
    };
    if label.is_empty() && inline_objs.is_none() {
        return None;
    }
    Some((
        Object::FootnoteRef {
            label,
            inline: inline_objs,
        },
        close + 1,
    ))
}

/// `[[target]]` or `[[target][description]]`.
fn try_link(chars: &[char], i: usize) -> Option<(Object, usize)> {
    let n = chars.len();
    let mut j = i + 2;
    while j + 1 < n {
        if chars[j] == ']' && chars[j + 1] == ']' {
            let inner = &chars[i + 2..j];
            let (target_str, desc) = split_link_inner(inner);
            let target = parse_target(&target_str);
            let description = desc.map(|d| parse_inline_run(&d));
            return Some((Object::Link(Link { target, description }), j + 2));
        }
        j += 1;
    }
    None
}

/// Split `target][desc` into its two halves at the first `][`.
fn split_link_inner(inner: &[char]) -> (String, Option<Vec<char>>) {
    for k in 0..inner.len().saturating_sub(1) {
        if inner[k] == ']' && inner[k + 1] == '[' {
            let target: String = inner[..k].iter().collect();
            let desc: Vec<char> = inner[k + 2..].to_vec();
            return (target, Some(desc));
        }
    }
    (inner.iter().collect(), None)
}

fn parse_target(s: &str) -> LinkTarget {
    if let Some(r) = s.strip_prefix('#') {
        LinkTarget::CustomId(r.to_string())
    } else if let Some(r) = s.strip_prefix("id:") {
        LinkTarget::Id(r.to_string())
    } else if let Some(r) = s.strip_prefix('*') {
        LinkTarget::Heading(r.to_string())
    } else if let Some(r) = s.strip_prefix("file:") {
        LinkTarget::File {
            path: r.into(),
            search: None,
        }
    } else if is_external_scheme(s) {
        LinkTarget::External(s.to_string())
    } else {
        LinkTarget::File {
            path: s.into(),
            search: None,
        }
    }
}

fn is_external_scheme(s: &str) -> bool {
    let s = s.to_ascii_lowercase();
    ["http://", "https://", "mailto:", "ftp://", "news:", "tel:"]
        .iter()
        .any(|p| s.starts_with(p))
}

fn is_scheme_start(chars: &[char], i: usize) -> bool {
    let tail: String = chars[i..].iter().take(8).collect();
    let tail = tail.to_ascii_lowercase();
    tail.starts_with("http://") || tail.starts_with("https://") || tail.starts_with("mailto:")
}

/// A bare URL in running text, e.g. `https://example.com`.
fn try_bare_url(chars: &[char], i: usize) -> Option<(Object, usize)> {
    let n = chars.len();
    let mut j = i;
    while j < n {
        let c = chars[j];
        if c.is_whitespace() || matches!(c, '<' | '>' | '[' | ']' | '"' | '{' | '}') {
            break;
        }
        j += 1;
    }
    // Trim trailing sentence punctuation that is unlikely to be part of the URL.
    while j > i && matches!(chars[j - 1], '.' | ',' | ';' | ':' | '!' | '?' | ')') {
        j -= 1;
    }
    if j <= i {
        return None;
    }
    let url: String = chars[i..j].iter().collect();
    Some((
        Object::Link(Link {
            target: LinkTarget::External(url),
            description: None,
        }),
        j,
    ))
}

// ---------------------------------------------------------------------------
// Timestamps
// ---------------------------------------------------------------------------

/// An org timestamp: `<2024-01-15 Mon>` (active) or `[2024-01-15 Mon]` (inactive), with
/// an optional `HH:MM` time, an optional `HH:MM-HH:MM` same-day range, and an optional
/// `--`-joined second stamp for a multi-day range.
fn try_timestamp(chars: &[char], i: usize) -> Option<(Object, usize)> {
    let active = chars[i] == '<';
    let (start, same_day_end, has_time, mut next) = parse_stamp(chars, i)?;
    let mut end = same_day_end;
    if end.is_none() && starts_with_at(chars, next, "--") {
        // A range's two halves must agree on activeness, or it is two adjacent stamps.
        if chars.get(next + 2) == Some(&chars[i]) {
            if let Some((stamp_end, _, _, after)) = parse_stamp(chars, next + 2) {
                end = Some(stamp_end);
                next = after;
            }
        }
    }
    Some((
        Object::Timestamp(Timestamp {
            active,
            start,
            end,
            has_time,
        }),
        next,
    ))
}

/// One bracketed stamp → `(start, same-day end, has_time, index past the bracket)`.
/// Day names (`Mon`) and repeater/warning cookies (`+1w`, `-2d`) are recognized and
/// discarded — they carry no export meaning (README §OUT: agenda semantics).
fn parse_stamp(
    chars: &[char],
    i: usize,
) -> Option<(NaiveDateTime, Option<NaiveDateTime>, bool, usize)> {
    let open = *chars.get(i)?;
    let close = match open {
        '<' => '>',
        '[' => ']',
        _ => return None,
    };
    let end = (i + 1..chars.len()).find(|&k| chars[k] == close)?;
    let body: String = chars[i + 1..end].iter().collect();
    let mut parts = body.split_whitespace();
    let date = NaiveDate::parse_from_str(parts.next()?, "%Y-%m-%d").ok()?;

    let mut has_time = false;
    let mut start_time = NaiveTime::MIN;
    let mut end_time = None;
    for part in parts {
        if let Some((from, to)) = parse_time_spec(part) {
            has_time = true;
            start_time = from;
            end_time = to;
        }
    }
    Some((
        date.and_time(start_time),
        end_time.map(|t| date.and_time(t)),
        has_time,
        end + 1,
    ))
}

/// `HH:MM` or `HH:MM-HH:MM`.
fn parse_time_spec(s: &str) -> Option<(NaiveTime, Option<NaiveTime>)> {
    let (from, to) = match s.split_once('-') {
        Some((a, b)) => (a, Some(b)),
        None => (s, None),
    };
    let from = NaiveTime::parse_from_str(from, "%H:%M").ok()?;
    let to = match to {
        Some(b) => Some(NaiveTime::parse_from_str(b, "%H:%M").ok()?),
        None => None,
    };
    Some((from, to))
}

fn is_marker(c: char) -> bool {
    matches!(c, '*' | '/' | '_' | '+' | '=' | '~')
}

fn pre_ok(prev: Option<char>) -> bool {
    match prev {
        None => true,
        Some(c) => c.is_whitespace() || matches!(c, '-' | '(' | '{' | '\'' | '"'),
    }
}

fn post_ok(next: Option<char>) -> bool {
    match next {
        None => true,
        Some(c) => {
            c.is_whitespace() || matches!(c, '-' | '.' | ',' | ';' | ':' | '!' | '?' | ')' | '}' | '[' | '"' | '\'')
        }
    }
}

/// Org emphasis with pre/post-char boundary rules. `=`/`~` carry literal content.
fn try_emphasis(chars: &[char], i: usize) -> Option<(Object, usize)> {
    let n = chars.len();
    let m = chars[i];
    let prev = if i == 0 { None } else { Some(chars[i - 1]) };
    if !pre_ok(prev) {
        return None;
    }
    if i + 1 >= n {
        return None;
    }
    // Org's body-character rule: the character after the opening marker may not be
    // whitespace, a comma or a quote. It *may* be another marker, which is what makes
    // `~~/.config/emacs~` verbatim for a path that starts with `~`.
    if !body_char_ok(chars[i + 1]) {
        return None;
    }
    let mut j = i + 1;
    while j < n {
        if chars[j] == m && j > i + 1 {
            let before = chars[j - 1];
            let next = chars.get(j + 1).copied();
            if body_char_ok(before) && post_ok(next) {
                let inner = &chars[i + 1..j];
                let obj = match m {
                    '=' => Object::Verbatim(inner.iter().collect()),
                    '~' => Object::Code(inner.iter().collect()),
                    '*' => Object::Bold(parse_inline_run(inner)),
                    '/' => Object::Italic(parse_inline_run(inner)),
                    '_' => Object::Underline(parse_inline_run(inner)),
                    '+' => Object::StrikeThrough(parse_inline_run(inner)),
                    _ => unreachable!(),
                };
                return Some((obj, j + 1));
            }
        }
        j += 1;
    }
    None
}

/// May this character sit directly inside an emphasis marker?
///
/// Only whitespace is forbidden — org's border class is `[:space:]`. A quote may open a
/// body, which is what makes `="proxied":false=` verbatim, and `=SPC m '=` may close on
/// an apostrophe. The marker character itself is allowed too, so `~~/.config/emacs~` is a
/// path that starts with a tilde.
fn body_char_ok(c: char) -> bool {
    !c.is_whitespace()
}

fn boundary_before(chars: &[char], i: usize) -> bool {
    if i == 0 {
        return true;
    }
    let c = chars[i - 1];
    c.is_whitespace() || matches!(c, '(' | '[' | '{' | '<' | '"' | '\'')
}
