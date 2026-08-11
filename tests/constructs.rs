//! Golden-file coverage of the v1 scope line (README §"v1 scope").
//!
//! Two halves, and the second is the point:
//!
//! - **IN** — every construct the v1 scope claims gets an element-tree snapshot (parser
//!   correctness) and a rendered-HTML snapshot (renderer correctness).
//! - **OUT** — every construct the v1 scope explicitly excludes gets an assertion that it
//!   *degrades predictably*: parsed and ignored, content preserved where that is the
//!   honest fallback, never a crash and never a half-rendered artifact.
//!
//! The OUT half is the scope guardrail: it is what defends against this project's stated
//! #1 risk, creeping back toward all-of-org.

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
    // A stable relative path keeps snapshots free of absolute machine paths.
    parse(Utf8PathBuf::from("fixtures").join(name).as_path(), &source).expect("parse fixture")
}

fn render_fixture(name: &str) -> String {
    let document = parse_fixture(name);
    let Html(html) = render(&ResolvedDoc { document }, &SyntectHighlighter::new());
    html
}

// ---------------------------------------------------------------------------
// IN: the constructs v1 promises to handle
// ---------------------------------------------------------------------------

#[test]
fn headings_element_tree() {
    insta::assert_json_snapshot!(parse_fixture("headings.org").root);
}

#[test]
fn headings_html() {
    insta::assert_snapshot!(render_fixture("headings.org"));
}

#[test]
fn lists_element_tree() {
    insta::assert_json_snapshot!(parse_fixture("lists.org").root);
}

#[test]
fn lists_html() {
    insta::assert_snapshot!(render_fixture("lists.org"));
}

#[test]
fn blocks_html() {
    insta::assert_snapshot!(render_fixture("blocks.org"));
}

#[test]
fn timestamps_element_tree() {
    insta::assert_json_snapshot!(parse_fixture("timestamps.org").root);
}

#[test]
fn timestamps_html() {
    insta::assert_snapshot!(render_fixture("timestamps.org"));
}

#[test]
fn images_html() {
    insta::assert_snapshot!(render_fixture("images.org"));
}

/// A TODO keyword is a whole word from the configured set, not a prefix: `TODOs are not
/// a keyword` is a plain title. This is the boundary rule most likely to regress.
#[test]
fn todo_keyword_requires_a_word_boundary() {
    let html = render_fixture("headings.org");
    assert!(
        html.contains("<span class=\"todo TODO\">TODO</span> "),
        "a real TODO keyword is marked up:\n{html}"
    );
    assert!(
        !html.contains("<span class=\"todo TODO\">TODO</span> s are"),
        "`TODOs` must not be split into a keyword plus a title:\n{html}"
    );
}

/// Nesting is by indentation, so a nested list must land *inside* its parent `<li>`.
#[test]
fn nested_list_is_nested_in_the_parent_item() {
    let html = render_fixture("lists.org");
    assert!(
        html.contains("<li>outer item<ul>"),
        "an indented sub-list belongs to the item above it:\n{html}"
    );
}

/// The caption supplies alt text, but an explicit `:alt` must win — emitting both
/// would put two `alt` attributes on one tag.
#[test]
fn explicit_alt_attribute_replaces_the_caption_derived_one() {
    let html = render_fixture("images.org");
    assert!(
        html.contains("<img src=\"cat.jpg\" alt=\"a cat, sitting\" loading=\"lazy\">"),
        "a quoted `:alt` should be the only alt attribute:\n{html}"
    );
    for line in html.lines() {
        assert!(
            line.matches(" alt=").count() <= 1,
            "no tag may carry two alt attributes:\n{line}"
        );
    }
}

/// Keywords in the file preamble are *copied* into the metadata map, not removed from
/// the body. Removing them used to merge the paragraphs either side of a keyword and
/// strand `#+CAPTION:` away from the image below it — content damage from a metadata
/// step, in the one region of a file where every real document has keywords.
#[test]
fn preamble_keywords_do_not_disturb_the_content_around_them() {
    let source = "#+TITLE: T\n\nOne.\n#+SOMEKEY: v\nTwo.\n\n#+CAPTION: shot\n[[file:a.png]]\n";
    let document = parse(Utf8PathBuf::from("t.org").as_path(), source).expect("parse");
    assert!(
        document
            .keywords
            .entries
            .iter()
            .any(|(k, v)| k == "TITLE" && v == "T"),
        "document metadata is still collected"
    );
    let Html(html) = render(&ResolvedDoc { document }, &SyntectHighlighter::new());
    assert!(
        html.contains("<p>One.</p>") && html.contains("<p>Two.</p>"),
        "a keyword between two paragraphs must not merge them:\n{html}"
    );
    assert!(
        html.contains("<figcaption>shot</figcaption>"),
        "a preamble `#+CAPTION:` must still attach to the image below it:\n{html}"
    );
}

/// Highlighting must emit CSS classes, never inline styles, so themes live in the
/// stylesheet (spec §3.2) — and the stylesheet the classes refer to must exist.
#[test]
fn highlighting_emits_classes_not_inline_styles() {
    let html = render_fixture("blocks.org");
    assert!(
        html.contains("<span class=\"storage type function python\">"),
        "python source should be tokenized into classed spans:\n{html}"
    );
    assert!(
        !html.contains("style=\""),
        "highlighting must not emit inline styles:\n{html}"
    );
    assert!(
        org_ssg::render::syntax_css().contains(".storage"),
        "the generated stylesheet must define the emitted classes"
    );
}

/// An unknown language is not an error: the block keeps its content, escaped.
#[test]
fn unknown_source_language_falls_back_to_plain_code() {
    let doc = "#+BEGIN_SRC nosuchlang\n<not markup> & such\n#+END_SRC\n";
    let document = parse(Utf8PathBuf::from("t.org").as_path(), doc).expect("parse");
    let Html(html) = render(&ResolvedDoc { document }, &SyntectHighlighter::new());
    assert_eq!(
        html,
        "<pre><code class=\"language-nosuchlang\">&lt;not markup&gt; &amp; such</code></pre>\n"
    );
}

// ---------------------------------------------------------------------------
// OUT: the constructs v1 explicitly excludes must degrade, not explode
// ---------------------------------------------------------------------------

/// The whole OUT fixture parses and renders. This is the crash gate.
#[test]
fn out_of_scope_fixture_renders_without_crashing() {
    let html = render_fixture("outofscope.org");
    assert!(!html.is_empty(), "an out-of-scope document still renders");
}

#[test]
fn out_of_scope_html() {
    insta::assert_snapshot!(render_fixture("outofscope.org"));
}

/// Babel is never executed and `#+RESULTS:` blocks are never trusted: the source block
/// renders as code, and its stale results do not reach the page.
#[test]
fn babel_is_not_executed_and_results_are_dropped() {
    let html = render_fixture("outofscope.org");
    assert!(
        html.contains("the block renders; :results is never executed"),
        "the source block itself still renders:\n{html}"
    );
    assert!(
        !html.contains("stale output from a previous evaluation"),
        "a `#+RESULTS:` block must not be emitted:\n{html}"
    );
}

/// `#+TBLFM:` is inert: the table renders with the values as written, and the formula
/// is neither evaluated nor printed.
#[test]
fn table_formulas_are_inert() {
    let html = render_fixture("outofscope.org");
    assert!(html.contains("<table>"), "the table still renders:\n{html}");
    assert!(
        !html.contains("vsum"),
        "the `#+TBLFM:` formula must not reach the page:\n{html}"
    );
}

/// LaTeX, macros and radio targets have no v1 semantics, so they survive as the literal
/// text the author typed — lossless, and obviously unhandled to a reader.
#[test]
fn latex_macros_and_radio_targets_stay_literal() {
    let html = render_fixture("outofscope.org");
    for literal in ["$x^2 + y^2$", "E = mc^2", "{{{author}}}", "\\alpha"] {
        assert!(
            html.contains(literal),
            "`{literal}` should survive as literal text:\n{html}"
        );
    }
}

/// Drawers other than PROPERTIES are captured by the parser and dropped by the
/// renderer — including LOGBOOK clock lines, which are agenda state, not content.
#[test]
fn drawers_are_parsed_and_dropped() {
    let html = render_fixture("outofscope.org");
    assert!(
        !html.contains("CLOCK:"),
        "LOGBOOK contents must not be emitted:\n{html}"
    );
    assert!(
        !html.contains("Drawer contents are captured and dropped"),
        "generic drawer contents must not be emitted:\n{html}"
    );
}

/// A non-HTML export block is dropped whole: emitting LaTeX into an HTML page would be
/// worse than emitting nothing.
#[test]
fn non_html_export_blocks_are_dropped() {
    let html = render_fixture("blocks.org");
    assert!(
        html.contains("<aside class=\"raw\">Raw HTML passes through.</aside>"),
        "an `html` export block passes through verbatim:\n{html}"
    );
    assert!(
        !html.contains("\\emph"),
        "a `latex` export block must be dropped:\n{html}"
    );
}

/// An unmodelled block type keeps its content rather than vanishing.
#[test]
fn unknown_block_types_keep_their_content() {
    let html = render_fixture("outofscope.org");
    assert!(
        html.contains("An unmodelled block type"),
        "a verse block degrades to a verbatim example block:\n{html}"
    );
}

/// `#+INCLUDE:` is not expanded — the build must not silently pull in another file.
#[test]
fn include_is_not_expanded() {
    let doc = parse_fixture("outofscope.org");
    assert!(
        doc.keywords
            .entries
            .iter()
            .any(|(k, _)| k.eq_ignore_ascii_case("INCLUDE")),
        "`#+INCLUDE:` is captured as an inert keyword"
    );
    let html = render_fixture("outofscope.org");
    assert!(
        !html.contains("other.org"),
        "`#+INCLUDE:` must not be expanded or echoed:\n{html}"
    );
}
