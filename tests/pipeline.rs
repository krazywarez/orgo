//! End-to-end PARSE → RENDER snapshots for the v0.1 core org subset (spec §5).
//!
//! Two layers, per the spec's testing philosophy: an element-tree JSON snapshot
//! (parser correctness) and a rendered-HTML snapshot (renderer correctness), for
//! each fixture.

use camino::Utf8PathBuf;

use org_ssg::model::Document;
use org_ssg::parser::parse;
use org_ssg::render::{render, Html, SyntectHighlighter};
use org_ssg::resolve::ResolvedDoc;

fn parse_fixture(name: &str) -> Document {
    let path = Utf8PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join(name);
    let source = std::fs::read_to_string(&path).expect("read fixture");
    // Use a stable relative path so snapshots don't embed an absolute machine path.
    parse(Utf8PathBuf::from("fixtures").join(name).as_path(), &source).expect("parse fixture")
}

fn render_fixture(name: &str) -> String {
    let document = parse_fixture(name);
    let resolved = ResolvedDoc { document };
    let Html(html) = render(&resolved, &SyntectHighlighter);
    html
}

#[test]
fn minimal_element_tree() {
    insta::assert_json_snapshot!(parse_fixture("minimal.org").root);
}

#[test]
fn minimal_html() {
    insta::assert_snapshot!(render_fixture("minimal.org"));
}

#[test]
fn core_element_tree() {
    insta::assert_json_snapshot!(parse_fixture("core.org").root);
}

#[test]
fn core_html() {
    insta::assert_snapshot!(render_fixture("core.org"));
}
