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
//! v0.1 scope (the CORE subset): headings + nesting, property drawers on headings,
//! paragraphs, plain lists (unordered + ordered) with checkboxes, source blocks, and
//! inline markup (bold/italic/underline/strike/verbatim/code, links, bare URLs).
//! Out of scope and left graceful (parsed-and-ignored, never crashing): tables,
//! footnotes, timestamps, TODO keywords, non-SRC blocks (kept verbatim as example
//! blocks), generic drawers other than PROPERTIES.

use camino::Utf8Path;

use crate::model::{
    BlockParams, Bullet, Checkbox, ContentHash, Document, Element, Heading, Keywords, Link,
    LinkTarget, List, ListItem, ListKind, Object, Properties, Section, Table, TableRow,
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

    // Preamble: document-level keywords are lifted into `keywords`; the remaining
    // lines become the root section's block content.
    {
        let mut body: Vec<&str> = Vec::new();
        for (l, c) in lines[..first].iter().zip(&classes[..first]) {
            if *c == Line::Keyword {
                if let Some((k, v)) = keyword_kv(l) {
                    keywords.entries.push((k, v));
                }
            } else {
                body.push(l);
            }
        }
        root.content = parse_elements(&body);
    }

    // Each heading segment runs from its own line up to (but excluding) the next heading.
    let mut flat: Vec<(u8, Section)> = Vec::new();
    for (k, &h_idx) in heading_idxs.iter().enumerate() {
        let end = heading_idxs.get(k + 1).copied().unwrap_or(lines.len());
        let heading = parse_heading(lines[h_idx]);
        let level = heading.level;
        let (heading, content) = parse_section_body(heading, &lines[h_idx + 1..end]);
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

    Ok(Document {
        source_path: path.to_owned(),
        content_hash,
        keywords,
        root,
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

fn parse_heading(line: &str) -> Heading {
    let level = heading_level(line).unwrap_or(1);
    let rest = line[level as usize..].trim();
    let (title_str, tags) = split_tags(rest);
    Heading {
        level,
        todo: None,     // TODO keywords: out of scope for v0.1.
        priority: None, // priorities: out of scope for v0.1.
        title: inline(title_str.trim()),
        tags,
        properties: Properties::default(),
        id: None,
        custom_id: None,
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

fn parse_section_body(mut heading: Heading, body: &[&str]) -> (Heading, Vec<Element>) {
    let mut idx = 0;
    while idx < body.len() && body[idx].trim().is_empty() {
        idx += 1;
    }
    if idx < body.len() && body[idx].trim().eq_ignore_ascii_case(":PROPERTIES:") {
        idx += 1;
        while idx < body.len() {
            let t = body[idx].trim();
            if t.eq_ignore_ascii_case(":END:") {
                idx += 1;
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
    }
    let content = parse_elements(&body[idx..]);
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

fn parse_elements(lines: &[&str]) -> Vec<Element> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        if line.trim().is_empty() {
            i += 1;
            continue;
        }
        if let Some((kind, after)) = block_begin(line) {
            let mut j = i + 1;
            let mut inner = Vec::new();
            while j < lines.len() && !is_block_end(lines[j]) {
                inner.push(lines[j]);
                j += 1;
            }
            let code = inner.join("\n");
            if kind.eq_ignore_ascii_case("SRC") {
                let (lang, params) = parse_src_header(&after);
                out.push(Element::SrcBlock { lang, params, code });
            } else {
                // Non-SRC blocks (quote/example/center/export) are kept verbatim for
                // v0.1 rather than richly modeled — see module scope note.
                out.push(Element::ExampleBlock(code));
            }
            i = if j < lines.len() { j + 1 } else { j };
            continue;
        }
        if is_rule(line) {
            out.push(Element::HorizontalRule);
            i += 1;
            continue;
        }
        if let Some((key, value)) = keyword_kv(line) {
            out.push(Element::Keyword { key, value });
            i += 1;
            continue;
        }
        if line.trim_start().starts_with('|') {
            let (table, next) = parse_table(lines, i);
            out.push(Element::Table(table));
            i = next;
            continue;
        }
        if let Some((label, first_rest)) = footnote_def_label(line) {
            let (def, next) = parse_footnote_def(lines, i, label, first_rest);
            out.push(def);
            i = next;
            continue;
        }
        if is_list_item(line.trim_start()).is_some() {
            let (list, next) = parse_list(lines, i);
            out.push(Element::List(list));
            i = next;
            continue;
        }
        // Paragraph: gather consecutive soft-wrapped text lines.
        let mut para = Vec::new();
        while i < lines.len() {
            let l = lines[i];
            if l.trim().is_empty() || is_structural(l) {
                break;
            }
            para.push(l.trim());
            i += 1;
        }
        if !para.is_empty() {
            out.push(Element::Paragraph(inline(&para.join(" "))));
        }
    }
    out
}

/// Is this line the start of a non-paragraph construct?
fn is_structural(line: &str) -> bool {
    let t = line.trim_start();
    block_begin(line).is_some()
        || is_block_end(line)
        || is_rule(line)
        || keyword_kv(line).is_some()
        || is_list_item(t).is_some()
        || t.starts_with('|')
        || footnote_def_label(line).is_some()
        || heading_level(line).is_some()
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

fn parse_list(lines: &[&str], start: usize) -> (List, usize) {
    let kind = match is_list_item(lines[start].trim_start()) {
        Some(Bullet::Ordered(_)) => ListKind::Ordered,
        _ => ListKind::Unordered,
    };
    let mut items = Vec::new();
    let mut i = start;
    while i < lines.len() {
        let t = lines[i].trim_start();
        let bullet = match is_list_item(t) {
            Some(b) => b,
            None => break,
        };
        let item_kind = match bullet {
            Bullet::Ordered(_) => ListKind::Ordered,
            _ => ListKind::Unordered,
        };
        if item_kind != kind {
            break;
        }
        let rest = item_body(t, &bullet);
        let (checkbox, text) = split_checkbox(rest);
        items.push(ListItem {
            bullet,
            checkbox,
            term: None, // description lists: out of scope for v0.1.
            content: vec![Element::Paragraph(inline(text.trim()))],
        });
        i += 1;
    }
    (List { kind, items }, i)
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
    let after = chars[i + 1];
    if after.is_whitespace() || after == m {
        return None;
    }
    let mut j = i + 1;
    while j < n {
        if chars[j] == m && j > i + 1 {
            let before = chars[j - 1];
            let next = chars.get(j + 1).copied();
            if !before.is_whitespace() && post_ok(next) {
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

fn boundary_before(chars: &[char], i: usize) -> bool {
    if i == 0 {
        return true;
    }
    let c = chars[i - 1];
    c.is_whitespace() || matches!(c, '(' | '[' | '{' | '<' | '"' | '\'')
}
