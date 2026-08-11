//! Incremental build layer (spec §4): content/config/template hashing, the link
//! dependency graph, the cache manifest, and the invalidation algorithm.
//!
//! This is the hard, non-retrofittable part (spec §4). The data model is already
//! shaped for it — pure, hashable, dependency-tracked units — so this layer is mostly
//! bookkeeping over the graph.
//!
//! The three hash classes (spec §4.1) all feed a page's composed `render_key`:
//! 1. **content hash** — blake3 of a source file's raw bytes (drives re-parse).
//! 2. **config hash** — blake3 of the resolved global config (a base-URL/options change
//!    can invalidate everything).
//! 3. **template hash** — blake3 of the templates (a base-layout edit invalidates every
//!    page that uses it).
//!
//! Skip rule (spec §4.1): if a page's `render_key` is unchanged, its emitted file on
//! disk is already correct — skip it. The dependency graph (spec §4.3) additionally
//! invalidates pages that *link into* a changed file's targets, so a renamed/removed
//! heading invalidates the pages that link to it, not just the file that owns it.

use std::collections::{HashMap, HashSet};

use camino::{Utf8Path, Utf8PathBuf};
use serde::{Deserialize, Serialize};

use crate::index::{SymbolTable, TargetId};
use crate::model::ContentHash;
use crate::util::output_url;

/// Bump whenever the `Document` type, hashing scheme, or resolution rules change.
/// On mismatch: discard cache, full rebuild (spec §4.5). The blake3 crate's major
/// version is folded in as the "hash-algo version" so a hash upgrade also busts.
pub const CACHE_FORMAT_VERSION: u32 = 7;

/// blake3 hex identity for a content/config/template/render-key hash class (spec §4.1).
pub type Hash = ContentHash;

/// The resolved global build config is [`crate::config::Config`]; its hash is a
/// component of every page's render key (spec §4.1), so editing `orgo.toml`
/// invalidates the pages it affects.
pub use crate::config::Config as BuildConfig;

/// Compose bytes into a blake3 hash. The one place hashing happens for composite keys.
fn hash_bytes(bytes: &[u8]) -> Hash {
    ContentHash(*blake3::hash(bytes).as_bytes())
}

/// blake3 of the resolved global config (spec §4.1, hash class 2).
pub fn config_hash(config: &BuildConfig) -> Hash {
    let json = serde_json::to_vec(config).expect("BuildConfig serializes");
    hash_bytes(&json)
}

/// Fold two hashes into one composite (order-sensitive).
pub fn combine(a: Hash, b: Hash) -> Hash {
    let mut hasher = blake3::Hasher::new();
    hasher.update(&a.0);
    hasher.update(&b.0);
    ContentHash(*hasher.finalize().as_bytes())
}

/// Hash of the global site structure that appears in every page's chrome. The nav bar is
/// built from every page's `(path, title)`, so any title change, path change, page
/// addition, or removal alters the nav on ALL pages and must invalidate them. This is a
/// genuine global dependency (like the config/template hashes, spec §4.1), so it is
/// folded into every page's render key. Deterministic: entries are sorted.
pub fn site_structure_hash(entries: &[(String, String)]) -> Hash {
    let mut sorted = entries.to_vec();
    sorted.sort();
    site_structure_hash_ordered(&sorted)
}

/// As [`site_structure_hash`], but hashing the sequence *as given*. Used where order is
/// itself part of the output — a listing page's entries are sorted deliberately, so
/// re-ordering them is a real change even when the set is identical.
pub fn site_structure_hash_ordered(entries: &[(String, String)]) -> Hash {
    let mut hasher = blake3::Hasher::new();
    for (path, title) in entries {
        hasher.update(path.as_bytes());
        hasher.update(&[0]);
        hasher.update(title.as_bytes());
        hasher.update(&[0]);
    }
    ContentHash(*hasher.finalize().as_bytes())
}

/// blake3 over the template sources (spec §4.1, hash class 3). One combined hash over
/// all templates; when partials land, split this per-template so a single-partial edit
/// invalidates only its users.
pub fn template_hash(sources: &[(String, String)]) -> Hash {
    let mut hasher = blake3::Hasher::new();
    for (name, src) in sources {
        hasher.update(name.as_bytes());
        hasher.update(&[0]);
        hasher.update(src.as_bytes());
        hasher.update(&[0]);
    }
    ContentHash(*hasher.finalize().as_bytes())
}

/// The link dependency graph (spec §4.3). `defines`: file → targets it owns.
/// `uses`: page → targets it resolved a link to (the invalidation-critical edges).
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct DepGraph {
    pub defines: HashMap<Utf8PathBuf, HashSet<TargetId>>,
    pub uses: HashMap<Utf8PathBuf, HashSet<TargetId>>,
}

impl DepGraph {
    /// Merge `self` (typically the previous build's graph) with `other` (this build's),
    /// unioning the `defines` targets per file and taking `other`'s `uses`. Used to build
    /// the graph handed to [`invalidation_set`]: a target that a changed file *removed*
    /// is still present in the old `defines`, so pages that linked to it are still found
    /// (the renamed/removed-heading case, spec §4.3 step 2).
    pub fn merged_defines_with(&self, other: &DepGraph) -> DepGraph {
        let mut defines = self.defines.clone();
        for (file, targets) in &other.defines {
            defines.entry(file.clone()).or_default().extend(targets.iter().cloned());
        }
        DepGraph {
            defines,
            uses: other.uses.clone(),
        }
    }
}

/// blake3 over the resolved URLs of the targets a page consumed (spec §4.1: the
/// `resolved_links_hash` component). Computed relative to the linking page, so a target
/// whose resolved URL/anchor changed — a renamed `*Heading`, a moved file — flips this
/// hash and therefore the page's `render_key`. Deterministic: targets are sorted.
pub fn resolved_links_hash(
    from: &Utf8Path,
    used: &HashSet<TargetId>,
    symbols: &SymbolTable,
) -> Hash {
    let mut pairs: Vec<(String, String)> = used
        .iter()
        .map(|tid| {
            let url = symbols
                .targets
                .get(tid)
                .map(|loc| output_url(from, &loc.source_path, loc.anchor.as_deref()))
                .unwrap_or_default();
            (tid.sort_key(), url)
        })
        .collect();
    pairs.sort();
    let mut hasher = blake3::Hasher::new();
    for (tid, url) in &pairs {
        hasher.update(tid.as_bytes());
        hasher.update(&[0]);
        hasher.update(url.as_bytes());
        hasher.update(&[0]);
    }
    ContentHash(*hasher.finalize().as_bytes())
}

/// Compose a page's final-output cache key (spec §4.1):
///
/// ```text
/// render_key = H( parse_result_hash ⊕ resolved_links_hash ⊕ config_hash ⊕ template_hash )
/// ```
///
/// `parse_result_hash` is the source content hash (PARSE is a pure function of the file
/// bytes, so the content hash fully identifies the parse result). If `render_key` is
/// unchanged the on-disk output is already correct and the page is skipped.
pub fn render_key(
    parse_result_hash: Hash,
    resolved_links_hash: Hash,
    config_hash: Hash,
    template_hash: Hash,
) -> Hash {
    let mut hasher = blake3::Hasher::new();
    hasher.update(&parse_result_hash.0);
    hasher.update(&resolved_links_hash.0);
    hasher.update(&config_hash.0);
    hasher.update(&template_hash.0);
    ContentHash(*hasher.finalize().as_bytes())
}

/// Per-page cache record persisted in the manifest (spec §4.5).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PageRecord {
    pub content_hash: ContentHash,
    pub render_key: Hash,
    pub output_path: Utf8PathBuf,
}

/// The on-disk cache manifest (spec §4.5). Serialized as JSON (human-diffable; the
/// cache is an optimization, never a correctness dependency — a `--no-cache`/`clean`
/// run always produces byte-identical output).
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Manifest {
    pub format_version: u32,
    pub config_hash: Option<Hash>,
    pub pages: HashMap<Utf8PathBuf, PageRecord>,
    pub graph: DepGraph,
}

/// The cache-manifest file lives inside the output directory (spec §4.5: an on-disk
/// cache dir). `clean` removes the output directory, taking the cache with it.
pub fn manifest_path(out: &Utf8Path) -> Utf8PathBuf {
    out.join(".orgo-cache.json")
}

/// Load the manifest, returning `None` on ANY of: missing file, read/parse error, or a
/// cache-format version mismatch (spec §4.5). `None` ⇒ the caller does a full rebuild.
/// The cache is never a correctness dependency, so a corrupt cache is never a crash.
pub fn load_manifest(out: &Utf8Path) -> Option<Manifest> {
    let bytes = std::fs::read(manifest_path(out)).ok()?;
    let manifest: Manifest = serde_json::from_slice(&bytes).ok()?;
    if manifest.format_version != CACHE_FORMAT_VERSION {
        return None;
    }
    Some(manifest)
}

/// Persist the manifest into the output directory. A write failure is surfaced to the
/// caller (a failed cache write only costs the next build a full rebuild).
pub fn save_manifest(out: &Utf8Path, manifest: &Manifest) -> std::io::Result<()> {
    let json = serde_json::to_vec_pretty(manifest).expect("manifest serializes");
    std::fs::write(manifest_path(out), json)
}

/// Given the set of changed files and the dependency graph, compute the set of pages to
/// rebuild (spec §4.3 invalidation algorithm): the changed files themselves, plus every
/// page with a `uses` edge into a target *defined by* a changed file. Pass a graph whose
/// `defines` is the union of the previous and current builds (see
/// [`DepGraph::merged_defines_with`]) so that a target a changed file *removed* still
/// pulls in the pages that linked to it (the renamed/removed-heading case).
pub fn invalidation_set(
    changed: &HashSet<Utf8PathBuf>,
    graph: &DepGraph,
) -> HashSet<Utf8PathBuf> {
    // Targets touched by any changed file (added, removed, or possibly-moved).
    let mut delta_targets: HashSet<&TargetId> = HashSet::new();
    for file in changed {
        if let Some(targets) = graph.defines.get(file) {
            delta_targets.extend(targets.iter());
        }
    }
    // Changed files themselves, plus any page that links into a delta'd target.
    let mut result: HashSet<Utf8PathBuf> = changed.clone();
    for (page, used) in &graph.uses {
        if used.iter().any(|t| delta_targets.contains(t)) {
            result.insert(page.clone());
        }
    }
    result
}
