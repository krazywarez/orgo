//! INDEX stage (spec §2.1, §4.3): collect link targets across all documents into a
//! global symbol table. This is the only inherently global stage before RESOLVE.

use std::collections::HashMap;

use camino::{Utf8Path, Utf8PathBuf};
use serde::{Deserialize, Serialize};

use crate::model::{Document, Section};
use crate::util::{plain_text, slugify};

/// Identity of a link target. A target is owned by exactly one file (spec §4.3).
///
/// `Serialize`/`Deserialize` so the dependency graph (defines/uses edges) round-trips
/// through the on-disk cache manifest (spec §4.5).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TargetId {
    Id(String),
    CustomId(String),
    Heading(String),
    File(Utf8PathBuf),
}

impl TargetId {
    /// A stable string form used to order targets deterministically when hashing
    /// (so a page's `resolved_links_hash` does not depend on `HashSet` iteration order).
    pub fn sort_key(&self) -> String {
        match self {
            TargetId::Id(s) => format!("id:{s}"),
            TargetId::CustomId(s) => format!("custom:{s}"),
            TargetId::Heading(s) => format!("heading:{s}"),
            TargetId::File(p) => format!("file:{p}"),
        }
    }
}

/// Where a resolved target lives, once INDEX has seen its defining file.
#[derive(Debug, Clone)]
pub struct TargetLocation {
    pub source_path: Utf8PathBuf,
    /// Final URL fragment/anchor for the target, filled during resolution.
    pub anchor: Option<String>,
}

/// Maps every collected target to its owning location (spec §4.3 "defines" edges).
#[derive(Debug, Default)]
pub struct SymbolTable {
    pub targets: HashMap<TargetId, TargetLocation>,
}

impl SymbolTable {
    pub fn new() -> Self {
        Self::default()
    }

    /// Walk one document, registering every `:ID:`/`:CUSTOM_ID:`/heading/file target.
    /// A target is owned by exactly one file (spec §4.3); the anchor is the fragment
    /// the renderer emits for that target's heading.
    pub fn index_document(&mut self, doc: &Document) {
        let path = &doc.source_path;
        self.targets.insert(
            TargetId::File(path.clone()),
            TargetLocation {
                source_path: path.clone(),
                anchor: None,
            },
        );
        index_section(&doc.root, path, &mut self.targets);
    }
}

/// The set of link targets a single document *defines* (owns) — the `defines` edges of
/// the dependency graph (spec §4.3). Mirrors [`SymbolTable::index_document`] but returns
/// the targets for one file in isolation, which is what the incremental layer records
/// per-file in the cache manifest.
pub fn document_targets(doc: &Document) -> Vec<TargetId> {
    let mut out = vec![TargetId::File(doc.source_path.clone())];
    collect_targets(&doc.root, &mut out);
    out
}

fn collect_targets(section: &Section, out: &mut Vec<TargetId>) {
    if let Some(h) = &section.heading {
        if let Some(cid) = &h.custom_id {
            out.push(TargetId::CustomId(cid.clone()));
        }
        if let Some(id) = &h.id {
            out.push(TargetId::Id(id.clone()));
        }
        out.push(TargetId::Heading(plain_text(&h.title)));
    }
    for child in &section.children {
        collect_targets(child, out);
    }
}

fn index_section(section: &Section, path: &Utf8Path, targets: &mut HashMap<TargetId, TargetLocation>) {
    if let Some(h) = &section.heading {
        let anchor = h
            .custom_id
            .clone()
            .or_else(|| h.id.clone())
            .unwrap_or_else(|| slugify(&plain_text(&h.title)));
        if let Some(cid) = &h.custom_id {
            targets.insert(
                TargetId::CustomId(cid.clone()),
                TargetLocation {
                    source_path: path.to_owned(),
                    anchor: Some(cid.clone()),
                },
            );
        }
        if let Some(id) = &h.id {
            targets.insert(
                TargetId::Id(id.clone()),
                TargetLocation {
                    source_path: path.to_owned(),
                    anchor: Some(id.clone()),
                },
            );
        }
        targets.insert(
            TargetId::Heading(plain_text(&h.title)),
            TargetLocation {
                source_path: path.to_owned(),
                anchor: Some(anchor),
            },
        );
    }
    for child in &section.children {
        index_section(child, path, targets);
    }
}
