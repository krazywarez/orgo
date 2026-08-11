//! RENDER stage (spec §2.1, §2.4): resolved element tree → HTML fragment.
//!
//! A tree walk emitting HTML into a buffer. Two sub-concerns get care (spec §2.4):
//! 1. Footnotes use a two-pass layout — definitions are collected up front, references
//!    are numbered in order of first appearance during the walk, and a back-linked
//!    notes section is emitted at page end.
//! 2. Syntax highlighting happens HERE, not at parse time — it is an output concern
//!    and its cost must be cache-skippable (spec §4.2). Emit CSS classes, not inline
//!    styles, so themes live in the stylesheet (spec §3.2).
//!
//! v0.2 renders: headings (always anchored, with tags), paragraphs, plain lists
//! (unordered/ordered + checkboxes), source/example blocks, horizontal rules, tables
//! (with header band from the rule row), footnotes, and inline markup. Real syntect
//! tokenizing remains a `<pre><code>` passthrough for now (see [`SyntectHighlighter`]).

use std::collections::HashMap;

use crate::model::{Checkbox, Element, LinkTarget, ListKind, Object, Section, TableRow};
use crate::resolve::ResolvedDoc;
use crate::util::{plain_text, slugify};

/// A rendered HTML fragment (content only — no page chrome; spec §2.4).
#[derive(Debug, Clone)]
pub struct Html(pub String);

/// Pluggable highlighter so tree-sitter can replace syntect per-language later
/// without touching the renderer (spec §3.2, R6).
pub trait Highlighter {
    fn highlight(&self, code: &str, lang: Option<&str>) -> Html;
}

/// Default v1 highlighter. For now this is a plain `<pre><code>` passthrough that
/// escapes the code and tags it with a `language-*` class; real syntect tokenizing
/// to CSS-class spans is deferred (spec §3.2, §4.2).
pub struct SyntectHighlighter;

impl Highlighter for SyntectHighlighter {
    fn highlight(&self, code: &str, lang: Option<&str>) -> Html {
        let class = match lang {
            Some(l) => format!(" class=\"language-{}\"", escape_attr(l)),
            None => String::new(),
        };
        Html(format!(
            "<pre><code{}>{}</code></pre>\n",
            class,
            escape_html(code)
        ))
    }
}

/// Carries the highlighter plus the footnote collector across the tree walk (spec §2.4).
struct Renderer<'a> {
    hl: &'a dyn Highlighter,
    /// Block footnote definitions, keyed by label (collected before the walk).
    block_defs: HashMap<String, Vec<Element>>,
    /// Inline footnote definitions discovered at reference sites.
    inline_defs: HashMap<String, Vec<Object>>,
    /// Reference keys in order of first appearance — drives numbering and note order.
    order: Vec<String>,
}

/// Render a resolved document to an HTML fragment.
pub fn render(doc: &ResolvedDoc, highlighter: &dyn Highlighter) -> Html {
    let mut r = Renderer {
        hl: highlighter,
        block_defs: HashMap::new(),
        inline_defs: HashMap::new(),
        order: Vec::new(),
    };
    r.collect_defs(&doc.document.root);
    let mut out = String::new();
    r.render_section(&doc.document.root, &mut out);
    r.emit_footnotes(&mut out);
    Html(out)
}

impl Renderer<'_> {
    /// First footnote pass: gather every block definition in the tree by label.
    fn collect_defs(&mut self, section: &Section) {
        collect_defs_in(&section.content, &mut self.block_defs);
        for child in &section.children {
            self.collect_defs(child);
        }
    }

    fn render_section(&mut self, section: &Section, out: &mut String) {
        if let Some(h) = &section.heading {
            let level = h.level.clamp(1, 6);
            let anchor = h
                .custom_id
                .clone()
                .or_else(|| h.id.clone())
                .unwrap_or_else(|| slugify(&plain_text(&h.title)));
            out.push_str(&format!("<h{} id=\"{}\">", level, escape_attr(&anchor)));
            self.render_objects(&h.title, out);
            for tag in &h.tags {
                out.push_str(&format!(" <span class=\"tag\">{}</span>", escape_html(tag)));
            }
            out.push_str(&format!("</h{}>\n", level));
        }
        for element in &section.content {
            self.render_element(element, out);
        }
        for child in &section.children {
            self.render_section(child, out);
        }
    }

    fn render_element(&mut self, element: &Element, out: &mut String) {
        match element {
            Element::Paragraph(objs) => {
                out.push_str("<p>");
                self.render_objects(objs, out);
                out.push_str("</p>\n");
            }
            Element::List(list) => {
                let tag = match list.kind {
                    ListKind::Ordered => "ol",
                    _ => "ul",
                };
                out.push_str(&format!("<{}>\n", tag));
                for item in &list.items {
                    out.push_str("<li>");
                    if let Some(cb) = &item.checkbox {
                        let checked = matches!(cb, Checkbox::On);
                        out.push_str(&format!(
                            "<input type=\"checkbox\" disabled{}> ",
                            if checked { " checked" } else { "" }
                        ));
                    }
                    match item.content.as_slice() {
                        [Element::Paragraph(objs)] => self.render_objects(objs, out),
                        els => {
                            for el in els {
                                self.render_element(el, out);
                            }
                        }
                    }
                    out.push_str("</li>\n");
                }
                out.push_str(&format!("</{}>\n", tag));
            }
            Element::Table(table) => self.render_table(table, out),
            Element::SrcBlock { lang, code, .. } => {
                let Html(h) = self.hl.highlight(code, lang.as_deref());
                out.push_str(&h);
            }
            Element::ExampleBlock(code) => {
                out.push_str(&format!("<pre>{}</pre>\n", escape_html(code)));
            }
            Element::HorizontalRule => out.push_str("<hr>\n"),
            // Definitions are emitted in the footnotes section, not inline.
            Element::FootnoteDefinition { .. } => {}
            // Out of scope (non-HTML export, generic drawers, stray keywords, comments):
            // emitted as nothing rather than crashing.
            _ => {}
        }
    }

    /// Rows before the first rule row become the `<thead>`; the rest are the `<tbody>`.
    fn render_table(&mut self, table: &crate::model::Table, out: &mut String) {
        let rule_at = table
            .rows
            .iter()
            .position(|r| matches!(r, TableRow::Rule));
        out.push_str("<table>\n");
        let mut wrote_body = false;
        let mut in_body = rule_at.is_none();
        for (idx, row) in table.rows.iter().enumerate() {
            match row {
                TableRow::Rule => {
                    if Some(idx) == rule_at {
                        in_body = true;
                    }
                    continue;
                }
                TableRow::Cells(cells) => {
                    let (open, cell_tag) = if in_body {
                        if !wrote_body {
                            wrote_body = true;
                            ("<tbody>\n<tr>", "td")
                        } else {
                            ("<tr>", "td")
                        }
                    } else {
                        ("<thead>\n<tr>", "th")
                    };
                    out.push_str(open);
                    for cell in cells {
                        out.push_str(&format!("<{}>", cell_tag));
                        self.render_objects(cell, out);
                        out.push_str(&format!("</{}>", cell_tag));
                    }
                    out.push_str("</tr>\n");
                    // Close the header band right after its last row.
                    if !in_body
                        && rule_at.map(|r| idx + 1 == r).unwrap_or(false)
                    {
                        out.push_str("</thead>\n");
                    }
                }
            }
        }
        if wrote_body {
            out.push_str("</tbody>\n");
        }
        out.push_str("</table>\n");
    }

    fn render_objects(&mut self, objs: &[Object], out: &mut String) {
        for obj in objs {
            self.render_object(obj, out);
        }
    }

    fn render_object(&mut self, obj: &Object, out: &mut String) {
        match obj {
            Object::Text(t) => out.push_str(&escape_html(t)),
            Object::Bold(inner) => self.wrap(out, "strong", inner),
            Object::Italic(inner) => self.wrap(out, "em", inner),
            Object::Underline(inner) => self.wrap(out, "u", inner),
            Object::StrikeThrough(inner) => self.wrap(out, "del", inner),
            Object::Verbatim(s) => {
                out.push_str(&format!("<code class=\"verbatim\">{}</code>", escape_html(s)))
            }
            Object::Code(s) => out.push_str(&format!("<code>{}</code>", escape_html(s))),
            Object::Link(link) => {
                let href = link_href(&link.target);
                out.push_str(&format!("<a href=\"{}\">", escape_attr(&href)));
                match &link.description {
                    Some(desc) => self.render_objects(desc, out),
                    None => out.push_str(&escape_html(&link_text(&link.target))),
                }
                out.push_str("</a>");
            }
            Object::FootnoteRef { label, inline } => {
                let key = if label.is_empty() {
                    format!("__anon{}", self.order.len() + 1)
                } else {
                    label.clone()
                };
                if let Some(objs) = inline {
                    self.inline_defs.insert(key.clone(), objs.clone());
                }
                if !self.order.contains(&key) {
                    self.order.push(key.clone());
                }
                let num = self.order.iter().position(|l| l == &key).unwrap() + 1;
                out.push_str(&format!(
                    "<sup class=\"footnote-ref\"><a id=\"fnr-{n}\" href=\"#fn-{n}\">{n}</a></sup>",
                    n = num
                ));
            }
            Object::LineBreak => out.push_str("<br>\n"),
            // Timestamps, entities: out of scope for now.
            _ => {}
        }
    }

    fn wrap(&mut self, out: &mut String, tag: &str, inner: &[Object]) {
        out.push_str(&format!("<{}>", tag));
        self.render_objects(inner, out);
        out.push_str(&format!("</{}>", tag));
    }

    /// Second footnote pass: emit the numbered, back-linked notes section (spec §2.4).
    fn emit_footnotes(&mut self, out: &mut String) {
        if self.order.is_empty() {
            return;
        }
        let order = self.order.clone();
        let inline_defs = self.inline_defs.clone();
        let block_defs = self.block_defs.clone();
        out.push_str("<section class=\"footnotes\">\n<hr>\n<ol>\n");
        for (idx, label) in order.iter().enumerate() {
            let n = idx + 1;
            out.push_str(&format!("<li id=\"fn-{n}\">"));
            if let Some(objs) = inline_defs.get(label) {
                self.render_objects(objs, out);
            } else if let Some(els) = block_defs.get(label) {
                for el in els {
                    self.render_element(el, out);
                }
            }
            out.push_str(&format!(
                " <a class=\"footnote-back\" href=\"#fnr-{n}\">&#8617;</a></li>\n"
            ));
        }
        out.push_str("</ol>\n</section>\n");
    }
}

fn collect_defs_in(elements: &[Element], defs: &mut HashMap<String, Vec<Element>>) {
    for el in elements {
        if let Element::FootnoteDefinition { label, content } = el {
            defs.entry(label.clone()).or_insert_with(|| content.clone());
        }
    }
}

/// Best-effort URL for a link target. After RESOLVE, internal targets have been
/// rewritten to `External` with their final URL; anything still internal here is an
/// unresolved link, rendered to a plausible anchor so the page stays self-consistent.
fn link_href(target: &LinkTarget) -> String {
    match target {
        LinkTarget::External(s) => s.clone(),
        LinkTarget::CustomId(id) => format!("#{}", id),
        LinkTarget::Id(id) => format!("#{}", id),
        LinkTarget::Heading(text) => format!("#{}", slugify(text)),
        LinkTarget::File { path, .. } => path.to_string(),
    }
}

fn link_text(target: &LinkTarget) -> String {
    match target {
        LinkTarget::External(s) => s.clone(),
        LinkTarget::CustomId(id) | LinkTarget::Id(id) => id.clone(),
        LinkTarget::Heading(text) => text.clone(),
        LinkTarget::File { path, .. } => path.to_string(),
    }
}

fn escape_html(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            _ => out.push(c),
        }
    }
    out
}

fn escape_attr(s: &str) -> String {
    let mut out = escape_html(s);
    out = out.replace('"', "&quot;");
    out
}
