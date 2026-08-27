//! orgo against the shared conformance corpus (`krz/org-conformance`).
//!
//! orgo *generates* that corpus's goldens, so this test is not looking for orgo to be
//! wrong — it is the tripwire that fires when orgo's renderer changes and the checked-in
//! corpus was not regenerated to match. Without it the two drift silently: orgo's own
//! snapshots update with `cargo insta accept`, the corpus does not, and every other
//! language is then conforming to a stale reference.
//!
//! The corpus lives in a sibling repo, so the test is opt-in: point `ORG_CONFORMANCE_DIR`
//! at a checkout and it runs; unset, it skips cleanly, exactly like the Emacs oracle.
//!
//!   ORG_CONFORMANCE_DIR=../org-conformance cargo test --test conformance

use std::path::PathBuf;

use camino::Utf8PathBuf;

use orgo::parser::parse;
use orgo::render::{render, Html, SyntectHighlighter};
use orgo::resolve::ResolvedDoc;
use orgo::skeleton::skeleton;

fn corpus_dir() -> Option<PathBuf> {
    let dir = PathBuf::from(std::env::var_os("ORG_CONFORMANCE_DIR")?);
    dir.join("cases").is_dir().then_some(dir)
}

fn render_skeleton(org_path: &std::path::Path) -> Vec<String> {
    let source = std::fs::read_to_string(org_path).expect("read case .org");
    let rel = Utf8PathBuf::from(org_path.file_name().unwrap().to_string_lossy().into_owned());
    let document = parse(rel.as_path(), &source).expect("parse case");
    let Html(html) = render(&ResolvedDoc { document }, &SyntectHighlighter::new());
    skeleton(&html)
}

#[test]
fn orgo_matches_the_conformance_corpus() {
    let Some(dir) = corpus_dir() else {
        eprintln!("ORG_CONFORMANCE_DIR unset or has no cases/ — skipping conformance check");
        return;
    };
    let cases = dir.join("cases");

    let mut checked = 0;
    let mut mismatches = Vec::new();
    for entry in std::fs::read_dir(&cases).expect("read cases dir") {
        let path = entry.expect("dir entry").path();
        if path.extension().and_then(|e| e.to_str()) != Some("org") {
            continue;
        }
        let name = path.file_stem().unwrap().to_string_lossy().into_owned();
        let golden_path = cases.join(format!("{name}.skeleton"));
        let golden = std::fs::read_to_string(&golden_path)
            .unwrap_or_else(|_| panic!("missing golden skeleton for case {name}"));
        let golden_lines: Vec<&str> = golden.lines().collect();

        let ours = render_skeleton(&path);
        if ours != golden_lines {
            mismatches.push(name);
        }
        checked += 1;
    }

    assert!(checked > 0, "corpus at {} has no .org cases", cases.display());
    assert!(
        mismatches.is_empty(),
        "orgo's skeleton no longer matches the corpus goldens for: {}. \
         If the renderer change is intended, regenerate the corpus \
         (ORGO_DIR=../orgo tools/generate.sh) and commit it.",
        mismatches.join(", ")
    );
    eprintln!("conformance: {checked} cases match the corpus goldens");
}
