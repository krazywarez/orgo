//! The org element tree. This *is* the document model (spec §2.2) — the parser's
//! output type is what the renderer consumes; there is no separate document AST.
//!
//! Org's two-tier structure is mirrored in the type system:
//! - [`Element`] — block-level things (headings, paragraphs, lists, tables, blocks).
//! - [`Object`] — inline things inside an element's content (bold, links, timestamps).
//!
//! This split lets the renderer never accidentally nest a heading inside emphasis.
//! The whole tree is `serde`-serializable so the parse cache and golden-file
//! snapshots share one representation (spec §4, §5).

use camino::Utf8PathBuf;
use chrono::NaiveDateTime;
use serde::{Deserialize, Serialize};

/// Content hash of raw source bytes. blake3 (spec §4.1). Serialized as hex.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ContentHash(pub [u8; 32]);

/// Parsed `#+KEYWORD:` directives (`#+TITLE`, `#+DATE`, `#+OPTIONS`, ...).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Keywords {
    pub entries: Vec<(String, String)>,
}

/// Parsed `:PROPERTIES:` drawer as an ordered key/value map.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Properties {
    pub entries: Vec<(String, String)>,
}

/// A TODO keyword resolved against the configured keyword set.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TodoKeyword {
    pub name: String,
    pub done: bool,
}

/// A problem found while parsing, carrying the 1-based source line it was found on.
///
/// Diagnostics are warnings, not errors: the parser's contract is that it always returns
/// a document (spec §1 — out-of-scope constructs degrade, never crash). What a warning
/// buys is that degrading stops being *silent*, which matters most exactly where the
/// damage is largest — an unterminated `#+BEGIN_SRC` swallows the rest of the file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Diagnostic {
    /// 1-based line number in the source file.
    pub line: usize,
    pub message: String,
}

/// One source file → one Document. This is the unit of parsing and caching (spec §2.3).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Document {
    pub source_path: Utf8PathBuf,
    pub content_hash: ContentHash,
    pub keywords: Keywords,
    /// Pre-first-heading content plus child headings.
    pub root: Section,
    /// Non-fatal problems found while parsing this file.
    #[serde(default)]
    pub diagnostics: Vec<Diagnostic>,
}

/// A section = content directly under a heading (or the file preamble), followed by
/// nested subsections. Recursive, mirroring org's headline hierarchy — so that
/// "renaming a heading invalidates its subtree's link targets" is a local operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Section {
    /// `None` for the file preamble.
    pub heading: Option<Heading>,
    /// Block-level content of THIS section.
    pub content: Vec<Element>,
    /// Nested sub-headings.
    pub children: Vec<Section>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Heading {
    pub level: u8,
    pub todo: Option<TodoKeyword>,
    /// `'A'..` from `[#A]`.
    pub priority: Option<char>,
    /// Inline objects — headings can contain markup/links.
    pub title: Vec<Object>,
    pub tags: Vec<String>,
    pub properties: Properties,
    /// `:ID:`.
    pub id: Option<String>,
    /// `:CUSTOM_ID:`.
    pub custom_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Element {
    Paragraph(Vec<Object>),
    List(List),
    Table(Table),
    SrcBlock {
        lang: Option<String>,
        params: BlockParams,
        code: String,
    },
    ExampleBlock(String),
    /// An image link carrying affiliated `#+CAPTION:`/`#+ATTR_HTML:` metadata, which
    /// promotes it from an inline image to a block-level `<figure>`.
    Figure {
        link: Link,
        caption: Vec<Object>,
        /// Raw `#+ATTR_HTML:` attribute string, passed through to the `<img>` tag.
        attrs: String,
    },
    QuoteBlock(Vec<Element>),
    CenterBlock(Vec<Element>),
    /// html passes through; others dropped at render (spec §1 OUT).
    ExportBlock {
        backend: String,
        raw: String,
    },
    HorizontalRule,
    FootnoteDefinition {
        label: String,
        content: Vec<Element>,
    },
    /// Stray `#+FOO:` kept as metadata.
    Keyword {
        key: String,
        value: String,
    },
    /// Generic drawer; LOGBOOK special-cased (spec §8 Q7).
    Drawer {
        name: String,
        content: Vec<Element>,
    },
    Comment(String),
}

/// `#+BEGIN_SRC` switches / header args. Parsed but mostly ignored in v1.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BlockParams {
    pub raw: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct List {
    pub kind: ListKind,
    pub items: Vec<ListItem>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ListKind {
    Unordered,
    Ordered,
    Description,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListItem {
    pub bullet: Bullet,
    pub checkbox: Option<Checkbox>,
    /// Description-list term before `::`.
    pub term: Option<Vec<Object>>,
    /// Items hold block content (may nest lists).
    pub content: Vec<Element>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Bullet {
    Dash,
    Plus,
    /// `1.` / `1)` — carries the ordinal.
    Ordered(u32),
}

/// `[ ]` / `[X]` / `[-]`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Checkbox {
    Off,
    On,
    Trans,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Table {
    /// Rule rows preserved to locate the header band.
    pub rows: Vec<TableRow>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TableRow {
    Cells(Vec<Vec<Object>>),
    Rule,
}

/// Inline objects (spec §2.3).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Object {
    Text(String),
    Bold(Vec<Object>),
    Italic(Vec<Object>),
    Underline(Vec<Object>),
    StrikeThrough(Vec<Object>),
    /// `=...=` : no nested markup (String, not Vec<Object>, by design).
    Verbatim(String),
    /// `~...~` : no nested markup.
    Code(String),
    Link(Link),
    FootnoteRef {
        label: String,
        inline: Option<Vec<Object>>,
    },
    Timestamp(Timestamp),
    LineBreak,
    /// Resolved `\alpha`-style entity, only if enabled.
    Entity(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Link {
    /// Unresolved at parse time — resolution is a separate global stage (spec §2.3).
    pub target: LinkTarget,
    pub description: Option<Vec<Object>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum LinkTarget {
    /// `https:`, `mailto:`, ...
    External(String),
    File {
        path: Utf8PathBuf,
        search: Option<String>,
    },
    /// `[[id:...]]`.
    Id(String),
    /// `[[#...]]`.
    CustomId(String),
    /// `[[*Heading text]]`.
    Heading(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Timestamp {
    /// `<...>` vs `[...]`.
    pub active: bool,
    pub start: NaiveDateTime,
    pub end: Option<NaiveDateTime>,
    pub has_time: bool,
}
