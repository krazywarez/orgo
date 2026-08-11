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
        org_ssg::render::syntax_css("InspiredGitHub")
            .expect("a built-in theme")
            .contains(".storage"),
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

// ---------------------------------------------------------------------------
// Parse diagnostics: degrading is fine, degrading *silently* is not
// ---------------------------------------------------------------------------

fn diagnostics(source: &str) -> Vec<String> {
    let document = parse(Utf8PathBuf::from("t.org").as_path(), source).expect("parse");
    document
        .diagnostics
        .iter()
        .map(|d| format!("{}: {}", d.line, d.message))
        .collect()
}

/// An unterminated block swallows the rest of the file. The parser's contract is to
/// degrade rather than crash, so it still returns a document — but a silent one would
/// mean a build that reports success while deleting most of a page.
#[test]
fn unterminated_block_is_reported_with_its_line() {
    let source = "#+TITLE: T\n\nIntro.\n\n* Section\n\n#+BEGIN_SRC rust\nfn main() {}\n\n* Vanishes\n";
    let found = diagnostics(source);
    assert_eq!(found.len(), 1, "exactly one diagnostic: {found:?}");
    assert!(
        found[0].starts_with("7: unterminated `#+BEGIN_SRC` block"),
        "must name the line the block opened on: {found:?}"
    );
}

/// The same failure mode, and quieter: drawers render to nothing, so an unterminated one
/// deletes the rest of the file without even leaving a code block behind.
#[test]
fn unterminated_drawer_is_reported_with_its_line() {
    let found = diagnostics("#+TITLE: T\n\n* Head\n:LOGBOOK:\nCLOCK: x\n\n* Lost\n");
    assert_eq!(found.len(), 1, "exactly one diagnostic: {found:?}");
    assert!(
        found[0].starts_with("4: unterminated `:LOGBOOK:` drawer"),
        "must name the drawer and its line: {found:?}"
    );
}

/// A stray terminator usually means the matching `#+BEGIN_` above it is misspelled.
#[test]
fn stray_block_end_is_reported_with_its_line() {
    let found = diagnostics("#+TITLE: T\n\nText.\n\n#+END_SRC\n\nMore.\n");
    assert_eq!(found.len(), 1, "exactly one diagnostic: {found:?}");
    assert!(
        found[0].starts_with("5: stray `#+END_SRC`"),
        "must name the stray terminator and its line: {found:?}"
    );
}

/// Line numbers must survive nesting. A block inside a list item inside a section is
/// several levels of re-parsed, re-indented, reconstructed lines away from the file, and
/// a diagnostic that points at the wrong line is worse than none.
#[test]
fn diagnostic_lines_survive_nesting() {
    let source = concat!(
        "#+TITLE: T\n",   // 1
        "\n",             // 2
        "* Section\n",    // 3
        "\n",             // 4
        "- an item\n",    // 5
        "\n",             // 6
        "  #+BEGIN_SRC sh\n", // 7
        "  echo hi\n",    // 8
    );
    let found = diagnostics(source);
    assert_eq!(found.len(), 1, "exactly one diagnostic: {found:?}");
    assert!(
        found[0].starts_with("7: unterminated"),
        "the line must be the real file line, not an offset into a nested slice: {found:?}"
    );
}

/// Every fixture that is meant to be well-formed must parse without complaint —
/// otherwise the diagnostics are crying wolf on ordinary documents.
#[test]
fn well_formed_fixtures_produce_no_diagnostics() {
    for name in [
        "minimal.org",
        "core.org",
        "elements.org",
        "table.org",
        "footnote.org",
        "headings.org",
        "lists.org",
        "blocks.org",
        "timestamps.org",
        "images.org",
        "outofscope.org",
    ] {
        let document = parse_fixture(name);
        assert!(
            document.diagnostics.is_empty(),
            "{name} should parse cleanly, got {:?}",
            document.diagnostics
        );
    }
}

// ---------------------------------------------------------------------------
// Bundled syntax definitions, and org's comma escape
// ---------------------------------------------------------------------------

/// syntect bundles neither TOML nor Org. Both are gaps this project hits on its own
/// first documentation page: every config example is TOML, and a tool for org users gets
/// written about in org.
#[test]
fn toml_and_org_blocks_are_highlighted() {
    for (lang, code, expect_scope) in [
        (
            "toml",
            "# comment\n[site]\ntitle = \"x\"\nport = 3000\nok = true\n",
            "entity name section toml",
        ),
        (
            // The heading is comma-escaped, which org *requires* inside a block: an
            // unescaped `*` at column 0 ends the block in Emacs too, verified against it.
            "org",
            ",#+TITLE: A page\n\n,* TODO [#A] Heading  :tag:\n\nSome *bold* text.\n",
            "markup heading org",
        ),
    ] {
        let source = format!("#+BEGIN_SRC {lang}\n{code}#+END_SRC\n");
        let document = parse(Utf8PathBuf::from("t.org").as_path(), &source).expect("parse");
        let Html(html) = render(&ResolvedDoc { document }, &SyntectHighlighter::new());

        assert!(
            html.contains(&format!("class=\"language-{lang} highlight\"")),
            "{lang} should be highlighted, not fall back to plain code:\n{html}"
        );
        assert!(
            html.contains(expect_scope),
            "{lang} should produce the scope {expect_scope:?}:\n{html}"
        );
    }
}

/// TOML's lexical corners: a table array is not a table, a date is not an integer, and a
/// comment is not a table header.
#[test]
fn the_toml_syntax_distinguishes_its_shapes() {
    let code = "#+BEGIN_SRC toml\n# note\n[[collections]]\nwhen = 2026-08-11\nn = 12\ns = \"q\"\nb = false\n#+END_SRC\n";
    let document = parse(Utf8PathBuf::from("t.org").as_path(), code).expect("parse");
    let Html(html) = render(&ResolvedDoc { document }, &SyntectHighlighter::new());

    for scope in [
        "comment line number-sign toml",
        "entity name section toml",
        "constant numeric date toml",
        "string quoted double toml",
        "constant language toml",
    ] {
        assert!(html.contains(scope), "expected scope {scope:?}:\n{html}");
    }
}

/// Org escapes a line inside a block that would look like structure by prefixing a
/// comma, and the exporter removes it. Without this, documentation *about* org shows the
/// escape characters its author had to type — to exactly the audience most likely to
/// notice. Verified against Emacs, which strips them.
#[test]
fn the_comma_escape_is_removed_from_block_content() {
    let source = concat!(
        "#+BEGIN_SRC org\n",
        ",#+TITLE: A page\n",
        ",* A heading\n",
        ",,* not a heading, one comma removed\n",
        "plain line\n",
        "#+END_SRC\n",
    );
    let document = parse(Utf8PathBuf::from("t.org").as_path(), source).expect("parse");
    let Html(html) = render(&ResolvedDoc { document }, &SyntectHighlighter::new());
    // Highlighting splits the line across spans, so compare the text, not the markup.
    let text = strip_tags(&html);

    assert!(text.contains("#+TITLE: A page"), "the comma is gone:\n{html}");
    assert!(!text.contains(",#+TITLE:"), "and not merely moved:\n{html}");
    assert!(
        text.contains(",* not a heading"),
        "a doubled comma loses exactly one:\n{html}"
    );
    assert!(text.contains("plain line"), "other lines are untouched:\n{html}");
}

/// Text content of an HTML fragment, with tags removed and entities decoded.
fn strip_tags(html: &str) -> String {
    let mut text = String::new();
    let mut rest = html;
    while let Some(open) = rest.find('<') {
        text.push_str(&rest[..open]);
        match rest[open..].find('>') {
            Some(close) => rest = &rest[open + close + 1..],
            None => break,
        }
    }
    text.push_str(rest);
    text.replace("&amp;", "&").replace("&lt;", "<").replace("&gt;", ">")
}

/// A comma that is not an escape is content, and must survive.
#[test]
fn an_ordinary_leading_comma_is_not_stripped() {
    let source = "#+BEGIN_SRC text\n, a list continuation\n,not an escape\n#+END_SRC\n";
    let document = parse(Utf8PathBuf::from("t.org").as_path(), source).expect("parse");
    let Html(html) = render(&ResolvedDoc { document }, &SyntectHighlighter::new());
    let text = strip_tags(&html);
    assert!(text.contains(", a list continuation"), "{html}");
    assert!(text.contains(",not an escape"), "{html}");
}
