//! CLI entry point (spec §3.5): `build`, `watch`, `clean`.

use std::fs;

use anyhow::{Context, Result};
use camino::{Utf8Path, Utf8PathBuf};
use clap::{Parser, Subcommand};

use orgo::parser::parse;
use orgo::config::{self, Config};
use orgo::render::{self, render, Html, SyntectHighlighter};
use orgo::resolve::ResolvedDoc;
use orgo::site::{build_site, BuildOptions, SYNTAX_STYLESHEET};
use orgo::template::{PageContext, RenderContext, SiteContext, Templater};

#[derive(Parser)]
#[command(name = "orgo", version, about = "Org-mode static site generator")]
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
        /// Treat broken links and parse diagnostics as errors (spec §4.3.4).
        #[arg(long)]
        strict: bool,
        /// Config file to use, overriding `orgo.toml` in the source directory.
        #[arg(long, value_name = "FILE")]
        config: Option<Utf8PathBuf>,
        /// Include pages marked `#+DRAFT:`.
        #[arg(long)]
        drafts: bool,
    },
    /// Watch a source directory and rebuild incrementally on change, driven by OS
    /// filesystem events.
    Watch {
        /// Source directory to watch.
        input: Utf8PathBuf,
        /// Output directory.
        #[arg(short, long)]
        output: Utf8PathBuf,
        /// Bypass the incremental cache on every rebuild.
        #[arg(long)]
        no_cache: bool,
        /// Treat broken links and parse diagnostics as errors.
        #[arg(long)]
        strict: bool,
        /// Config file to use, overriding `orgo.toml` in the source directory.
        #[arg(long, value_name = "FILE")]
        config: Option<Utf8PathBuf>,
        /// Include pages marked `#+DRAFT:`. Handy while writing one.
        #[arg(long)]
        drafts: bool,
    },
    /// Serve the built site locally, rebuilding and reloading the browser on change.
    Serve {
        /// Source directory to build and watch.
        input: Utf8PathBuf,
        /// Output directory to serve.
        #[arg(short, long)]
        output: Utf8PathBuf,
        /// Port to listen on.
        #[arg(short, long, default_value_t = 3000)]
        port: u16,
        /// Address to bind. Defaults to loopback; set `0.0.0.0` to expose the server to
        /// your network, which also exposes any drafts you are building.
        #[arg(long, default_value = "127.0.0.1")]
        host: String,
        /// Include pages marked `#+DRAFT:`.
        #[arg(long)]
        drafts: bool,
        /// Config file to use, overriding `orgo.toml` in the source directory.
        #[arg(long, value_name = "FILE")]
        config: Option<Utf8PathBuf>,
    },
    /// Remove the build output directory (which holds the cache manifest).
    Clean {
        /// Output directory to remove.
        output: Utf8PathBuf,
    },
    /// Audit a corpus: report which org constructs it uses and how they land against
    /// the v1 scope line. Reports names, counts and locations — never document text.
    Audit {
        /// Source directory (or single `.org` file) to audit.
        input: Utf8PathBuf,
    },
    /// Scaffold a new site: config, an editable copy of the default layout, and a page.
    Init {
        /// Directory to create the site in (created if missing; defaults to the
        /// current directory).
        #[arg(default_value = ".")]
        directory: Utf8PathBuf,
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
            config,
            drafts,
        } => {
            if input.is_dir() {
                let out = output
                    .context("site build requires an output directory: build <src-dir> -o <out-dir>")?;
                let opts = BuildOptions {
                    no_cache,
                    strict,
                    config_path: config.clone(),
                    drafts,
                };
                let report = build_site(&input, &out, &opts)?;
                println!(
                    "built {} page(s) ({} rendered, {} cached), copied {} asset(s) from {} -> {} ({} unresolved link(s), {} diagnostic(s))",
                    report.pages.len(),
                    report.rendered.len(),
                    report.skipped.len(),
                    report.assets.len(),
                    input,
                    out,
                    report.broken.len(),
                    report.diagnostics.len()
                );
            } else {
                let output = output.unwrap_or_else(|| input.with_extension("html"));
                build_file(&input, &output)?;
                println!("built {} -> {}", input, output);
            }
            Ok(())
        }
        Command::Watch {
            input,
            output,
            no_cache,
            strict,
            config,
            drafts,
        } => orgo::watch::run(
            &input,
            &output,
            &BuildOptions {
                no_cache,
                strict,
                config_path: config,
                drafts,
            },
        ),
        Command::Audit { input } => {
            let result = orgo::audit::audit(&input)?;
            print!("{}", orgo::audit::report(&result));
            Ok(())
        }
        Command::Serve {
            input,
            output,
            port,
            host,
            drafts,
            config,
        } => orgo::serve::run(
            &input,
            &output,
            &BuildOptions {
                drafts,
                config_path: config,
                ..Default::default()
            },
            &host,
            port,
        ),
        Command::Init { directory } => init(&directory),
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

/// Scaffold a working site. Writes only files that do not already exist, so running it
/// in a directory that has content is safe and additive rather than destructive.
fn init(dir: &Utf8Path) -> Result<()> {
    use orgo::config::{CONFIG_FILE, STARTER_CONFIG};
    use orgo::template::{
        starter_template, STARTER_FEED_TEMPLATE, STARTER_LIST_TEMPLATE, STARTER_TAGS_TEMPLATE,
    };

    fs::create_dir_all(dir).with_context(|| format!("creating {dir}"))?;
    fs::create_dir_all(dir.join("templates")).with_context(|| format!("creating {dir}/templates"))?;
    fs::create_dir_all(dir.join("blog")).with_context(|| format!("creating {dir}/blog"))?;

    let index = concat!(
        "#+TITLE: Hello\n",
        "#+DATE: today\n",
        "\n",
        "Welcome to your new site. Edit this file, then run the build again.\n",
        "\n",
        "* A heading\n",
        "\n",
        "Org markup works as you would expect: *bold*, /italic/, ~code~, and\n",
        "[[https://orgmode.org][links]].\n",
        "\n",
        "#+BEGIN_SRC rust\n",
        "fn main() {\n",
        "    println!(\"syntax highlighting is on by default\");\n",
        "}\n",
        "#+END_SRC\n",
    );

    let post = concat!(
        "#+TITLE: A first post\n",
        "#+DATE: <2026-01-15 Thu>\n",
        "#+FILETAGS: :example:\n",
        "\n",
        "Posts in this directory are collected into /blog/ by the [[collections]] block\n",
        "in orgo.toml, newest first.\n",
    );

    let files: [(Utf8PathBuf, &str); 7] = [
        (dir.join(CONFIG_FILE), STARTER_CONFIG),
        (dir.join("templates/base.html"), starter_template()),
        (dir.join("templates/list.html"), STARTER_LIST_TEMPLATE),
        (dir.join("templates/tags.html"), STARTER_TAGS_TEMPLATE),
        (dir.join("templates/feed.xml"), STARTER_FEED_TEMPLATE),
        (dir.join("index.org"), index),
        (dir.join("blog/first-post.org"), post),
    ];

    let mut created = Vec::new();
    for (path, contents) in &files {
        if path.exists() {
            println!("kept existing {path}");
            continue;
        }
        fs::write(path, contents).with_context(|| format!("writing {path}"))?;
        created.push(path.clone());
    }

    for path in &created {
        println!("created {path}");
    }
    println!("\nNext: orgo build {dir} -o _site");
    Ok(())
}

/// Single-file build: read → PARSE → RENDER → TEMPLATE → write. No cross-file link
/// resolution (there is no corpus to resolve against); links keep their best-effort
/// URLs. Whole-site link resolution lives in [`build_site`]. The syntax stylesheet is
/// written alongside the page, since highlighting emits CSS classes.
fn build_file(input: &Utf8Path, output: &Utf8Path) -> Result<()> {
    let source = fs::read_to_string(input)
        .with_context(|| format!("reading source file {input}"))?;
    let document = parse(input, &source).with_context(|| format!("parsing {input}"))?;
    for d in &document.diagnostics {
        eprintln!("warning: {input}:{}: {}", d.line, d.message);
    }

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

    // A single-file build still honours a config beside the source, so `build one.org`
    // and a whole-site build produce the same-looking page.
    let dir = input.parent().unwrap_or_else(|| Utf8Path::new("."));
    let config = Config::load(dir)?;
    config.validate()?;
    let templater = Templater::load(Some(&dir.join(&config.templates.dir)), &config.site.base_url)?;
    let css_text = render::syntax_css(&config.highlight.theme).ok_or_else(|| {
        anyhow::anyhow!(
            "unknown highlight.theme {:?}. Available: {}",
            config.highlight.theme,
            render::available_themes().join(", ")
        )
    })?;

    let site = SiteContext {
        title: config.site.title.clone(),
        base_url: config.site.base_url.clone(),
        description: config.site.description.clone(),
        language: config.site.language.clone(),
    };
    let page_ctx = PageContext {
        title: title.clone(),
        url: output.file_name().unwrap_or("index.html").to_string(),
        source: input.to_string(),
        date: None,
        date_iso: None,
        year: None,
        tags: Vec::new(),
        excerpt: String::new(),
        content: None,
        word_count: 0,
        reading_time: 0,
        keywords: Default::default(),
        toc: orgo::util::table_of_contents(&resolved.document.root),
    };
    let mut ctx = RenderContext::new(&site, &page_ctx, &[], SYNTAX_STYLESHEET, "");
    ctx.body = &fragment;
    // `#+TEMPLATE:` and `[[pages]]` apply here too, so `build one.org` and a whole-site
    // build put the same page through the same layout.
    let name = config::page_template(
        &config,
        Utf8Path::new(input.file_name().unwrap_or_default()),
        &resolved.document.keywords,
    );
    let page = templater
        .render(&name, &ctx)
        .with_context(|| format!("templating {input} through {name}"))?;
    fs::write(output, page).with_context(|| format!("writing output file {output}"))?;

    let css = output.with_file_name(SYNTAX_STYLESHEET);
    fs::write(&css, css_text).with_context(|| format!("writing stylesheet {css}"))?;
    Ok(())
}
