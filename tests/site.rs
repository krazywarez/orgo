//! Multi-file site build + new-construct snapshots for v0.2 (spec §5, Phase 4/5).
//!
//! Layers, per the spec's testing philosophy: rendered-HTML snapshots (resolved links,
//! templated pages, tables, footnotes) plus explicit assertions that a cross-file link
//! resolves to the right URL and that unresolved links are reported, not fatal.

use camino::Utf8PathBuf;

use orgo::index::{SymbolTable, TargetId};
use orgo::parser::parse;
use orgo::render::{render, Html, SyntectHighlighter};
use orgo::resolve::{resolve, ResolvedDoc};
use orgo::site::{render_site, BuiltPage};

fn fixtures() -> Utf8PathBuf {
    Utf8PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures")
}

fn build_fixture_site() -> Vec<BuiltPage> {
    let (pages, broken) = render_site(&fixtures().join("site")).expect("build site");
    assert!(broken.is_empty(), "fixture site has no broken links: {broken:?}");
    pages
}

fn page<'a>(pages: &'a [BuiltPage], source: &str) -> &'a BuiltPage {
    pages
        .iter()
        .find(|p| p.source == source)
        .unwrap_or_else(|| panic!("no page for {source}"))
}

fn render_fragment(name: &str) -> String {
    let path = fixtures().join(name);
    let source = std::fs::read_to_string(&path).expect("read fixture");
    let document = parse(Utf8PathBuf::from(name).as_path(), &source).expect("parse");
    let Html(html) = render(&ResolvedDoc { document }, &SyntectHighlighter::new());
    html
}

#[test]
fn site_index_html() {
    let pages = build_fixture_site();
    insta::assert_snapshot!(page(&pages, "index.org").html);
}

#[test]
fn site_guide_html() {
    let pages = build_fixture_site();
    insta::assert_snapshot!(page(&pages, "guide.org").html);
}

/// The invariant the whole RESOLVE stage exists for: a cross-file `[[#setup]]` link on
/// the home page must resolve to the guide page's URL plus the target anchor.
#[test]
fn cross_file_link_resolves() {
    let pages = build_fixture_site();
    let index = &page(&pages, "index.org").html;
    assert!(
        index.contains("href=\"guide.html#setup\""),
        "cross-file custom-id link should resolve to guide.html#setup, got:\n{index}"
    );
    // The `file:` link resolves to the bare output path.
    assert!(
        index.contains("href=\"guide.html\""),
        "file: link should resolve to guide.html"
    );
    // A same-page `[[*Overview]]` link stays a local fragment.
    assert!(
        index.contains("href=\"#overview\""),
        "same-page heading link should be a local fragment"
    );
}

/// Unresolved internal links are reported as warnings, never a crash (spec §4.3.4).
#[test]
fn unresolved_link_is_reported_not_fatal() {
    let source = "Broken [[#does-not-exist][link]] here.\n";
    let doc = parse(Utf8PathBuf::from("orphan.org").as_path(), source).expect("parse");
    let mut symbols = SymbolTable::new();
    symbols.index_document(&doc);
    let out = resolve(&doc, &symbols);
    assert_eq!(out.broken.len(), 1, "one unresolved link expected");
    assert_eq!(
        out.broken[0].target,
        TargetId::CustomId("does-not-exist".into())
    );
    assert!(out.used_targets.is_empty(), "nothing resolved, nothing used");
}

/// RESOLVE records the `uses` edges (spec §4.3, R2) even though incrementality does
/// not consume them yet.
#[test]
fn resolve_records_used_targets() {
    let pages = build_fixture_site();
    // index.org uses: guide's #setup, guide.org (file), and its own *Overview → 3.
    let _ = pages; // pages already assert no broken links; check the edge count directly.
    let src = fixtures().join("site").join("index.org");
    let source = std::fs::read_to_string(&src).unwrap();
    let doc = parse(Utf8PathBuf::from("index.org").as_path(), &source).unwrap();

    let guide_src = fixtures().join("site").join("guide.org");
    let guide = parse(
        Utf8PathBuf::from("guide.org").as_path(),
        &std::fs::read_to_string(&guide_src).unwrap(),
    )
    .unwrap();

    let mut symbols = SymbolTable::new();
    symbols.index_document(&doc);
    symbols.index_document(&guide);
    let out = resolve(&doc, &symbols);
    assert_eq!(out.used_targets.len(), 3, "three internal links resolved");
    assert!(out.broken.is_empty());
}

#[test]
fn table_render() {
    insta::assert_snapshot!(render_fragment("table.org"));
}

#[test]
fn footnote_render() {
    insta::assert_snapshot!(render_fragment("footnote.org"));
}

// ---------------------------------------------------------------------------
// `#+SLUG:` output paths (Phase 0 corpus-audit finding)
// ---------------------------------------------------------------------------

/// The audit found `#+SLUG:` in 178 of the target corpus's 179 files, and the live site
/// derives every URL from it — `2018-11-28-aes-encryption.org` publishes as
/// `aes-encryption.html`. Deriving output paths from source filenames would therefore
/// have rewritten every URL on the site.
#[test]
fn slug_renames_the_output_page() {
    let (pages, broken) = render_site(&fixtures().join("slugsite")).expect("build site");
    assert!(broken.is_empty(), "fixture site has no broken links: {broken:?}");
    let post = pages
        .iter()
        .find(|p| p.source == "2024-02-11-long-source-name.org")
        .expect("post page");
    assert_eq!(
        post.output, "short-url.html",
        "the slug names the output file, not the source stem"
    );
}

/// A link's URL has to follow the target's slug. If resolution kept using source paths,
/// every cross-page link would point at a file that was never written.
#[test]
fn links_resolve_through_the_slug() {
    let (pages, _) = render_site(&fixtures().join("slugsite")).expect("build site");
    let index = &page(&pages, "index.org").html;
    assert!(
        index.contains("href=\"short-url.html\""),
        "a file: link must target the slugged page:\n{index}"
    );
    assert!(
        index.contains("href=\"short-url.html#setup\""),
        "a custom-id link must target the slugged page plus the anchor:\n{index}"
    );
    assert!(
        !index.contains("long-source-name"),
        "no URL may mention the source filename:\n{index}"
    );
}

/// A slug is author-controlled text that becomes a path we write to, so traversal has to
/// be impossible by construction rather than by convention.
#[test]
fn slugs_cannot_escape_the_output_directory() {
    use orgo::model::Keywords;
    let source = Utf8PathBuf::from("blog/post.org");
    let slugged = |value: &str| {
        let keywords = Keywords {
            entries: vec![("SLUG".to_string(), value.to_string())],
        };
        orgo::util::output_path(&source, &keywords).to_string()
    };
    assert_eq!(slugged("../../etc/passwd"), "blog/etc-passwd.html");
    assert_eq!(slugged("/absolute"), "blog/absolute.html");
    assert_eq!(slugged(".hidden"), "blog/hidden.html");
    assert_eq!(slugged("Mixed Case Slug"), "blog/mixed-case-slug.html");
    // An empty or punctuation-only slug falls back to the source stem rather than
    // producing `.html` with no name at all.
    assert_eq!(slugged("///"), "blog/post.html");
}

/// Two pages claiming one URL silently drops a page. With slugs that is a typo away and
/// invisible in the source filenames, so the build refuses rather than picking a winner.
#[test]
fn colliding_slugs_are_a_build_error() {
    let dir = std::env::temp_dir().join(format!("orgo-slug-{}", std::process::id()));
    let dir = Utf8PathBuf::from_path_buf(dir).expect("utf-8 temp dir");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("a.org"), "#+TITLE: A\n#+SLUG: same\n").unwrap();
    std::fs::write(dir.join("b.org"), "#+TITLE: B\n#+SLUG: same\n").unwrap();

    let err = render_site(&dir).expect_err("colliding slugs must fail the build");
    let message = format!("{err:#}");
    assert!(
        message.contains("collision") && message.contains("same.html"),
        "the error must name the collision: {message}"
    );
    std::fs::remove_dir_all(&dir).unwrap();
}
