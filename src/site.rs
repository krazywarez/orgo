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
use rayon::prelude::*;
use walkdir::WalkDir;

use crate::incremental::{
    self, combine, config_hash, render_key, resolved_links_hash, site_structure_hash,
    template_hash, DepGraph, Hash, Manifest, PageRecord, CACHE_FORMAT_VERSION,
};
use crate::index::{document_targets, SymbolTable, TargetId};
use crate::model::{ContentHash, Diagnostic, Document};
use crate::parser::parse;
use crate::render::{self, render_with, Html, RenderOptions, SyntectHighlighter};
use crate::resolve::resolve;
use crate::config::{self, Config, NavMode};
use crate::template::{NavItem, PageContext, SiteContext, Templater};
use crate::util::{output_path, output_url, relative_root};

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
    /// Explicit config file, overriding `org-ssg.toml` in the source directory.
    pub config_path: Option<Utf8PathBuf>,
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
    /// Parse diagnostics: `(source file, diagnostic)`, in file then line order.
    pub diagnostics: Vec<(Utf8PathBuf, Diagnostic)>,
}

impl SiteReport {
    /// Every diagnostic and broken link, formatted one per line as
    /// `file:line: message` — the form an editor can jump to.
    pub fn warnings(&self) -> Vec<String> {
        let mut out: Vec<String> = self
            .diagnostics
            .iter()
            .map(|(path, d)| format!("{path}:{}: {}", d.line, d.message))
            .collect();
        out.extend(
            self.broken
                .iter()
                .map(|(page, target)| format!("{page}: unresolved link {target}")),
        );
        out
    }
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
    diagnostics: Vec<Diagnostic>,
    nav: Vec<NavItem>,
    context: PageContext,
}

/// Which pages the configured [`NavMode`] selects, in nav order.
fn nav_selection<'a>(
    config: &Config,
    pages: &'a [(Utf8PathBuf, Utf8PathBuf, String)],
) -> Vec<&'a (Utf8PathBuf, Utf8PathBuf, String)> {
    match config.nav.mode {
        NavMode::None => Vec::new(),
        NavMode::All => pages.iter().collect(),
        NavMode::TopLevel => pages.iter().filter(|(_, out, _)| is_top_level(out)).collect(),
        // Configured order wins over discovery order — a hand-written nav is a designed
        // sequence, not an alphabetical one.
        NavMode::Explicit => config
            .nav
            .pages
            .iter()
            .filter_map(|want| pages.iter().find(|(source, _, _)| source == want))
            .collect(),
    }
}

/// DISCOVER + PARSE + INDEX + RESOLVE the whole site, returning per-page prep and the
/// global symbol table. RENDER/TEMPLATE is deferred to the caller so the incremental
/// build can render only the pages it must. PARSE/INDEX/RESOLVE are cheap and pure, so
/// they run for every file each build; the incremental win is on RENDER + EMIT (spec §4.4).
fn prepare_pages(
    src: &Utf8Path,
    config: &Config,
    out: Option<&Utf8Path>,
) -> Result<(Vec<PagePrep>, SymbolTable)> {
    let (org_rel, _assets) = discover(src, config, out)?;

    // PARSE every file (relative paths keep snapshots and links machine-independent).
    // PARSE is a pure function of one file's bytes (spec §2.1), which is exactly the
    // property that makes it safe to run in parallel. `par_iter().collect()` preserves
    // input order, so the document list — and everything downstream of it — is identical
    // to the sequential build regardless of how the work was scheduled.
    let docs: Vec<Document> = org_rel
        .par_iter()
        .map(|rel| {
            let abs = src.join(rel);
            let source = fs::read_to_string(&abs).with_context(|| format!("reading {abs}"))?;
            parse(rel.as_path(), &source).with_context(|| format!("parsing {rel}"))
        })
        .collect::<Result<Vec<_>>>()?;

    // INDEX: collect every link target across the corpus.
    let mut symbols = SymbolTable::new();
    for doc in &docs {
        symbols.index_document(doc);
    }

    // `(source, output, title)` for every page. Titles come from #+TITLE (falling back to
    // the file stem) and URLs from each page's output path, which `#+SLUG:` can rename.
    let all_pages: Vec<(Utf8PathBuf, Utf8PathBuf, String)> = docs
        .iter()
        .map(|d| {
            (
                d.source_path.clone(),
                output_path(&d.source_path, &d.keywords),
                page_title(d),
            )
        })
        .collect();

    // Two sources emitting one page would silently drop a page — and with slugs, a
    // collision is a typo away and invisible in the source filenames.
    let mut claimed: std::collections::HashMap<&Utf8PathBuf, &Utf8PathBuf> =
        std::collections::HashMap::new();
    for (source, out, _) in &all_pages {
        if let Some(other) = claimed.insert(out, source) {
            anyhow::bail!(
                "output collision: {other} and {source} both build to {out} \
                 (check their #+SLUG:)"
            );
        }
    }

    // An explicit nav naming a page that does not exist is a typo, and a silently
    // shorter nav is a poor way to learn about it.
    if config.nav.mode == NavMode::Explicit {
        for want in &config.nav.pages {
            if !all_pages.iter().any(|(source, _, _)| source == want) {
                anyhow::bail!("nav.pages lists {want}, which is not a page in {src}");
            }
        }
    }
    let entries: Vec<(Utf8PathBuf, String)> = nav_selection(config, &all_pages)
        .into_iter()
        .map(|(_, out, title)| (out.clone(), title.clone()))
        .collect();

    // RESOLVE reads the shared symbol table and writes only into its own page's output,
    // so it parallelizes for free once INDEX has finished building the table.
    let pages: Vec<PagePrep> = docs
        .par_iter()
        .map(|doc| {
        let out = resolve(doc, &symbols);
        let used: HashSet<TargetId> = out.used_targets.iter().cloned().collect();
        let broken: Vec<TargetId> = out.broken.iter().map(|b| b.target.clone()).collect();
        let defines: HashSet<TargetId> = document_targets(doc).into_iter().collect();

        let output = output_path(&doc.source_path, &doc.keywords);

        // Nav links are relative to *this* page (spec URL scheme, §8 Q3).
        let nav: Vec<NavItem> = entries
            .iter()
            .map(|(path, title)| NavItem {
                title: title.clone(),
                url: output_url(&output, path, None),
            })
            .collect();

            PagePrep {
                context: page_context(doc, &output),
                source: doc.source_path.clone(),
                output,
                title: page_title(doc),
                content_hash: doc.content_hash,
                resolved: out.resolved,
                used,
                defines,
                broken,
                diagnostics: doc.diagnostics.clone(),
                nav,
            }
        })
        .collect();

    Ok((pages, symbols))
}

/// Parse + index + resolve + render + template a whole site *in memory*, without
/// touching the output directory. Shared by the tests (full render, every page).
pub fn render_site(src: &Utf8Path) -> Result<(Vec<BuiltPage>, BrokenLinks)> {
    let config = Config::load(src)?;
    config.validate()?;
    let (preps, _symbols) = prepare_pages(src, &config, None)?;
    let highlighter = SyntectHighlighter::new();
    let templater = Templater::load(Some(&src.join(&config.templates.dir)))?;
    let site = site_context(&config);
    let listing = page_listing(&config, &preps);
    let render_opts = render_options(&config);

    let mut pages = Vec::new();
    let mut broken = Vec::new();
    for p in &preps {
        for t in &p.broken {
            broken.push((p.source.clone(), t.clone()));
        }
        let html = render_page(&templater, &highlighter, &site, listing.as_deref(), &render_opts, p)?;
        pages.push(BuiltPage {
            source: p.source.clone(),
            output: p.output.clone(),
            title: p.title.clone(),
            html,
        });
    }
    Ok((pages, broken))
}

fn render_options(config: &Config) -> RenderOptions {
    RenderOptions {
        heading_offset: config.html.heading_offset,
    }
}

fn site_context(config: &Config) -> SiteContext {
    SiteContext {
        title: config.site.title.clone(),
        base_url: config.site.base_url.clone(),
        description: config.site.description.clone(),
        language: config.site.language.clone(),
    }
}

/// The `pages` list templates see, when configured to see one (see
/// [`crate::config::Templates::expose_page_list`]).
fn page_listing(config: &Config, preps: &[PagePrep]) -> Option<Vec<PageContext>> {
    config
        .templates
        .expose_page_list
        .then(|| preps.iter().map(|p| p.context.clone()).collect())
}

/// RENDER + TEMPLATE one prepared page into its final HTML string.
#[allow(clippy::too_many_arguments)]
fn render_page(
    templater: &Templater,
    highlighter: &SyntectHighlighter,
    site: &SiteContext,
    pages: Option<&[PageContext]>,
    render_opts: &RenderOptions,
    p: &PagePrep,
) -> Result<String> {
    let Html(fragment) = render_with(&p.resolved, highlighter, render_opts);
    // Relative to the *output* path, since `#+SLUG:` can move a page between depths.
    let root = relative_root(&p.output);
    let stylesheet = format!("{root}{SYNTAX_STYLESHEET}");
    templater
        .render_page(site, &p.context, &fragment, &p.nav, &stylesheet, &root, pages)
        .with_context(|| format!("templating {}", p.source))
}

/// Site-root-relative name of the generated syntax stylesheet. Every page links to it.
pub const SYNTAX_STYLESHEET: &str = "syntax.css";

/// Full site build with the incremental layer (spec §4). Renders only the pages whose
/// `render_key` changed or that link into a changed file's targets; reuses the on-disk
/// output of everything else; persists an updated cache manifest.
pub fn build_site(src: &Utf8Path, out: &Utf8Path, opts: &BuildOptions) -> Result<SiteReport> {
    let cfg = match &opts.config_path {
        Some(path) => Config::load_file(path)?,
        None => Config::load(src)?,
    };
    cfg.validate()?;

    // Create the output directory up front so it can be recognised and excluded when it
    // lives inside the source tree.
    fs::create_dir_all(out).with_context(|| format!("creating {out}"))?;
    let (_org_rel, assets) = discover(src, &cfg, Some(out))?;
    let (preps, symbols) = prepare_pages(src, &cfg, Some(out))?;

    let templater = Templater::load(Some(&src.join(&cfg.templates.dir)))?;
    let syntax_css = render::syntax_css(&cfg.highlight.theme).ok_or_else(|| {
        anyhow::anyhow!(
            "unknown highlight.theme {:?}. Available: {}",
            cfg.highlight.theme,
            render::available_themes().join(", ")
        )
    })?;

    // The global hash classes (spec §4.1): a change in any invalidates the site. The
    // config hash is combined with a site-structure hash covering the global chrome each
    // page carries, so a change to that chrome re-renders the pages showing it.
    //
    // Which pages belong in that hash depends on what a template can *see*. Normally it
    // is the nav only — a nested page cannot change another page's nav, so adding a blog
    // post should render one page, not the site. But `expose_page_list` hands every
    // template every page's metadata, and then any page's output really can depend on
    // any other page, so the hash has to widen to match. Keyed on output paths, since a
    // `#+SLUG:` change moves a page's URL without moving its source.
    let all_pages: Vec<(Utf8PathBuf, Utf8PathBuf, String)> = preps
        .iter()
        .map(|p| (p.source.clone(), p.output.clone(), p.title.clone()))
        .collect();
    let structure: Vec<(String, String)> = if cfg.templates.expose_page_list {
        all_pages
            .iter()
            .map(|(_, out, title)| (out.to_string(), title.clone()))
            .collect()
    } else {
        // The same selection the nav itself is built from, so the two can never drift.
        nav_selection(&cfg, &all_pages)
            .into_iter()
            .map(|(_, out, title)| (out.to_string(), title.clone()))
            .collect()
    };
    let cfg_hash = combine(config_hash(&cfg), site_structure_hash(&structure));
    let tmpl_hash = template_hash(templater.sources());

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
    let site = site_context(&cfg);
    let listing = page_listing(&cfg, &preps);
    let render_opts = render_options(&cfg);
    let mut report = SiteReport::default();

    // RENDER + TEMPLATE + EMIT, in parallel. This is where a build's time actually goes
    // (syntect highlighting and templating dominate), and each page writes only its own
    // file, so the pages are independent.
    //
    // The parallel pass returns whether each page was written; the report is assembled
    // sequentially afterwards from `preps` order. Pushing to the report from inside the
    // parallel pass would make `rendered`/`skipped` ordering depend on thread scheduling,
    // which would be a non-deterministic build report over a deterministic build.
    let written: Vec<bool> = preps
        .par_iter()
        .map(|p| {
            if !rebuild.contains(&p.source) {
                // Skip: the on-disk output is already correct (spec §4.1). Leave it alone.
                return Ok(false);
            }
            let dest = out.join(&p.output);
            if let Some(parent) = dest.parent() {
                fs::create_dir_all(parent).with_context(|| format!("creating {parent}"))?;
            }
            let html = render_page(&templater, &highlighter, &site, listing.as_deref(), &render_opts, p)?;
            fs::write(&dest, &html).with_context(|| format!("writing {dest}"))?;
            Ok(true)
        })
        .collect::<Result<Vec<_>>>()?;

    for (p, was_written) in preps.iter().zip(&written) {
        for t in &p.broken {
            report.broken.push((p.source.clone(), t.clone()));
        }
        for d in &p.diagnostics {
            report.diagnostics.push((p.source.clone(), d.clone()));
        }
        report.pages.push(p.output.clone());
        if *was_written {
            report.rendered.push(p.output.clone());
        } else {
            report.skipped.push(p.output.clone());
        }
    }

    // The syntax stylesheet the highlighter's CSS classes refer to. Written every build
    // (it is a few KB and depends only on the theme, which lives in the config hash).
    fs::write(out.join(SYNTAX_STYLESHEET), &syntax_css)
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

    let warnings = report.warnings();
    if opts.strict && !warnings.is_empty() {
        for w in &warnings {
            eprintln!("error: {w}");
        }
        anyhow::bail!(
            "{} problem(s) under --strict ({} parse diagnostic(s), {} unresolved link(s))",
            warnings.len(),
            report.diagnostics.len(),
            report.broken.len()
        );
    }
    for w in &warnings {
        eprintln!("warning: {w}");
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
fn discover(
    src: &Utf8Path,
    config: &Config,
    out: Option<&Utf8Path>,
) -> Result<(Vec<Utf8PathBuf>, Vec<Utf8PathBuf>)> {
    let skip_dirs = excluded_dirs(src, config, out);
    let mut org = Vec::new();
    let mut assets = Vec::new();

    let walker = WalkDir::new(src).sort_by_file_name().into_iter();
    for entry in walker.filter_entry(|e| {
        let Some(path) = Utf8Path::from_path(e.path()) else {
            return false;
        };
        let rel = path.strip_prefix(src).unwrap_or(path);
        // The source root itself always passes; `filter_entry` prunes whole subtrees.
        rel.as_str().is_empty() || !is_excluded(rel, &skip_dirs)
    }) {
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
        if rel == config::CONFIG_FILE {
            continue;
        }
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

/// Source-relative directories that DISCOVER must not descend into: the template
/// directory (build input, not content) and the output directory when it lives inside
/// the source.
///
/// The output case is not a corner case — `org-ssg build . -o _site` is the obvious
/// thing to type, and without this the build copies its own output back into itself,
/// growing `_site/_site/_site/…` on every run.
fn excluded_dirs(src: &Utf8Path, config: &Config, out: Option<&Utf8Path>) -> Vec<Utf8PathBuf> {
    let mut dirs = vec![config.templates.dir.clone()];
    if let Some(out) = out {
        // Compare canonicalized paths so `.`, `./x` and an absolute path all agree.
        // The output may not exist yet, in which case it cannot contain anything and
        // the textual fallback is enough.
        let canon = |p: &Utf8Path| -> Option<Utf8PathBuf> {
            std::fs::canonicalize(p)
                .ok()
                .and_then(|p| Utf8PathBuf::from_path_buf(p).ok())
        };
        match (canon(src), canon(out)) {
            (Some(src_abs), Some(out_abs)) => {
                if let Ok(rel) = out_abs.strip_prefix(&src_abs) {
                    if !rel.as_str().is_empty() {
                        dirs.push(rel.to_owned());
                    }
                }
            }
            _ => {
                if let Ok(rel) = out.strip_prefix(src) {
                    if !rel.as_str().is_empty() {
                        dirs.push(rel.to_owned());
                    }
                }
            }
        }
    }
    dirs
}

/// Is this source-relative path excluded from discovery?
///
/// Dot-entries are skipped wholesale. That is the conventional rule for site generators,
/// and the reason is safety rather than tidiness: a source directory is very often a git
/// repository, and publishing `.git` — or `.env` — is a way to leak a project's entire
/// history alongside its homepage.
fn is_excluded(rel: &Utf8Path, skip_dirs: &[Utf8PathBuf]) -> bool {
    if rel
        .components()
        .any(|c| c.as_str().starts_with('.') && c.as_str() != "." && c.as_str() != "..")
    {
        return true;
    }
    skip_dirs
        .iter()
        .any(|dir| !dir.as_str().is_empty() && rel.starts_with(dir))
}

/// Does this output path sit at the site root?
///
/// The nav is the site's global chrome, and listing *every* page in it makes an `n`-page
/// site emit `n²` nav links — 1,790 pages produced 284 MB of output, most of it nav. A
/// nav is a map of the site's top level, not an index of its contents, so it is built
/// from root-level pages only. Section pages reach their siblings through that section's
/// own landing page.
fn is_top_level(output: &Utf8Path) -> bool {
    output.parent().is_none_or(|p| p.as_str().is_empty())
}

/// Everything a template can know about one page. Every `#+KEYWORD:` is passed through
/// under its lowercased name, so a template can use metadata this crate has never heard
/// of without the crate needing a release to support it.
fn page_context(doc: &Document, output: &Utf8Path) -> PageContext {
    let keyword = |name: &str| {
        doc.keywords
            .entries
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.clone())
    };
    PageContext {
        title: page_title(doc),
        url: output.to_string(),
        source: doc.source_path.to_string(),
        date: keyword("DATE"),
        tags: keyword("FILETAGS")
            .unwrap_or_default()
            .split(':')
            .filter(|t| !t.trim().is_empty())
            .map(|t| t.trim().to_string())
            .collect(),
        keywords: doc
            .keywords
            .entries
            .iter()
            .map(|(k, v)| (k.to_lowercase(), v.clone()))
            .collect(),
    }
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
