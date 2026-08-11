//! `watch`: rebuild when the source changes, driven by OS filesystem events.
//!
//! This replaced a 500ms poll loop that re-walked the whole tree twice a second to
//! compare mtimes. Native events cost nothing while nothing happens, and arrive in
//! milliseconds when something does.
//!
//! Two things matter more than the watching itself:
//!
//! 1. **Not watching our own output.** `org-ssg watch . -o _site` puts the output inside
//!    the source. Rebuilding writes files, writing files raises events, and events
//!    trigger a rebuild — a loop that never stops and never idles. [`ChangeFilter`] is
//!    what prevents it, and it is a pure function precisely so it can be tested without
//!    a filesystem.
//! 2. **Debouncing.** Saving a file in an editor is rarely one event: editors write a
//!    temp file, rename it over the original, and touch the directory. Rebuilding per
//!    event would rebuild several times per save.

use std::sync::mpsc;
use std::time::Duration;

use anyhow::{Context, Result};
use camino::{Utf8Path, Utf8PathBuf};
use notify::{Config as NotifyConfig, RecursiveMode, Watcher};

use crate::site::{build_site, BuildOptions};

/// How long the tree must be quiet before a rebuild starts. Long enough to coalesce an
/// editor's write burst, short enough to feel immediate.
pub const DEBOUNCE: Duration = Duration::from_millis(120);

/// Poll interval for the fallback watcher, used where native events are unavailable
/// (some network and container filesystems). Slower than the old poll loop on purpose:
/// it is a fallback, not the primary path.
const POLL_INTERVAL: Duration = Duration::from_secs(2);

/// Decides whether a changed path should trigger a rebuild.
///
/// Deliberately *not* the same rule as build-time discovery. Discovery skips the config
/// file and the templates directory because they are not site content — but a change to
/// either must rebuild, because both change the output. The rule here is "would this
/// change the site?", not "is this a page?".
#[derive(Debug, Clone)]
pub struct ChangeFilter {
    /// Output directory, relative to the source root, when it lives inside it.
    output_inside: Option<Utf8PathBuf>,
    /// Every spelling of the source root an event path might carry, longest first.
    ///
    /// One entry is not enough. On macOS the temp directory is `/var/…`, a symlink to
    /// `/private/var/…`, and FSEvents reports the resolved path — so stripping event
    /// paths with the root *as the user typed it* silently fails, every event keeps its
    /// absolute path, and every absolute path looks like a source change. That includes
    /// the build's own writes, so `watch` rebuilds in a loop forever.
    roots: Vec<Utf8PathBuf>,
}

impl ChangeFilter {
    /// Build a filter for a source and output directory. Paths are canonicalized so
    /// `.`, `./src`, an absolute path and a symlinked one all compare equal.
    pub fn new(src: &Utf8Path, out: &Utf8Path) -> Self {
        let canon = |p: &Utf8Path| -> Option<Utf8PathBuf> {
            std::fs::canonicalize(p)
                .ok()
                .and_then(|p| Utf8PathBuf::from_path_buf(p).ok())
        };
        let src_canon = canon(src);
        let output_inside = match (&src_canon, canon(out)) {
            (Some(src), Some(out)) => out
                .strip_prefix(src)
                .ok()
                .filter(|rel| !rel.as_str().is_empty())
                .map(|rel| rel.to_owned()),
            _ => out
                .strip_prefix(src)
                .ok()
                .filter(|rel| !rel.as_str().is_empty())
                .map(|rel| rel.to_owned()),
        };

        let mut roots: Vec<Utf8PathBuf> = src_canon.into_iter().chain([src.to_owned()]).collect();
        roots.dedup();
        // Longest first, so the most specific spelling wins.
        roots.sort_by_key(|r| std::cmp::Reverse(r.as_str().len()));
        ChangeFilter {
            output_inside,
            roots,
        }
    }

    /// Should a change to `rel` (relative to the source root) cause a rebuild?
    pub fn is_relevant(&self, rel: &Utf8Path) -> bool {
        if let Some(out) = &self.output_inside {
            if rel.starts_with(out) {
                return false;
            }
        }
        // Dot-entries: `.git` churns on every command, and the cache manifest lives in
        // the output anyway. Emacs' `.#lock` files land here too.
        if rel
            .components()
            .any(|c| c.as_str().starts_with('.') && c.as_str().len() > 1)
        {
            return false;
        }
        let Some(name) = rel.file_name() else {
            return false;
        };
        !is_editor_scratch(name)
    }

    /// Filter absolute event paths down to the relevant ones, as source-relative paths.
    ///
    /// A path that cannot be made relative to the source root is discarded rather than
    /// kept: an event from outside the watched tree cannot be a source change, and
    /// treating unrecognized paths as changes is what turns a path-spelling mismatch
    /// into an endless rebuild.
    pub fn relevant(&self, paths: impl IntoIterator<Item = Utf8PathBuf>) -> Vec<Utf8PathBuf> {
        let mut out: Vec<Utf8PathBuf> = paths
            .into_iter()
            .filter_map(|p| self.to_relative(&p))
            .filter(|rel| self.is_relevant(rel))
            .collect();
        out.sort();
        out.dedup();
        out
    }

    /// An event path as a source-relative path, under whichever spelling of the root it
    /// arrived with. Already-relative paths pass through.
    fn to_relative(&self, path: &Utf8Path) -> Option<Utf8PathBuf> {
        if path.is_relative() {
            return Some(path.to_owned());
        }
        self.roots
            .iter()
            .find_map(|root| path.strip_prefix(root).ok())
            .map(Utf8Path::to_owned)
    }
}

/// Files an editor writes beside the real one. Emacs is the relevant case: it leaves
/// `file.org~` backups, which do not start with a dot and would otherwise look like a
/// content change to a tool aimed squarely at Emacs users.
fn is_editor_scratch(name: &str) -> bool {
    name.ends_with('~')
        || name.ends_with(".swp")
        || name.ends_with(".swx")
        || name.ends_with(".tmp")
        || (name.starts_with('#') && name.ends_with('#'))
}

/// Build once, then rebuild whenever the source changes. Runs until interrupted.
pub fn run(src: &Utf8Path, out: &Utf8Path, opts: &BuildOptions) -> Result<()> {
    if !src.is_dir() {
        anyhow::bail!("watch requires a source directory: watch <src-dir> -o <out-dir>");
    }

    let report = build_site(src, out, opts)?;
    println!(
        "watching {src} -> {out}: built {} page(s) ({} rendered). Ctrl-C to stop.",
        report.pages.len(),
        report.rendered.len()
    );

    let filter = ChangeFilter::new(src, out);
    let (tx, rx) = mpsc::channel();
    let mut watcher = make_watcher(tx)?;
    watcher
        .watch(src.as_std_path(), RecursiveMode::Recursive)
        .with_context(|| format!("watching {src}"))?;

    loop {
        // Block until something happens, then keep draining while events keep arriving
        // inside the debounce window — one save produces several events, and they should
        // produce one rebuild.
        let Ok(first) = rx.recv() else {
            return Ok(()); // watcher dropped
        };
        let mut batch = vec![first];
        while let Ok(next) = rx.recv_timeout(DEBOUNCE) {
            batch.push(next);
        }

        let changed = filter.relevant(batch.into_iter().flatten());
        if changed.is_empty() {
            continue;
        }

        let summary = summarize(&changed);
        match build_site(src, out, opts) {
            Ok(report) => println!(
                "{summary}: {} rendered, {} cached",
                report.rendered.len(),
                report.skipped.len()
            ),
            // A rebuild that fails must not end the session — the usual cause is a
            // half-saved file, and the next keystroke fixes it.
            Err(e) => eprintln!("{summary}: build failed: {e:#}"),
        }
    }
}

fn summarize(changed: &[Utf8PathBuf]) -> String {
    match changed {
        [one] => format!("{one} changed"),
        [first, rest @ ..] => format!("{first} and {} more changed", rest.len()),
        [] => "changed".to_string(),
    }
}

/// The platform's native watcher, falling back to polling where that is unavailable —
/// some network and container filesystems have no event API, and `watch` failing outright
/// there would be worse than being slow.
fn make_watcher(tx: mpsc::Sender<Vec<Utf8PathBuf>>) -> Result<Box<dyn Watcher>> {
    let handler = move |result: notify::Result<notify::Event>| {
        if let Ok(event) = result {
            let paths: Vec<Utf8PathBuf> = event
                .paths
                .into_iter()
                .filter_map(|p| Utf8PathBuf::from_path_buf(p).ok())
                .collect();
            if !paths.is_empty() {
                // The receiver going away just means the loop ended.
                let _ = tx.send(paths);
            }
        }
    };

    match notify::RecommendedWatcher::new(handler.clone(), NotifyConfig::default()) {
        Ok(watcher) => Ok(Box::new(watcher)),
        Err(e) => {
            eprintln!("note: native file watching unavailable ({e}); polling every {POLL_INTERVAL:?}");
            let config = NotifyConfig::default().with_poll_interval(POLL_INTERVAL);
            let watcher = notify::PollWatcher::new(handler, config)
                .context("starting the fallback poll watcher")?;
            Ok(Box::new(watcher))
        }
    }
}
