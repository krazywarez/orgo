//! CLI entry point (spec §3.5): `build`, `watch`, `clean`.

use std::fs;

use anyhow::{Context, Result};
use camino::{Utf8Path, Utf8PathBuf};
use clap::{Parser, Subcommand};

use org_ssg::parser::parse;
use org_ssg::render::{render, syntax_css, Html, SyntectHighlighter};
use org_ssg::resolve::ResolvedDoc;
use org_ssg::site::{build_site, BuildOptions, SYNTAX_STYLESHEET};
use org_ssg::template::Templater;

#[derive(Parser)]
#[command(name = "org-ssg", version, about = "Org-mode static site generator")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Build a site. If INPUT is a directory, walk it and emit a linked static site
    /// to OUTPUT (a directory); if INPUT is a single `.org` file, emit one HTML file.
    Build {
        /// Input `.org` file, or a source directory for a whole-site build.
        input: Utf8PathBuf,
        /// Output path: an `.html` file for a single input, or a directory for a site.
        #[arg(short, long)]
        output: Option<Utf8PathBuf>,
        /// Bypass the incremental cache and re-render every page (spec §4.5).
        #[arg(long)]
        no_cache: bool,
        /// Treat broken internal links as errors (spec §4.3.4).
        #[arg(long)]
        strict: bool,
    },
    /// Watch a source directory and rebuild incrementally on change (simple poll loop).
    Watch {
        /// Source directory to watch.
        input: Utf8PathBuf,
        /// Output directory.
        #[arg(short, long)]
        output: Utf8PathBuf,
    },
    /// Remove the build output directory (which holds the cache manifest).
    Clean {
        /// Output directory to remove.
        output: Utf8PathBuf,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Build {
            input,
            output,
            no_cache,
            strict,
        } => {
            if input.is_dir() {
                let out = output
                    .context("site build requires an output directory: build <src-dir> -o <out-dir>")?;
                let opts = BuildOptions { no_cache, strict };
                let report = build_site(&input, &out, &opts)?;
                println!(
                    "built {} page(s) ({} rendered, {} cached), copied {} asset(s) from {} -> {} ({} unresolved link(s))",
                    report.pages.len(),
                    report.rendered.len(),
                    report.skipped.len(),
                    report.assets.len(),
                    input,
                    out,
                    report.broken.len()
                );
            } else {
                let output = output.unwrap_or_else(|| input.with_extension("html"));
                build_file(&input, &output)?;
                println!("built {} -> {}", input, output);
            }
            Ok(())
        }
        // Watch is intentionally a minimal poll loop, not an OS file-watch (spec §5 Phase
        // 6 lists `watch`; the real fs-notify integration is deferred). It rebuilds
        // incrementally whenever any source file's mtime advances.
        Command::Watch { input, output } => watch(&input, &output),
        Command::Clean { output } => {
            if output.exists() {
                fs::remove_dir_all(&output)
                    .with_context(|| format!("removing output directory {output}"))?;
                println!("removed {output}");
            } else {
                println!("nothing to clean: {output} does not exist");
            }
            Ok(())
        }
    }
}

/// Minimal poll-based watch loop: rebuild incrementally whenever a source file changes.
/// Not an OS file-watcher (deferred); it snapshots source mtimes every 500ms.
fn watch(input: &Utf8Path, output: &Utf8Path) -> Result<()> {
    use std::time::{Duration, SystemTime};

    if !input.is_dir() {
        anyhow::bail!("watch requires a source directory: watch <src-dir> -o <out-dir>");
    }
    let opts = BuildOptions::default();

    let snapshot = |root: &Utf8Path| -> Vec<(Utf8PathBuf, SystemTime)> {
        let mut v = Vec::new();
        for entry in walkdir::WalkDir::new(root).sort_by_file_name() {
            let Ok(entry) = entry else { continue };
            if !entry.file_type().is_file() {
                continue;
            }
            if let (Ok(path), Ok(meta)) = (
                Utf8PathBuf::from_path_buf(entry.path().to_owned()),
                entry.metadata(),
            ) {
                let mtime = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
                v.push((path, mtime));
            }
        }
        v
    };

    let report = build_site(input, output, &opts)?;
    println!(
        "watching {input} -> {output}: built {} page(s) ({} rendered). Ctrl-C to stop.",
        report.pages.len(),
        report.rendered.len()
    );
    let mut last = snapshot(input);
    loop {
        std::thread::sleep(Duration::from_millis(500));
        let now = snapshot(input);
        if now != last {
            match build_site(input, output, &opts) {
                Ok(report) => println!(
                    "rebuilt: {} rendered, {} cached",
                    report.rendered.len(),
                    report.skipped.len()
                ),
                Err(e) => eprintln!("build error: {e:#}"),
            }
            last = now;
        }
    }
}

/// Single-file build: read → PARSE → RENDER → TEMPLATE → write. No cross-file link
/// resolution (there is no corpus to resolve against); links keep their best-effort
/// URLs. Whole-site link resolution lives in [`build_site`]. The syntax stylesheet is
/// written alongside the page, since highlighting emits CSS classes.
fn build_file(input: &Utf8Path, output: &Utf8Path) -> Result<()> {
    let source = fs::read_to_string(input)
        .with_context(|| format!("reading source file {input}"))?;
    let document = parse(input, &source).with_context(|| format!("parsing {input}"))?;

    let title = document
        .keywords
        .entries
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("TITLE"))
        .map(|(_, v)| v.clone())
        .unwrap_or_else(|| input.file_stem().unwrap_or("untitled").to_string());

    let resolved = ResolvedDoc { document };
    let highlighter = SyntectHighlighter::new();
    let Html(fragment) = render(&resolved, &highlighter);

    let templater = Templater::new();
    let page = templater
        .render_page(&title, &fragment, &[], SYNTAX_STYLESHEET)
        .with_context(|| format!("templating {input}"))?;
    fs::write(output, page).with_context(|| format!("writing output file {output}"))?;

    let css = output.with_file_name(SYNTAX_STYLESHEET);
    fs::write(&css, syntax_css()).with_context(|| format!("writing stylesheet {css}"))?;
    Ok(())
}
