//! Small shared helpers used across INDEX, RESOLVE, RENDER and the site build:
//! flattening inline objects to plain text, slugifying heading text into anchors,
//! and computing relative output URLs between pages.

use camino::{Utf8Path, Utf8PathBuf};

use crate::model::Object;

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

/// The output URL to reach `to_rel` (a source `.org` path relative to the site root)
/// from the page at `from_rel`, honoring an optional `anchor`. Same-file links reduce
/// to a bare `#anchor` fragment; cross-file links become a relative `.html` path.
pub fn output_url(from_rel: &Utf8Path, to_rel: &Utf8Path, anchor: Option<&str>) -> String {
    let path = if from_rel == to_rel {
        String::new()
    } else {
        let to_html = to_rel.with_extension("html");
        let from_dir = from_rel.parent().unwrap_or_else(|| Utf8Path::new(""));
        relative_path(from_dir, &to_html)
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
