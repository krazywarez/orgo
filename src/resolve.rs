//! RESOLVE stage (spec §2.1, §4.3): rewrite `LinkTarget`s to final URLs using the
//! symbol table, AND record which targets each page consumed.
//!
//! Critical invariant (spec §4.3, R2): RESOLVE returns the used-target list as a
//! first-class side output from v1, even before incrementality consumes it. Throwing
//! it away would make renamed-heading invalidation impossible to compute later without
//! re-resolving everything.
//!
//! Resolution rewrites each internal link into an [`crate::model::LinkTarget::External`]
//! carrying its final URL, so the renderer needs no symbol-table knowledge. Unresolved
//! links are left untouched (the renderer falls back to a best-effort anchor) and
//! reported as [`BrokenLink`] warnings (spec §4.3.4).

use camino::Utf8Path;

use crate::index::{SymbolTable, TargetId};
use crate::model::{Document, Element, Link, LinkTarget, Object, Section, TableRow};
use crate::util::{normalize_link_path, output_path, output_url};

/// A document whose links have been rewritten to concrete URLs.
#[derive(Debug, Clone)]
pub struct ResolvedDoc {
    pub document: Document,
}

/// A broken internal link — surfaced as a warning, or an error under `--strict` (spec §4.3.4).
#[derive(Debug, Clone)]
pub struct BrokenLink {
    pub target: TargetId,
}

/// Result of resolving one page. `used_targets` are the "uses" edges (spec §4.3).
#[derive(Debug)]
pub struct ResolveOutput {
    pub resolved: ResolvedDoc,
    /// The "uses" edges — MUST be captured from day one (spec §4.3, R2).
    pub used_targets: Vec<TargetId>,
    pub broken: Vec<BrokenLink>,
}

/// `resolve(doc, &SymbolTable) -> (ResolvedDoc, Vec<TargetId used>)` (spec §4.3).
pub fn resolve(doc: &Document, symbols: &SymbolTable) -> ResolveOutput {
    let mut document = doc.clone();
    let from = doc.source_path.clone();
    // URLs are computed between *output* paths, which `#+SLUG:` can rename.
    let from_out = output_path(&from, &doc.keywords);
    let mut used = Vec::new();
    let mut broken = Vec::new();
    let mut cx = Cx {
        from: &from,
        from_out: &from_out,
        symbols,
        used: &mut used,
        broken: &mut broken,
    };
    cx.section(&mut document.root);
    ResolveOutput {
        resolved: ResolvedDoc { document },
        used_targets: used,
        broken,
    }
}

/// The text a description-less internal link should display once its target becomes a URL.
fn human_text(target: &LinkTarget) -> Option<String> {
    match target {
        LinkTarget::Heading(t) => Some(t.clone()),
        LinkTarget::CustomId(id) | LinkTarget::Id(id) => Some(id.clone()),
        LinkTarget::File { path, .. } => Some(path.to_string()),
        LinkTarget::External(_) => None,
    }
}

struct Cx<'a> {
    from: &'a Utf8Path,
    from_out: &'a Utf8Path,
    symbols: &'a SymbolTable,
    used: &'a mut Vec<TargetId>,
    broken: &'a mut Vec<BrokenLink>,
}

impl Cx<'_> {
    fn section(&mut self, section: &mut Section) {
        if let Some(h) = &mut section.heading {
            self.objects(&mut h.title);
        }
        for el in &mut section.content {
            self.element(el);
        }
        for child in &mut section.children {
            self.section(child);
        }
    }

    fn element(&mut self, el: &mut Element) {
        match el {
            Element::Paragraph(objs) => self.objects(objs),
            Element::List(list) => {
                for item in &mut list.items {
                    if let Some(term) = &mut item.term {
                        self.objects(term);
                    }
                    for e in &mut item.content {
                        self.element(e);
                    }
                }
            }
            Element::Table(table) => {
                for row in &mut table.rows {
                    if let TableRow::Cells(cells) = row {
                        for cell in cells {
                            self.objects(cell);
                        }
                    }
                }
            }
            Element::QuoteBlock(inner)
            | Element::CenterBlock(inner)
            | Element::Drawer { content: inner, .. }
            | Element::FootnoteDefinition { content: inner, .. } => {
                for e in inner {
                    self.element(e);
                }
            }
            _ => {}
        }
    }

    fn objects(&mut self, objs: &mut [Object]) {
        for obj in objs {
            match obj {
                Object::Link(link) => self.link(link),
                Object::Bold(i)
                | Object::Italic(i)
                | Object::Underline(i)
                | Object::StrikeThrough(i) => self.objects(i),
                Object::FootnoteRef {
                    inline: Some(i), ..
                } => self.objects(i),
                _ => {}
            }
        }
    }

    fn link(&mut self, link: &mut Link) {
        if let Some(desc) = &mut link.description {
            self.objects(desc);
        }
        let tid = match &link.target {
            LinkTarget::External(_) => return,
            LinkTarget::CustomId(id) => TargetId::CustomId(id.clone()),
            LinkTarget::Id(id) => TargetId::Id(id.clone()),
            LinkTarget::Heading(t) => TargetId::Heading(t.clone()),
            LinkTarget::File { path, .. } => {
                // Only `.org` files are pages. A link to an asset (an image, a PDF) is
                // already a correct relative URL in the output tree, since assets are
                // copied preserving layout — so it is neither resolved nor reported.
                if path.extension() != Some("org") {
                    return;
                }
                TargetId::File(normalize_link_path(self.from, path))
            }
        };
        match self.symbols.targets.get(&tid) {
            Some(loc) => {
                self.used.push(tid.clone());
                // Preserve the human-readable text before the target becomes a bare URL,
                // so a description-less `[[*Heading]]` still renders as the heading text.
                if link.description.is_none() {
                    if let Some(text) = human_text(&link.target) {
                        link.description = Some(vec![Object::Text(text)]);
                    }
                }
                let url = output_url(self.from_out, &loc.output_path, loc.anchor.as_deref());
                link.target = LinkTarget::External(url);
            }
            None => {
                self.broken.push(BrokenLink { target: tid });
                // Leave the original target for the renderer's best-effort fallback.
            }
        }
    }
}
