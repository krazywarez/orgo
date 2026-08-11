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
        html.contains("<figcaption><span class=\"figure-number\">Figure 1: </span>shot</figcaption>"),
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
    for literal in ["$x^2 + y^2$", "E = mc^2", "{{{author}}}", "\\notanentity"] {
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

/// A verse block keeps its line breaks, and a block with an unrecognised name becomes a
/// div holding parsed org — the two ways a `#+BEGIN_` other than SRC/EXAMPLE/QUOTE/CENTER
/// can carry content.
#[test]
fn verse_and_special_blocks_keep_their_content() {
    let html = render_fixture("blocks.org");
    assert!(
        html.contains("<p class=\"verse\">") && html.contains("Line breaks are the point<br>"),
        "verse keeps its breaks:\n{html}"
    );
    assert!(
        html.contains("<div class=\"note\">") && html.contains("<strong>org</strong>"),
        "a special block holds parsed org:\n{html}"
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
    ] {
        let document = parse_fixture(name);
        assert!(
            document.diagnostics.is_empty(),
            "{name} should parse cleanly, got {:?}",
            document.diagnostics
        );
    }

    // The out-of-scope fixture is the exception, and only for the one construct that is
    // *meant* to announce itself: an unexpanded `#+INCLUDE:` means content is missing
    // from the page, which is worth a line in the build output.
    let out = parse_fixture("outofscope.org");
    let messages: Vec<&str> = out.diagnostics.iter().map(|d| d.message.as_str()).collect();
    assert_eq!(messages.len(), 1, "one diagnostic, not a pile: {messages:?}");
    assert!(
        messages[0].contains("#+INCLUDE:") && messages[0].contains("not expanded"),
        "and it is the include: {messages:?}"
    );
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

// ---------------------------------------------------------------------------
// Export-time text conversions
// ---------------------------------------------------------------------------

fn html_of(source: &str) -> String {
    let document = parse(Utf8PathBuf::from("t.org").as_path(), source).expect("parse");
    let Html(html) = render(&ResolvedDoc { document }, &SyntectHighlighter::new());
    html
}

/// Org converts dash runs and ellipses in prose. A reader of the published page should
/// see the typography the author meant, not the ASCII they had to type.
#[test]
fn special_strings_become_real_punctuation() {
    let html = html_of("An em---dash, an en--dash, and an ellipsis...\n");
    assert!(html.contains("em\u{2014}dash"), "em dash:\n{html}");
    assert!(html.contains("en\u{2013}dash"), "en dash:\n{html}");
    assert!(html.contains("ellipsis\u{2026}"), "ellipsis:\n{html}");
}

/// A shell transcript is not prose. `--verbose` inside code has to survive intact, or
/// copying a command off the page produces one that does not run.
#[test]
fn special_strings_leave_code_alone() {
    let html = html_of(
        "Prose --dash and ~ls --all~ and =grep --color=.\n\n\
         #+BEGIN_SRC sh\nls --all\n#+END_SRC\n",
    );
    assert!(html.contains("Prose \u{2013}dash"), "prose converts:\n{html}");
    assert!(html.contains("<code>ls --all</code>"), "inline code:\n{html}");
    assert!(html.contains("--color"), "verbatim:\n{html}");
    // The source block's `--all` is split across highlighting spans, so count the
    // conversion itself: exactly one en dash on the page, the one in the prose.
    assert_eq!(
        html.matches('\u{2013}').count(),
        1,
        "nothing inside code converted:\n{html}"
    );
}

/// `#+OPTIONS: -:nil` is how a document opts out, and org-ssg honours org's own switch
/// rather than inventing one.
#[test]
fn a_document_can_turn_special_strings_off() {
    let html = html_of("#+OPTIONS: -:nil\n\nAn em---dash and an ellipsis...\n");
    assert!(html.contains("em---dash"), "left alone:\n{html}");
    assert!(html.contains("ellipsis..."), "left alone:\n{html}");
}

/// Sub- and superscripts, including the braceless form — which is what makes
/// `snake_case` in prose render as a subscript, exactly as Emacs does with the same file.
#[test]
fn sub_and_superscripts_convert() {
    let html = html_of("Water is H_2O, x^2 is a square, and sshd_{config}.d is a path.\n");
    assert!(html.contains("H<sub>2O</sub>"), "braceless subscript:\n{html}");
    assert!(html.contains("x<sup>2</sup>"), "superscript:\n{html}");
    assert!(html.contains("sshd<sub>config</sub>.d"), "braced:\n{html}");
}

/// `_underlined_` is emphasis, not a subscript. The two are told apart by what comes
/// *before* the marker: emphasis follows whitespace, a script follows a word.
#[test]
fn underline_still_wins_where_org_says_it_does() {
    let html = html_of("Some _underlined_ text.\n");
    assert!(html.contains("<u>underlined</u>"), "underline:\n{html}");
    assert!(!html.contains("<sub>"), "not a subscript:\n{html}");
}

/// `#+OPTIONS: ^:nil` turns them off, `^:{}` limits them to the braced form.
#[test]
fn a_document_can_restrict_sub_and_superscripts() {
    let off = html_of("#+OPTIONS: ^:nil\n\nH_2O and x^2.\n");
    assert!(off.contains("H_2O") && off.contains("x^2"), "off:\n{off}");

    let braces = html_of("#+OPTIONS: ^:{}\n\nH_2O and a_{b}.\n");
    assert!(braces.contains("H_2O"), "braceless left alone:\n{braces}");
    assert!(braces.contains("a<sub>b</sub>"), "braced converts:\n{braces}");
}

/// LaTeX is passed through for a typesetter, so the text conversions must not reach
/// inside it: `x^2` in `$…$` is mathematics, not markup.
#[test]
fn latex_fragments_are_left_intact() {
    let html = html_of("Inline $x^2 + y^2$ and \\(a_1\\) and \\[E = mc^2\\] stay put.\n");
    for literal in ["$x^2 + y^2$", "\\(a_1\\)", "\\[E = mc^2\\]"] {
        assert!(html.contains(literal), "`{literal}` survives:\n{html}");
    }
}

/// A dollar amount is not a formula. The body of a `$…$` fragment may not begin or end
/// with a space, which is what keeps prices out of the math.
#[test]
fn dollar_amounts_are_not_latex() {
    let html = html_of("It cost $5 or $6 --- a bargain.\n");
    assert!(html.contains("\u{2014}"), "the em dash still converts:\n{html}");
}

/// Org exports outline levels relative to the file's own shallowest heading, so a
/// document written entirely under `**` is a document of top-level sections.
#[test]
fn heading_levels_are_relative_to_the_shallowest_heading() {
    let html = html_of("#+TITLE: T\n\n** First\n\nBody.\n\n*** Nested\n\nMore.\n");
    assert!(html.contains("<h2 id=\"first\">"), "** becomes h2:\n{html}");
    assert!(html.contains("<h3 id=\"nested\">"), "*** becomes h3:\n{html}");
}

/// An unknown `#+BEGIN_` block is a special block: a div with that name, holding org.
/// Rendering its contents as literal text loses the markup the author wrote.
#[test]
fn a_special_block_holds_org_not_text() {
    let html = html_of("#+BEGIN_NOTE\n*Note:* read this.\n#+END_NOTE\n");
    assert!(html.contains("<div class=\"note\">"), "named div:\n{html}");
    assert!(html.contains("<strong>Note:</strong>"), "markup parsed:\n{html}");
}

/// A path that starts with `~` inside `~…~` verbatim: the body may open with the same
/// character as the marker, and org says so.
#[test]
fn verbatim_can_start_with_its_own_marker() {
    let html = html_of("Edit ~~/.config/doom/config.el~ now.\n");
    assert!(
        html.contains("<code>~/.config/doom/config.el</code>"),
        "the leading ~ belongs to the path:\n{html}"
    );
}

/// Org's special first column holds export markers, not data: `/` marks a column group,
/// `#` a row to recalculate. Publishing them puts a column of punctuation on the page.
#[test]
fn a_tables_special_column_and_marker_rows_are_dropped() {
    let html = html_of(
        "| N | N^2 |\n\
         | / |   < |\n\
         | 1 |   1 |\n",
    );
    assert!(!html.contains("<td>/</td>"), "the marker row is gone:\n{html}");
    assert!(html.contains("<td>1</td>"), "the data row stays:\n{html}");

    // Every row marked, so the column itself goes too.
    let all_marked = html_of(
        "| # | exp(x) | 1 |\n\
         | # | exp(x) | 2 |\n",
    );
    assert!(
        !all_marked.contains(">#<"),
        "a wholly-special column is dropped:\n{all_marked}"
    );
    assert!(all_marked.contains("exp(x)"), "data survives:\n{all_marked}");
}

/// An affiliated keyword belongs to the element *immediately* below it. Someone who
/// writes `#+CAPTION:` under their image has captioned nothing — and captioning the next
/// image instead would put the wrong words under the wrong picture.
#[test]
fn a_blank_line_ends_a_captions_association() {
    let html = html_of(
        "[[file:one.png]]\n\
         #+CAPTION: stranded\n\
         \n\
         [[file:two.png]]\n",
    );
    assert!(
        !html.contains("stranded"),
        "an orphaned caption attaches to nothing:\n{html}"
    );

    let attached = html_of("#+CAPTION: attached\n[[file:one.png]]\n");
    assert!(
        attached.contains("<figcaption>"),
        "a caption directly above its image still works:\n{attached}"
    );
}

/// Org's entity table, taken from Emacs' own `org-entities` so the mapping is not a
/// hand-typed approximation of 400 entries.
#[test]
fn entities_become_their_characters() {
    let html = html_of("Greek \\alpha and \\beta{}s, an arrow \\rarr, and 20\\deg today.\n");
    assert!(html.contains("&alpha;"), "alpha:\n{html}");
    // `{}` is the explicit terminator and must not survive into the text.
    assert!(html.contains("&beta;s"), "beta with {{}}:\n{html}");
    assert!(html.contains("&rarr;"), "arrow:\n{html}");
    assert!(html.contains("20&deg;"), "degree:\n{html}");
}

/// A name org does not know is a typo, and a typo should look like one rather than
/// disappear. `\alphabet` is not a Greek letter followed by "bet", either.
#[test]
fn unknown_entities_and_longer_words_stay_literal() {
    let html = html_of("Neither \\notanentity nor \\alphabet is an entity.\n");
    assert!(html.contains("\\notanentity"), "unknown stays:\n{html}");
    assert!(html.contains("\\alphabet"), "no prefix match:\n{html}");
}

/// `#+OPTIONS: e:nil` is org's own switch for turning entities off.
#[test]
fn a_document_can_turn_entities_off() {
    let html = html_of("#+OPTIONS: e:nil\n\nGreek \\alpha stays.\n");
    assert!(html.contains("\\alpha"), "left alone:\n{html}");
    assert!(!html.contains("&alpha;"), "not converted:\n{html}");
}

/// A caption above a table becomes a numbered `<caption>`, as it does for figures.
#[test]
fn tables_take_a_numbered_caption() {
    let html = html_of(
        "#+CAPTION: Quarterly figures\n| Q | Rev |\n\n\
         #+CAPTION: Second table\n| A | B |\n",
    );
    assert!(
        html.contains("<caption><span class=\"table-number\">Table 1: </span>Quarterly figures"),
        "first table:\n{html}"
    );
    assert!(html.contains("Table 2: </span>Second table"), "second:\n{html}");
}

/// `#+INCLUDE:` is not expanded, and says so. Silently dropping it publishes a page with
/// content missing and nobody told.
#[test]
fn an_unexpanded_include_reports_itself() {
    let doc = parse(
        Utf8PathBuf::from("t.org").as_path(),
        "#+TITLE: T\n\n#+INCLUDE: \"other.org\" :lines \"5-10\"\n\nBody.\n",
    )
    .expect("parse");
    assert_eq!(doc.diagnostics.len(), 1, "{:?}", doc.diagnostics);
    assert_eq!(doc.diagnostics[0].line, 3, "the line it is on");
    assert!(
        doc.diagnostics[0].message.contains("other.org"),
        "names the file: {:?}",
        doc.diagnostics[0]
    );
}

/// A back-link whose whole visible content is `↩` has that glyph as its whole accessible
/// name, so a screen reader announces "left arrow with hook" once per note and the reader
/// cannot tell which reference each one returns to.
#[test]
fn footnote_links_and_section_are_labelled() {
    let html = html_of("Text[fn:1] and more[fn:2].\n\n[fn:1] First.\n\n[fn:2] Second.\n");
    assert!(
        html.contains("<section class=\"footnotes\" aria-label=\"Footnotes\">"),
        "the landmark is named:\n{html}"
    );
    assert!(
        html.contains("aria-label=\"Back to reference 1\"")
            && html.contains("aria-label=\"Back to reference 2\""),
        "each back-link says where it goes:\n{html}"
    );
}
