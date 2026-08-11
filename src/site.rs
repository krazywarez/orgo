//! Site build: walk a source directory, PARSE every `.org` file, INDEX their targets,
//! then RESOLVE + RENDER + TEMPLATE each page into a linked static site, copying
//! non-`.org` assets through unchanged (spec §2.1 DISCOVER…EMIT).
//!
//! v0.3 wires in the incremental layer (spec §4, [`crate::incremental`]): a persisted
//! cache manifest lets a rebuild re-render only the pages whose composed `render_key`
//! changed, plus the pages that *link into* a changed file's targets (the dependency
//! graph, spec §4.3). Unchanged pages keep their existing on-disk output untouched.
//! `--no-cache` forces a full rebuild; the cache is never a correctness dependency, so a
//! full rebuild and an incremental rebuild produce byte-identical output.

use std::collections::HashSet;
use std::fs;

use anyhow::{Context, Result};
use camino::{Utf8Path, Utf8PathBuf};
use walkdir::WalkDir;

use crate::incremental::{
    self, combine, config_hash, render_key, resolved_links_hash, site_structure_hash,
    template_hash, BuildConfig, DepGraph, Hash, Manifest, PageRecord, CACHE_FORMAT_VERSION,
};
use crate::index::{document_targets, SymbolTable, TargetId};
use crate::model::{ContentHash, Document};
use crate::parser::parse;
use crate::render::{render, syntax_css, Html, SyntectHighlighter};
use crate::resolve::resolve;
use crate::template::{template_sources, NavItem, Templater};
use crate::util::{output_url, relative_root};

/// A fully built page: source and output paths (relative to their roots) and its
/// final templated HTML.
#[derive(Debug, Clone)]
pub struct BuiltPage {
    pub source: Utf8PathBuf,
    pub output: Utf8PathBuf,
    pub title: String,
    pub html: String,
}

/// Unresolved internal links found during a build: `(page, target)` (spec §4.3.4).
pub type BrokenLinks = Vec<(Utf8PathBuf, TargetId)>;

/// Options controlling a site build.
#[derive(Debug, Clone, Default)]
pub struct BuildOptions {
    /// Bypass the incremental cache and re-render every page (spec §4.5).
    pub no_cache: bool,
    /// Treat broken internal links as a build error rather than a warning (spec §4.3.4).
    pub strict: bool,
}

/// Summary of a site build.
#[derive(Debug, Default)]
pub struct SiteReport {
    /// Every output page (rendered this build or reused from cache).
    pub pages: Vec<Utf8PathBuf>,
    /// Pages actually re-rendered and written this build (the invalidation set).
    pub rendered: Vec<Utf8PathBuf>,
    /// Pages whose existing on-disk output was reused unchanged (spec §4.1 skip rule).
    pub skipped: Vec<Utf8PathBuf>,
    pub assets: Vec<Utf8PathBuf>,
    /// Unresolved internal links: `(page, target)`. Warnings, not failures (spec §4.3.4).
    pub broken: Vec<(Utf8PathBuf, TargetId)>,
}

/// Everything a build needs about one page *before* the decision to render it: its
/// hashes, its resolved element tree, and the dependency edges it participates in.
struct PagePrep {
    source: Utf8PathBuf,
    output: Utf8PathBuf,
    title: String,
    content_hash: ContentHash,
    resolved: crate::resolve::ResolvedDoc,
    used: HashSet<TargetId>,
    defines: HashSet<TargetId>,
    broken: Vec<TargetId>,
    nav: Vec<NavItem>,
}

/// DISCOVER + PARSE + INDEX + RESOLVE the whole site, returning per-page prep and the
/// global symbol table. RENDER/TEMPLATE is deferred to the caller so the incremental
/// build can render only the pages it must. PARSE/INDEX/RESOLVE are cheap and pure, so
/// they run for every file each build; the incremental win is on RENDER + EMIT (spec §4.4).
fn prepare_pages(src: &Utf8Path) -> Result<(Vec<PagePrep>, SymbolTable)> {
    let (org_rel, _assets) = discover(src)?;

    // PARSE every file (relative paths keep snapshots and links machine-independent).
    let mut docs: Vec<Document> = Vec::new();
    for rel in &org_rel {
        let abs = src.join(rel);
        let source = fs::read_to_string(&abs).with_context(|| format!("reading {abs}"))?;
        let doc = parse(rel.as_path(), &source).with_context(|| format!("parsing {rel}"))?;
        docs.push(doc);
    }

    // INDEX: collect every link target across the corpus.
    let mut symbols = SymbolTable::new();
    for doc in &docs {
        symbols.index_document(doc);
    }

    // Nav is global; titles come from #+TITLE (falling back to the file stem).
    let entries: Vec<(Utf8PathBuf, String)> = docs
        .iter()
        .map(|d| (d.source_path.clone(), page_title(d)))
        .collect();

    let mut pages = Vec::new();
    for doc in &docs {
        let out = resolve(doc, &symbols);
        let used: HashSet<TargetId> = out.used_targets.iter().cloned().collect();
        let broken: Vec<TargetId> = out.broken.iter().map(|b| b.target.clone()).collect();
        let defines: HashSet<TargetId> = document_targets(doc).into_iter().collect();

        // Nav links are relative to *this* page (spec URL scheme, §8 Q3).
        let nav: Vec<NavItem> = entries
            .iter()
            .map(|(path, title)| NavItem {
                title: title.clone(),
                url: output_url(&doc.source_path, path, None),
            })
            .collect();

        pages.push(PagePrep {
            source: doc.source_path.clone(),
            output: doc.source_path.with_extension("html"),
            title: page_title(doc),
            content_hash: doc.content_hash,
            resolved: out.resolved,
            used,
            defines,
            broken,
            nav,
        });
    }

    Ok((pages, symbols))
}

/// Parse + index + resolve + render + template a whole site *in memory*, without
/// touching the output directory. Shared by the tests (full render, every page).
pub fn render_site(src: &Utf8Path) -> Result<(Vec<BuiltPage>, BrokenLinks)> {
    let (preps, _symbols) = prepare_pages(src)?;
    let highlighter = SyntectHighlighter::new();
    let templater = Templater::new();

    let mut pages = Vec::new();
    let mut broken = Vec::new();
    for p in &preps {
        for t in &p.broken {
            broken.push((p.source.clone(), t.clone()));
        }
        let html = render_page(&templater, &highlighter, p)?;
        pages.push(BuiltPage {
            source: p.source.clone(),
            output: p.output.clone(),
            title: p.title.clone(),
            html,
        });
    }
    Ok((pages, broken))
}

/// RENDER + TEMPLATE one prepared page into its final HTML string.
fn render_page(
    templater: &Templater,
    highlighter: &SyntectHighlighter,
    p: &PagePrep,
) -> Result<String> {
    let Html(fragment) = render(&p.resolved, highlighter);
    let stylesheet = format!("{}{}", relative_root(&p.source), SYNTAX_STYLESHEET);
    templater
        .render_page(&p.title, &fragment, &p.nav, &stylesheet)
        .with_context(|| format!("templating {}", p.source))
}

/// Site-root-relative name of the generated syntax stylesheet. Every page links to it.
pub const SYNTAX_STYLESHEET: &str = "syntax.css";

/// Full site build with the incremental layer (spec §4). Renders only the pages whose
/// `render_key` changed or that link into a changed file's targets; reuses the on-disk
/// output of everything else; persists an updated cache manifest.
pub fn build_site(src: &Utf8Path, out: &Utf8Path, opts: &BuildOptions) -> Result<SiteReport> {
    let (_org_rel, assets) = discover(src)?;
    let (preps, symbols) = prepare_pages(src)?;

    // The global hash classes (spec §4.1): a change in any invalidates the site. The
    // config hash is combined with a site-structure hash because the nav bar — global
    // chrome on every page — is built from every page's (path, title), so a title/path
    // change or a page add/remove must re-render every page (else stale nav on disk).
    let cfg = BuildConfig::default();
    let nav_entries: Vec<(String, String)> = preps
        .iter()
        .map(|p| (p.source.to_string(), p.title.clone()))
        .collect();
    let cfg_hash = combine(config_hash(&cfg), site_structure_hash(&nav_entries));
    let tmpl_hash = template_hash(template_sources());

    // Compose each page's render key and record its dependency edges.
    let mut new_graph = DepGraph::default();
    let mut new_records: Vec<(Utf8PathBuf, PageRecord, Hash)> = Vec::new();
    for p in &preps {
        let rlh = resolved_links_hash(&p.source, &p.used, &symbols);
        let key = render_key(p.content_hash, rlh, cfg_hash, tmpl_hash);
        new_graph.defines.insert(p.source.clone(), p.defines.clone());
        new_graph.uses.insert(p.source.clone(), p.used.clone());
        new_records.push((
            p.source.clone(),
            PageRecord {
                content_hash: p.content_hash,
                render_key: key,
                output_path: p.output.clone(),
            },
            key,
        ));
    }

    // Load the prior manifest (unless bypassed). Absent/corrupt/version-mismatch ⇒ None
    // ⇒ full rebuild (spec §4.5).
    let prior = if opts.no_cache {
        None
    } else {
        incremental::load_manifest(out)
    };

    let rebuild: HashSet<Utf8PathBuf> = compute_rebuild_set(
        &preps,
        &new_records,
        &new_graph,
        cfg_hash,
        tmpl_hash,
        out,
        prior.as_ref(),
    );

    // Delete outputs for pages that existed last build but are gone now (spec §4.3 step 1:
    // removed files). Their targets are already in the merged graph, so their linkers were
    // invalidated above.
    if let Some(prior) = &prior {
        let current: HashSet<&Utf8PathBuf> = preps.iter().map(|p| &p.source).collect();
        for (src_path, rec) in &prior.pages {
            if !current.contains(src_path) {
                let dest = out.join(&rec.output_path);
                let _ = fs::remove_file(&dest);
            }
        }
    }

    let highlighter = SyntectHighlighter::new();
    let templater = Templater::new();
    let mut report = SiteReport::default();

    for p in &preps {
        for t in &p.broken {
            report.broken.push((p.source.clone(), t.clone()));
        }
        report.pages.push(p.output.clone());

        let dest = out.join(&p.output);
        if rebuild.contains(&p.source) {
            if let Some(parent) = dest.parent() {
                fs::create_dir_all(parent).with_context(|| format!("creating {parent}"))?;
            }
            let html = render_page(&templater, &highlighter, p)?;
            fs::write(&dest, &html).with_context(|| format!("writing {dest}"))?;
            report.rendered.push(p.output.clone());
        } else {
            // Skip: the on-disk output is already correct (spec §4.1). Leave it untouched.
            report.skipped.push(p.output.clone());
        }
    }

    // The syntax stylesheet the highlighter's CSS classes refer to. Written every build
    // (it is a few KB and depends only on the theme, which lives in the config hash).
    fs::create_dir_all(out).with_context(|| format!("creating {out}"))?;
    fs::write(out.join(SYNTAX_STYLESHEET), syntax_css())
        .with_context(|| format!("writing {SYNTAX_STYLESHEET} under {out}"))?;

    // Assets are a dumb copy in v0.3 (spec §8 Q11): copy every run. Cheap, and keeps the
    // full-vs-incremental byte equivalence trivially true for non-`.org` files.
    for rel in &assets {
        let from = src.join(rel);
        let dest = out.join(rel);
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent).with_context(|| format!("creating {parent}"))?;
        }
        fs::copy(&from, &dest).with_context(|| format!("copying {from} -> {dest}"))?;
        report.assets.push(rel.clone());
    }

    // Persist the manifest for the next build.
    let manifest = Manifest {
        format_version: CACHE_FORMAT_VERSION,
        config_hash: Some(cfg_hash),
        template_hash: Some(tmpl_hash),
        pages: new_records
            .into_iter()
            .map(|(src_path, rec, _)| (src_path, rec))
            .collect(),
        graph: new_graph,
    };
    incremental::save_manifest(out, &manifest)
        .with_context(|| format!("writing cache manifest under {out}"))?;

    if opts.strict && !report.broken.is_empty() {
        for (page, target) in &report.broken {
            eprintln!("error: {page}: unresolved link {target}");
        }
        anyhow::bail!(
            "{} unresolved internal link(s) under --strict",
            report.broken.len()
        );
    }
    for (page, target) in &report.broken {
        eprintln!("warning: {page}: unresolved link {target}");
    }

    Ok(report)
}

/// The set of source files to (re)render this build (spec §4.3 invalidation algorithm),
/// as the union of:
/// - **no prior cache** (absent/corrupt/version-mismatch/`--no-cache`) ⇒ every page;
/// - a **global** config- or template-hash change ⇒ every page (spec §4.1);
/// - **content-changed** files ∪ pages that link into a changed file's targets, via the
///   dependency graph merged with the prior build's `defines` (spec §4.3, so a removed
///   target still invalidates its linkers);
/// - any page whose composed **render_key** differs from the cached one (catches URL
///   changes on linked targets precisely);
/// - any page whose **output file is missing** on disk.
fn compute_rebuild_set(
    preps: &[PagePrep],
    new_records: &[(Utf8PathBuf, PageRecord, Hash)],
    new_graph: &DepGraph,
    cfg_hash: Hash,
    tmpl_hash: Hash,
    out: &Utf8Path,
    prior: Option<&Manifest>,
) -> HashSet<Utf8PathBuf> {
    let all: HashSet<Utf8PathBuf> = preps.iter().map(|p| p.source.clone()).collect();

    let Some(prior) = prior else {
        return all; // No usable cache ⇒ full rebuild.
    };

    // A global config/template change invalidates every page (spec §4.1).
    if prior.config_hash != Some(cfg_hash) || prior.template_hash != Some(tmpl_hash) {
        return all;
    }

    // Content-changed = hash differs from the cached record, or the file is new.
    let mut changed: HashSet<Utf8PathBuf> = HashSet::new();
    for p in preps {
        match prior.pages.get(&p.source) {
            Some(rec) if rec.content_hash == p.content_hash => {}
            _ => {
                changed.insert(p.source.clone());
            }
        }
    }

    // Graph expansion: changed files ∪ pages that link into a changed file's targets.
    // Merge prior `defines` so a target a changed file removed still pulls its linkers.
    let merged = prior.graph.merged_defines_with(new_graph);
    let mut rebuild = incremental::invalidation_set(&changed, &merged);

    // Precise render_key delta (catches a linked target's URL change; also a belt for the
    // graph). A page whose render_key matches the cache and whose output exists is correct.
    for (src_path, _rec, key) in new_records {
        let unchanged = prior
            .pages
            .get(src_path)
            .map(|old| old.render_key == *key)
            .unwrap_or(false);
        if !unchanged {
            rebuild.insert(src_path.clone());
        }
    }

    // Any page whose output file is missing must be re-emitted regardless.
    for p in preps {
        if !out.join(&p.output).exists() {
            rebuild.insert(p.source.clone());
        }
    }

    rebuild
}

/// Walk `src`, returning `.org` source paths and non-`.org` asset paths, both relative
/// to `src` and sorted for deterministic output. The cache manifest is not an asset.
fn discover(src: &Utf8Path) -> Result<(Vec<Utf8PathBuf>, Vec<Utf8PathBuf>)> {
    let mut org = Vec::new();
    let mut assets = Vec::new();
    for entry in WalkDir::new(src).sort_by_file_name() {
        let entry = entry.with_context(|| format!("walking {src}"))?;
        if !entry.file_type().is_file() {
            continue;
        }
        let abs = Utf8PathBuf::from_path_buf(entry.into_path())
            .map_err(|p| anyhow::anyhow!("non-UTF-8 path: {}", p.display()))?;
        let rel = abs
            .strip_prefix(src)
            .map(|p| p.to_owned())
            .unwrap_or_else(|_| abs.clone());
        if rel.extension() == Some("org") {
            org.push(rel);
        } else {
            assets.push(rel);
        }
    }
    org.sort();
    assets.sort();
    Ok((org, assets))
}

fn page_title(doc: &Document) -> String {
    doc.keywords
        .entries
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("TITLE"))
        .map(|(_, v)| v.clone())
        .unwrap_or_else(|| {
            doc.source_path
                .file_stem()
                .unwrap_or("untitled")
                .to_string()
        })
}
