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
//! Renders the v1 IN set: headings (always anchored, with TODO keyword, priority and
//! tags), paragraphs, plain lists (unordered/ordered/description, nested, with
//! checkboxes), tables, source blocks (syntect-highlighted), example/quote/center
//! blocks, HTML export blocks, horizontal rules, images and captioned figures,
//! footnotes, timestamps, and inline markup. Out-of-scope elements (generic drawers,
//! comments, stray keywords, non-HTML export blocks) render to nothing.

use std::collections::HashMap;
use std::sync::OnceLock;

use syntect::highlighting::ThemeSet;
use syntect::html::{css_for_theme_with_class_style, ClassStyle, ClassedHTMLGenerator};
use syntect::parsing::SyntaxSet;
use syntect::util::LinesWithEndings;

use crate::model::{Checkbox, Element, Link, LinkTarget, ListKind, Object, Section, TableRow};
use crate::parser::is_image_target;
use crate::resolve::ResolvedDoc;
use crate::util::{heading_anchor, plain_text, slugify};

/// A rendered HTML fragment (content only — no page chrome; spec §2.4).
#[derive(Debug, Clone)]
pub struct Html(pub String);

/// Pluggable highlighter so tree-sitter can replace syntect per-language later
/// without touching the renderer (spec §3.2, R6).
pub trait Highlighter {
    fn highlight(&self, code: &str, lang: Option<&str>) -> Html;
}

/// The class style used for both the emitted spans and the generated stylesheet. The
/// two must agree or the CSS will not match the markup.
const CLASS_STYLE: ClassStyle = ClassStyle::Spaced;

/// Syntect's default syntax definitions, loaded once per process (loading is far more
/// expensive than highlighting, and a site build highlights many blocks).
fn syntax_set() -> &'static SyntaxSet {
    static SET: OnceLock<SyntaxSet> = OnceLock::new();
    SET.get_or_init(SyntaxSet::load_defaults_newlines)
}

fn theme_set() -> &'static ThemeSet {
    static THEMES: OnceLock<ThemeSet> = OnceLock::new();
    THEMES.get_or_init(ThemeSet::load_defaults)
}

/// The stylesheet the emitted highlight classes refer to, for a named syntect theme.
/// Highlighting emits CSS classes rather than inline styles (spec §3.2), so a build must
/// also emit this. `None` means the theme name is not one syntect ships — the caller
/// reports that rather than quietly emitting an empty stylesheet, which would look like
/// highlighting is broken.
pub fn syntax_css(theme: &str) -> Option<String> {
    let theme = theme_set().themes.get(theme)?;
    css_for_theme_with_class_style(theme, CLASS_STYLE).ok()
}

/// Every theme name [`syntax_css`] accepts, for error messages and documentation.
pub fn available_themes() -> Vec<&'static str> {
    theme_set().themes.keys().map(String::as_str).collect()
}

/// The v1 highlighter: syntect tokenizing to CSS-class spans (spec §3.2, §4.2). A block
/// whose language syntect does not know falls back to escaped `<pre><code>`.
pub struct SyntectHighlighter {
    syntaxes: &'static SyntaxSet,
}

impl SyntectHighlighter {
    pub fn new() -> Self {
        SyntectHighlighter {
            syntaxes: syntax_set(),
        }
    }
}

impl Default for SyntectHighlighter {
    fn default() -> Self {
        Self::new()
    }
}

impl Highlighter for SyntectHighlighter {
    fn highlight(&self, code: &str, lang: Option<&str>) -> Html {
        let Some(syntax) = lang.and_then(|l| self.syntaxes.find_syntax_by_token(l)) else {
            return Html(plain_code(code, lang));
        };
        let mut generator =
            ClassedHTMLGenerator::new_with_class_style(syntax, self.syntaxes, CLASS_STYLE);
        for line in LinesWithEndings::from(code) {
            if generator
                .parse_html_for_line_which_includes_newline(line)
                .is_err()
            {
                return Html(plain_code(code, lang));
            }
        }
        Html(format!(
            "<pre><code class=\"{} highlight\">{}</code></pre>\n",
            language_class(lang),
            generator.finalize()
        ))
    }
}

fn plain_code(code: &str, lang: Option<&str>) -> String {
    format!(
        "<pre><code class=\"{}\">{}</code></pre>\n",
        language_class(lang),
        escape_html(code)
    )
}

fn language_class(lang: Option<&str>) -> String {
    match lang {
        Some(l) => format!("language-{}", escape_attr(l)),
        None => "language-none".to_string(),
    }
}

/// Carries the highlighter plus the footnote collector across the tree walk (spec §2.4).
struct Renderer<'a> {
    hl: &'a dyn Highlighter,
    opts: RenderOptions,
    /// Block footnote definitions, keyed by label (collected before the walk).
    block_defs: HashMap<String, Vec<Element>>,
    /// Inline footnote definitions discovered at reference sites.
    inline_defs: HashMap<String, Vec<Object>>,
    /// Reference keys in order of first appearance — drives numbering and note order.
    order: Vec<String>,
    /// Counter per heading depth, for section numbers.
    counters: Vec<usize>,
}

/// Options affecting how the tree becomes HTML. Presentation choices that belong to the
/// site rather than to the document.
#[derive(Debug, Clone, Copy)]
pub struct RenderOptions {
    /// Added to every heading's level, so a level-1 org heading can render as `<h2>`
    /// beneath a page title supplied by the layout. See
    /// [`HtmlOutput::heading_offset`](crate::config::HtmlOutput::heading_offset).
    pub heading_offset: u8,
    /// Prefix headings with `1.`, `1.1.`, … See
    /// [`HtmlOutput::section_numbers`](crate::config::HtmlOutput::section_numbers).
    pub section_numbers: bool,
}

impl Default for RenderOptions {
    fn default() -> Self {
        let html = crate::config::HtmlOutput::default();
        RenderOptions {
            heading_offset: html.heading_offset,
            section_numbers: html.section_numbers,
        }
    }
}

/// Render a resolved document to an HTML fragment, with default options.
pub fn render(doc: &ResolvedDoc, highlighter: &dyn Highlighter) -> Html {
    render_with(doc, highlighter, &RenderOptions::default())
}

/// Render a resolved document to an HTML fragment.
pub fn render_with(doc: &ResolvedDoc, highlighter: &dyn Highlighter, opts: &RenderOptions) -> Html {
    let mut r = Renderer {
        hl: highlighter,
        opts: *opts,
        block_defs: HashMap::new(),
        inline_defs: HashMap::new(),
        order: Vec::new(),
        counters: Vec::new(),
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
            let level = h.level.saturating_add(self.opts.heading_offset).clamp(1, 6);
            let anchor = heading_anchor(h);
            // A heading with no title text has no meaningful slug; emit no `id` at all
            // rather than a run of duplicate empty ones.
            if anchor.is_empty() {
                out.push_str(&format!("<h{}>", level));
            } else {
                out.push_str(&format!("<h{} id=\"{}\">", level, escape_attr(&anchor)));
            }
            if self.opts.section_numbers {
                let number = self.next_section_number(h.level);
                out.push_str(&format!(
                    "<span class=\"section-number-{}\">{number}</span> ",
                    level
                ));
            }
            // Keyword/priority markup mirrors Emacs' own HTML export classes, so output
            // stays diffable against an `emacs --batch` oracle.
            if let Some(todo) = &h.todo {
                out.push_str(&format!(
                    "<span class=\"{} {}\">{}</span> ",
                    if todo.done { "done" } else { "todo" },
                    escape_attr(&todo.name),
                    escape_html(&todo.name)
                ));
            }
            if let Some(priority) = h.priority {
                out.push_str(&format!(
                    "<span class=\"priority\">[#{}]</span> ",
                    escape_html(&priority.to_string())
                ));
            }
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

    /// The next section number at `level`, e.g. `1.`, `1.1.`, `2.`.
    ///
    /// Deeper levels reset when a shallower one advances, and a document that skips a
    /// level (a `***` under a `*`) simply starts the missing levels at 1 rather than
    /// being treated as malformed.
    fn next_section_number(&mut self, level: u8) -> String {
        let depth = usize::from(level).max(1);
        self.counters.truncate(depth);
        while self.counters.len() < depth {
            self.counters.push(0);
        }
        self.counters[depth - 1] += 1;
        let parts: Vec<String> = self.counters.iter().map(usize::to_string).collect();
        format!("{}.", parts.join("."))
    }

    fn render_element(&mut self, element: &Element, out: &mut String) {
        match element {
            Element::Paragraph(objs) => {
                out.push_str("<p>");
                self.render_objects(objs, out);
                out.push_str("</p>\n");
            }
            Element::List(list) => self.render_list(list, out),
            Element::Table(table) => self.render_table(table, out),
            Element::SrcBlock { lang, code, .. } => {
                let Html(h) = self.hl.highlight(code, lang.as_deref());
                out.push_str(&h);
            }
            Element::ExampleBlock(code) => {
                out.push_str(&format!("<pre>{}</pre>\n", escape_html(code)));
            }
            Element::QuoteBlock(inner) => {
                out.push_str("<blockquote>\n");
                for el in inner {
                    self.render_element(el, out);
                }
                out.push_str("</blockquote>\n");
            }
            Element::CenterBlock(inner) => {
                out.push_str("<div class=\"center\">\n");
                for el in inner {
                    self.render_element(el, out);
                }
                out.push_str("</div>\n");
            }
            // An `html` export block is verbatim output by definition; every other
            // backend is out of scope and drops (README §OUT).
            Element::ExportBlock { backend, raw } => {
                if backend.eq_ignore_ascii_case("html") {
                    out.push_str(raw);
                    out.push('\n');
                }
            }
            Element::Figure {
                link,
                caption,
                attrs,
            } => {
                out.push_str("<figure>");
                out.push_str(&image_tag(link, attrs, &plain_text(caption)));
                if !caption.is_empty() {
                    out.push_str("<figcaption>");
                    self.render_objects(caption, out);
                    out.push_str("</figcaption>");
                }
                out.push_str("</figure>\n");
            }
            Element::HorizontalRule => out.push_str("<hr>\n"),
            // Definitions are emitted in the footnotes section, not inline.
            Element::FootnoteDefinition { .. } => {}
            // Out of scope (generic drawers, stray keywords, comments): emitted as
            // nothing rather than crashing.
            Element::Drawer { .. } | Element::Keyword { .. } | Element::Comment(_) => {}
        }
    }

    fn render_list(&mut self, list: &crate::model::List, out: &mut String) {
        if list.kind == ListKind::Description {
            out.push_str("<dl>\n");
            for item in &list.items {
                out.push_str("<dt>");
                if let Some(term) = &item.term {
                    self.render_objects(term, out);
                }
                out.push_str("</dt>\n<dd>");
                self.render_item_content(&item.content, out);
                out.push_str("</dd>\n");
            }
            out.push_str("</dl>\n");
            return;
        }
        let tag = if list.kind == ListKind::Ordered {
            "ol"
        } else {
            "ul"
        };
        out.push_str(&format!("<{}>\n", tag));
        for item in &list.items {
            out.push_str("<li>");
            if let Some(cb) = &item.checkbox {
                out.push_str(&format!(
                    "<input type=\"checkbox\" disabled{}> ",
                    if matches!(cb, Checkbox::On) {
                        " checked"
                    } else {
                        ""
                    }
                ));
            }
            self.render_item_content(&item.content, out);
            out.push_str("</li>\n");
        }
        out.push_str(&format!("</{}>\n", tag));
    }

    /// A single-paragraph item renders its text bare — `<li>text<ul>…` rather than
    /// `<li><p>text</p><ul>…` — which is what org does and what makes a nested list read
    /// as a continuation of its parent item. An item holding *several* paragraphs wraps
    /// them all, so they do not run together.
    fn render_item_content(&mut self, content: &[Element], out: &mut String) {
        let lead_is_bare = matches!(content.first(), Some(Element::Paragraph(_)))
            && !content[1..]
                .iter()
                .any(|el| matches!(el, Element::Paragraph(_)));
        let mut rest = content;
        if lead_is_bare {
            if let Some((Element::Paragraph(objs), tail)) = content.split_first() {
                self.render_objects(objs, out);
                rest = tail;
            }
        }
        for el in rest {
            self.render_element(el, out);
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
            // A description-less link to an image is the image itself, not a link to it.
            Object::Link(link) if link.description.is_none() && is_image_target(&link.target) => {
                out.push_str(&image_tag(link, "", ""));
            }
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
            Object::Timestamp(ts) => out.push_str(&timestamp_html(ts)),
            Object::Entity(e) => out.push_str(&escape_html(e)),
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

/// An `<img>` for an image link, carrying any `#+ATTR_HTML:` attributes and falling back
/// to the caption for alt text — but only when the author did not write an `:alt` of
/// their own, since two `alt` attributes on one tag is invalid HTML.
fn image_tag(link: &Link, attrs: &str, alt: &str) -> String {
    let mut pairs = attr_html(attrs);
    if !pairs.iter().any(|(k, _)| k.eq_ignore_ascii_case("alt")) {
        pairs.insert(0, ("alt".to_string(), alt.to_string()));
    }
    let attributes: String = pairs
        .iter()
        .map(|(k, v)| format!(" {}=\"{}\"", escape_attr(k), escape_attr(v)))
        .collect();
    format!(
        "<img src=\"{}\"{}>",
        escape_attr(&link_href(&link.target)),
        attributes
    )
}

/// `#+ATTR_HTML: :width 400 :class hero` → `[(width, 400), (class, hero)]`. Values run to
/// the next `:key` token and may be double-quoted to include spaces. A malformed spec
/// contributes nothing rather than emitting broken markup.
fn attr_html(spec: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut key: Option<&str> = None;
    let mut value = String::new();
    let mut quoted: Option<String> = None;

    let flush = |out: &mut Vec<(String, String)>, key: &mut Option<&str>, value: &mut String| {
        if let Some(k) = key.take() {
            out.push((k.to_string(), value.trim().to_string()));
        }
        value.clear();
    };

    for token in spec.split_whitespace() {
        // Inside a quoted value, everything up to the closing quote is literal.
        if let Some(buf) = &mut quoted {
            buf.push(' ');
            buf.push_str(token.trim_end_matches('"'));
            if token.ends_with('"') {
                value = quoted.take().expect("quoted value in progress");
            }
            continue;
        }
        if let Some(k) = token.strip_prefix(':') {
            flush(&mut out, &mut key, &mut value);
            if !k.is_empty() {
                key = Some(k);
            }
            continue;
        }
        if key.is_none() {
            continue;
        }
        if let Some(rest) = token.strip_prefix('"') {
            if let Some(inner) = rest.strip_suffix('"') {
                value = inner.to_string();
            } else {
                quoted = Some(rest.to_string());
            }
            continue;
        }
        if !value.is_empty() {
            value.push(' ');
        }
        value.push_str(token);
    }
    if let Some(buf) = quoted {
        value = buf;
    }
    flush(&mut out, &mut key, &mut value);
    out
}

/// `<time>` markup for a timestamp. A range emits both endpoints; a same-day range
/// abbreviates its end to just the time.
fn timestamp_html(ts: &crate::model::Timestamp) -> String {
    let class = if ts.active {
        "timestamp"
    } else {
        "timestamp inactive"
    };
    let one = |dt: &chrono::NaiveDateTime, text: String| {
        let attr = if ts.has_time {
            dt.format("%Y-%m-%dT%H:%M").to_string()
        } else {
            dt.format("%Y-%m-%d").to_string()
        };
        format!(
            "<time class=\"{class}\" datetime=\"{}\">{}</time>",
            escape_attr(&attr),
            escape_html(&text)
        )
    };
    let text_of = |dt: &chrono::NaiveDateTime| {
        if ts.has_time {
            dt.format("%Y-%m-%d %H:%M").to_string()
        } else {
            dt.format("%Y-%m-%d").to_string()
        }
    };

    let mut out = one(&ts.start, text_of(&ts.start));
    if let Some(end) = &ts.end {
        out.push_str("&#8211;");
        let text = if end.date() == ts.start.date() && ts.has_time {
            end.format("%H:%M").to_string()
        } else {
            text_of(end)
        };
        out.push_str(&one(end, text));
    }
    out
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
