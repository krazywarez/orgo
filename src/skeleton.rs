//! HTML → semantic skeleton reduction.
//!
//! Two HTML exporters that agree on what a document *means* will still disagree on how
//! they wrap it: org buries every section in `outline-container` divs keyed by generated
//! ids, syntect emits one `<span>` per code token, and each backend picks its own class
//! names. Byte equality therefore measures nothing. The skeleton throws all of that away
//! and keeps the part worth comparing: the ordered sequence of element opens, element
//! closes, and text runs, with `<div>`/`<span>` dropped, every attribute except
//! `href`/`src` removed, whitespace collapsed, and entities decoded.
//!
//! This is the comparison primitive behind two things at once: orgo's differential check
//! against Emacs (`tests/oracle.rs`), and the cross-language conformance corpus, where an
//! implementation in another language ports [`skeleton`] and asserts its output matches
//! orgo's checked-in golden skeleton. Keeping one implementation here, rather than a copy
//! per consumer, is the point — the reduction *is* the contract, so it must be identical
//! everywhere.

/// Elements dropped from the skeleton entirely, because once attributes are gone they
/// carry no meaning two exporters could agree or disagree *about*.
///
/// `div` is pure layout: org wraps every section in `outline-container`/`outline-text`
/// wrappers and we emit none. `span` is the same story at the inline level, and matters
/// far more than it looks: syntect emits one span per code token, so keeping them made a
/// source block contribute ~60 skeleton lines of pure noise and dragged the agreement on
/// `blocks.org` down to 36% — a number that said nothing about whether we render blocks
/// correctly. Text still carries the signal: a `<span class="todo">` shows up as its
/// text, `"TODO"`, which is the part worth comparing.
const IGNORED: &[&str] = &["div", "span"];

/// Attributes kept in the skeleton. Ids and classes are generated (`org6c28c1b`) or
/// cosmetic (`org-ul`); `href` and `src` are the content.
const KEPT_ATTRS: &[&str] = &["href", "src"];

/// HTML void elements, which never emit a close event.
const VOID: &[&str] = &[
    "br", "hr", "img", "input", "meta", "link", "col", "area", "base", "source", "wbr",
];

/// Reduce an HTML fragment to its semantic skeleton: one line per element open, element
/// close, or text run.
pub fn skeleton(html: &str) -> Vec<String> {
    let mut out = Vec::new();
    let chars: Vec<char> = html.chars().collect();
    let mut i = 0;
    let mut text = String::new();

    while i < chars.len() {
        if chars[i] != '<' {
            text.push(chars[i]);
            i += 1;
            continue;
        }

        // Comments and doctypes carry nothing.
        if chars[i..].starts_with(&['<', '!']) {
            i += match find_from(&chars, i, ">") {
                Some(end) => end - i + 1,
                None => break,
            };
            continue;
        }
        let Some(end) = find_from(&chars, i, ">") else {
            break;
        };
        let raw: String = chars[i + 1..end].iter().collect();
        i = end + 1;

        let raw = raw.trim().trim_end_matches('/').trim().to_string();
        // Text is flushed only when a tag is actually *emitted*. Text either side of an
        // ignored tag therefore merges into one run, which is what makes a highlighted
        // source block compare as the one string of code it is, rather than as a
        // token-by-token sequence that has to line up exactly.
        if let Some(name) = raw.strip_prefix('/') {
            let name = name.trim().to_ascii_lowercase();
            if !IGNORED.contains(&name.as_str()) && !VOID.contains(&name.as_str()) {
                flush_text(&mut text, &mut out);
                out.push(format!("</{name}>"));
            }
            continue;
        }
        let mut parts = raw.splitn(2, char::is_whitespace);
        let name = parts.next().unwrap_or("").to_ascii_lowercase();
        if name.is_empty() || IGNORED.contains(&name.as_str()) {
            continue;
        }
        let attrs = kept_attributes(parts.next().unwrap_or(""));
        flush_text(&mut text, &mut out);
        out.push(format!("<{name}{attrs}>"));
    }
    flush_text(&mut text, &mut out);
    out
}

fn flush_text(text: &mut String, out: &mut Vec<String>) {
    let decoded = decode_entities(text);
    let collapsed = decoded.split_whitespace().collect::<Vec<_>>().join(" ");
    if !collapsed.is_empty() {
        out.push(format!("{collapsed:?}"));
    }
    text.clear();
}

fn find_from(chars: &[char], from: usize, needle: &str) -> Option<usize> {
    let n: Vec<char> = needle.chars().collect();
    (from..chars.len()).find(|&k| chars[k..].starts_with(&n[..]))
}

/// Keep only the content-bearing attributes, in a stable order.
fn kept_attributes(rest: &str) -> String {
    let mut kept: Vec<(String, String)> = Vec::new();
    for attr in KEPT_ATTRS {
        if let Some(value) = attribute_value(rest, attr) {
            kept.push(((*attr).to_string(), value));
        }
    }
    kept.iter()
        .map(|(k, v)| format!(" {k}=\"{}\"", decode_entities(v)))
        .collect()
}

fn attribute_value(rest: &str, name: &str) -> Option<String> {
    let mut search = rest;
    while let Some(pos) = search.find(name) {
        let before_ok = pos == 0
            || search[..pos]
                .chars()
                .next_back()
                .is_some_and(char::is_whitespace);
        let after = &search[pos + name.len()..];
        let after_trimmed = after.trim_start();
        if before_ok && after_trimmed.starts_with('=') {
            let value = after_trimmed[1..].trim_start();
            let quote = value.chars().next()?;
            if quote == '"' || quote == '\'' {
                let end = value[1..].find(quote)? + 1;
                return Some(value[1..end].to_string());
            }
            let end = value.find(char::is_whitespace).unwrap_or(value.len());
            return Some(value[..end].to_string());
        }
        search = &search[pos + name.len()..];
    }
    None
}

/// Decode the entities either exporter is likely to emit, so an encoding difference is
/// never reported as a semantic one.
pub fn decode_entities(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        let tail = &rest[amp..];
        let Some(semi) = tail.find(';').filter(|s| *s <= 12) else {
            out.push('&');
            rest = &tail[1..];
            continue;
        };
        let entity = &tail[1..semi];
        let decoded = match entity {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            "nbsp" => Some(' '),
            _ => entity
                .strip_prefix('#')
                .and_then(|n| match n.strip_prefix(['x', 'X']) {
                    Some(hex) => u32::from_str_radix(hex, 16).ok(),
                    None => n.parse::<u32>().ok(),
                })
                .and_then(char::from_u32),
        };
        match decoded {
            // A non-breaking space is a space for comparison purposes.
            Some('\u{a0}') => out.push(' '),
            Some(c) => out.push(c),
            None => {
                out.push('&');
                rest = &tail[1..];
                continue;
            }
        }
        rest = &tail[semi + 1..];
    }
    out.push_str(rest);
    out
}
