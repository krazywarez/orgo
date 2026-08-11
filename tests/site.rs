//! Multi-file site build + new-construct snapshots for v0.2 (spec §5, Phase 4/5).
//!
//! Layers, per the spec's testing philosophy: rendered-HTML snapshots (resolved links,
//! templated pages, tables, footnotes) plus explicit assertions that a cross-file link
//! resolves to the right URL and that unresolved links are reported, not fatal.

use camino::Utf8PathBuf;

use org_ssg::index::{SymbolTable, TargetId};
use org_ssg::parser::parse;
use org_ssg::render::{render, Html, SyntectHighlighter};
use org_ssg::resolve::{resolve, ResolvedDoc};
use org_ssg::site::{render_site, BuiltPage};

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
