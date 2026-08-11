//! The `emacs --batch` ground-truth oracle (spec §5, Phase 0).
//!
//! Every other test in this suite checks org-ssg against org-ssg: a snapshot says our
//! output has not *changed*, never that it is *right*. Those two questions are different,
//! and only one of them matters to someone whose site is currently published by Emacs.
//! This file answers the second by exporting the same fixture with org's own HTML
//! exporter — the exporter weblorg wraps to publish the target corpus — and diffing the
//! two.
//!
//! **What is compared.** Byte equality is not a useful goal: org wraps every section in
//! `outline-container` divs keyed by generated ids, and no amount of agreement on
//! semantics would survive that. Both sides are reduced to a *semantic skeleton* — the
//! sequence of element opens, closes, and text runs, with `<div>`s and all attributes
//! except `href`/`src` dropped, whitespace collapsed, and entities decoded. What remains
//! is the question worth asking: does org think this is a `<blockquote><p>`, and do we?
//!
//! **What the result means.** These tests do not assert agreement — they *snapshot the
//! disagreement*. A divergence report that is checked in and reviewed is worth more than
//! a red test nobody can act on, and it makes any new divergence show up as a diff in
//! code review. A few invariants that must never break are asserted outright.
//!
//! The suite skips cleanly when Emacs is absent, so it never blocks a machine or CI
//! runner that has no Emacs.

use std::process::Command;

use camino::Utf8PathBuf;

use org_ssg::parser::parse;
use org_ssg::render::{render, Html, SyntectHighlighter};
use org_ssg::resolve::ResolvedDoc;

fn manifest_dir() -> Utf8PathBuf {
    Utf8PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Is a usable Emacs on PATH? The oracle is a development instrument, not a build
/// dependency, so its absence skips rather than fails.
fn emacs_available() -> bool {
    Command::new("emacs")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Export a fixture with org's own HTML exporter.
fn org_export(fixture: &str) -> String {
    let root = manifest_dir();
    let output = Command::new("emacs")
        .args(["-Q", "--batch", "-l"])
        .arg(root.join("tests/oracle.el"))
        .env("ORG_ORACLE_INPUT", root.join("fixtures").join(fixture))
        .current_dir(&root)
        .output()
        .expect("run emacs");
    assert!(
        output.status.success(),
        "emacs export of {fixture} failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("emacs emits UTF-8")
}

/// Render a fixture with org-ssg.
fn our_export(fixture: &str) -> String {
    let path = manifest_dir().join("fixtures").join(fixture);
    let source = std::fs::read_to_string(&path).expect("read fixture");
    let document = parse(Utf8PathBuf::from(fixture).as_path(), &source).expect("parse");
    let Html(html) = render(&ResolvedDoc { document }, &SyntectHighlighter::new());
    html
}

// ---------------------------------------------------------------------------
// HTML → semantic skeleton
// ---------------------------------------------------------------------------

/// Elements dropped from the skeleton entirely, because once attributes are gone they
/// carry no meaning the two exporters could agree or disagree *about*.
///
/// `div` is pure layout: org wraps every section in `outline-container`/`outline-text`
/// wrappers and we emit none. `span` is the same story at the inline level, and matters
/// far more than it looks: syntect emits one span per code token, so keeping them made a
/// source block contribute ~60 skeleton lines of pure noise and dragged the agreement on
/// `blocks.org` down to 36% — a number that said nothing about whether we render blocks
/// correctly. Text still carries the signal: a `<span class="todo">` shows up as its
/// text, `"TODO"`, which is the part worth comparing.
const IGNORED: &[&str] = &["div", "span"];

/// Attributes kept in the skeleton. Ids and classes are generated (`org6c28c1b`) or
/// cosmetic (`org-ul`); `href` and `src` are the content.
const KEPT_ATTRS: &[&str] = &["href", "src"];

/// HTML void elements, which never emit a close event.
const VOID: &[&str] = &[
    "br", "hr", "img", "input", "meta", "link", "col", "area", "base", "source", "wbr",
];

/// Reduce an HTML fragment to its semantic skeleton: one line per element open, element
/// close, or text run.
fn skeleton(html: &str) -> Vec<String> {
    let mut out = Vec::new();
    let chars: Vec<char> = html.chars().collect();
    let mut i = 0;
    let mut text = String::new();

    while i < chars.len() {
        if chars[i] != '<' {
            text.push(chars[i]);
            i += 1;
            continue;
        }

        // Comments and doctypes carry nothing.
        if chars[i..].starts_with(&['<', '!']) {
            i += match find_from(&chars, i, ">") {
                Some(end) => end - i + 1,
                None => break,
            };
            continue;
        }
        let Some(end) = find_from(&chars, i, ">") else {
            break;
        };
        let raw: String = chars[i + 1..end].iter().collect();
        i = end + 1;

        let raw = raw.trim().trim_end_matches('/').trim().to_string();
        // Text is flushed only when a tag is actually *emitted*. Text either side of an
        // ignored tag therefore merges into one run, which is what makes a highlighted
        // source block compare as the one string of code it is, rather than as a
        // token-by-token sequence that has to line up exactly.
        if let Some(name) = raw.strip_prefix('/') {
            let name = name.trim().to_ascii_lowercase();
            if !IGNORED.contains(&name.as_str()) && !VOID.contains(&name.as_str()) {
                flush_text(&mut text, &mut out);
                out.push(format!("</{name}>"));
            }
            continue;
        }
        let mut parts = raw.splitn(2, char::is_whitespace);
        let name = parts.next().unwrap_or("").to_ascii_lowercase();
        if name.is_empty() || IGNORED.contains(&name.as_str()) {
            continue;
        }
        let attrs = kept_attributes(parts.next().unwrap_or(""));
        flush_text(&mut text, &mut out);
        out.push(format!("<{name}{attrs}>"));
    }
    flush_text(&mut text, &mut out);
    out
}

fn flush_text(text: &mut String, out: &mut Vec<String>) {
    let decoded = decode_entities(text);
    let collapsed = decoded.split_whitespace().collect::<Vec<_>>().join(" ");
    if !collapsed.is_empty() {
        out.push(format!("{collapsed:?}"));
    }
    text.clear();
}

fn find_from(chars: &[char], from: usize, needle: &str) -> Option<usize> {
    let n: Vec<char> = needle.chars().collect();
    (from..chars.len()).find(|&k| chars[k..].starts_with(&n[..]))
}

/// Keep only the content-bearing attributes, in a stable order.
fn kept_attributes(rest: &str) -> String {
    let mut kept: Vec<(String, String)> = Vec::new();
    for attr in KEPT_ATTRS {
        if let Some(value) = attribute_value(rest, attr) {
            kept.push(((*attr).to_string(), value));
        }
    }
    kept.iter()
        .map(|(k, v)| format!(" {k}=\"{}\"", decode_entities(v)))
        .collect()
}

fn attribute_value(rest: &str, name: &str) -> Option<String> {
    let mut search = rest;
    while let Some(pos) = search.find(name) {
        let before_ok = pos == 0
            || search[..pos]
                .chars()
                .next_back()
                .is_some_and(char::is_whitespace);
        let after = &search[pos + name.len()..];
        let after_trimmed = after.trim_start();
        if before_ok && after_trimmed.starts_with('=') {
            let value = after_trimmed[1..].trim_start();
            let quote = value.chars().next()?;
            if quote == '"' || quote == '\'' {
                let end = value[1..].find(quote)? + 1;
                return Some(value[1..end].to_string());
            }
            let end = value.find(char::is_whitespace).unwrap_or(value.len());
            return Some(value[..end].to_string());
        }
        search = &search[pos + name.len()..];
    }
    None
}

/// Decode the entities either exporter is likely to emit, so an encoding difference is
/// never reported as a semantic one.
fn decode_entities(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        let tail = &rest[amp..];
        let Some(semi) = tail.find(';').filter(|s| *s <= 12) else {
            out.push('&');
            rest = &tail[1..];
            continue;
        };
        let entity = &tail[1..semi];
        let decoded = match entity {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            "nbsp" => Some(' '),
            _ => entity
                .strip_prefix('#')
                .and_then(|n| match n.strip_prefix(['x', 'X']) {
                    Some(hex) => u32::from_str_radix(hex, 16).ok(),
                    None => n.parse::<u32>().ok(),
                })
                .and_then(char::from_u32),
        };
        match decoded {
            // A non-breaking space is a space for comparison purposes.
            Some('\u{a0}') => out.push(' '),
            Some(c) => out.push(c),
            None => {
                out.push('&');
                rest = &tail[1..];
                continue;
            }
        }
        rest = &tail[semi + 1..];
    }
    out.push_str(rest);
    out
}

// ---------------------------------------------------------------------------
// Divergence report
// ---------------------------------------------------------------------------

/// One divergence org-ssg makes on purpose, so the report can separate "we chose this"
/// from "we got this wrong".
///
/// Without this split the agreement percentage is noise: the timestamps fixture sat at
/// 40% while being entirely correct, because org writes `<2024-01-15 Mon>` as text and
/// org-ssg writes a `<time datetime>` element. A number that cannot fall when a real
/// defect appears is not measuring anything.
struct Deliberate {
    name: &'static str,
    /// Does this hunk consist only of the difference described? `ours` are the `-` lines,
    /// `theirs` the `+` lines.
    matches: fn(&[String], &[String]) -> bool,
    /// Only applies once the notes section has started. Two footnote definitions differ
    /// by `</li><li>` against `</sup><p>`, which is the same shape difference — but a
    /// list where a paragraph was expected is a real defect anywhere else, so the rule
    /// is not allowed to explain it anywhere else.
    in_notes_only: bool,
}

fn is_tag(line: &str, names: &[&str]) -> bool {
    names
        .iter()
        .any(|n| line == format!("<{n}>") || line == format!("</{n}>"))
}

fn text_of(line: &str) -> Option<&str> {
    line.strip_prefix('"')?.strip_suffix('"')
}

/// Does this text run contain an org timestamp, `<2024-01-15 Mon>` or `[2024-01-15]`?
fn has_timestamp(line: &str) -> bool {
    let t = match text_of(line) {
        Some(t) => t,
        None => return false,
    };
    t.contains('<') && t.contains('-') || t.contains('[') && t.contains('-')
}

const DELIBERATE: &[Deliberate] = &[
    // `<time datetime="…">` instead of org's plain text: the date is data, and a reader's
    // browser can do something with it.
    Deliberate {
        name: "semantic-time",
        matches: |ours, theirs| {
            !ours.is_empty()
                && ours
                    .iter()
                    .all(|l| is_tag(l, &["time"]) || text_of(l).is_some())
                && theirs.iter().all(|l| has_timestamp(l) || text_of(l).is_some())
        },
        in_notes_only: false,
    },
    // `<figure>`/`<figcaption>` instead of two paragraphs in a div.
    Deliberate {
        name: "figure-element",
        matches: |ours, theirs| {
            ours.iter().any(|l| is_tag(l, &["figure", "figcaption"]))
                && ours
                    .iter()
                    .all(|l| is_tag(l, &["figure", "figcaption"]) || text_of(l).is_some())
                && theirs.iter().all(|l| is_tag(l, &["p"]) || text_of(l).is_some())
        },
        in_notes_only: false,
    },
    // `<em>`/`<strong>` instead of org's presentational `<i>`/`<b>`.
    Deliberate {
        name: "semantic-emphasis",
        matches: |ours, theirs| {
            !ours.is_empty()
                && ours.iter().all(|l| is_tag(l, &["em", "strong", "del"]))
                && theirs.iter().all(|l| is_tag(l, &["i", "b", "s", "del"]))
        },
        in_notes_only: false,
    },
    // `<pre><code>` instead of a bare `<pre>`: the nested element is what every syntax
    // highlighter and every reader's stylesheet expects.
    Deliberate {
        name: "pre-code",
        matches: |ours, theirs| {
            !ours.is_empty() && ours.iter().all(|l| is_tag(l, &["code"])) && theirs.is_empty()
        },
        in_notes_only: false,
    },
    // Org emits a `<colgroup>` of empty `<col>`s to carry column alignment; org-ssg
    // leaves alignment to the stylesheet.
    Deliberate {
        name: "no-colgroup",
        matches: |ours, theirs| {
            ours.is_empty()
                && !theirs.is_empty()
                && theirs.iter().all(|l| is_tag(l, &["colgroup", "col"]) || l == "<col>")
        },
        in_notes_only: false,
    },
    // Footnote ids: `fn-1` rather than org's `fn.1`, because a dot in an id is awkward in
    // a CSS selector.
    Deliberate {
        name: "footnote-anchor-naming",
        matches: |ours, theirs| {
            ours.len() == 1
                && theirs.len() == 1
                && ours[0].replace("fn-", "fn.") == theirs[0]
                && ours[0].contains("#fn")
        },
        in_notes_only: false,
    },
    // The notes section itself: an `<ol>` under a rule, rather than org's headed div of
    // paragraphs, and a `↩` back-link rather than a repeated superscript number. Same
    // notes, same order, same links, in the shape a screen reader announces as a list.
    Deliberate {
        name: "footnote-section-shape",
        matches: |ours, theirs| {
            let ours_is_notes = ours.iter().all(|l| {
                is_tag(l, &["section", "ol", "li", "a", "sup", "p"])
                    || l == "<hr>"
                    || l.starts_with("<a href=\"#fn")
                    || text_of(l) == Some("↩")
                    || text_of(l).is_some()
            });
            let theirs_is_notes = theirs.iter().all(|l| {
                is_tag(l, &["h2", "sup", "p", "a", "div"])
                    || l.starts_with("<a href=\"#fn")
                    || text_of(l).is_some()
            });
            let touches_notes = ours.iter().chain(theirs).any(|l| {
                l.contains("#fn") || text_of(l) == Some("Footnotes:") || is_tag(l, &["section"])
            });
            touches_notes && ours_is_notes && theirs_is_notes
        },
        in_notes_only: false,
    },
    // Two note definitions abutting: `</li><li>` where org writes `</sup><p>`.
    Deliberate {
        name: "footnote-section-shape",
        matches: |ours, theirs| {
            !ours.is_empty()
                && ours.iter().all(|l| is_tag(l, &["li", "ol", "section"]))
                && theirs.iter().all(|l| is_tag(l, &["p", "sup", "div"]))
        },
        in_notes_only: true,
    },
    // Verse ends without a trailing `<br>`: org emits one for the final newline, which is
    // a blank line at the end of the stanza and nothing else.
    Deliberate {
        name: "verse-trailing-break",
        matches: |ours, theirs| ours.is_empty() && theirs == ["<br>"],
        in_notes_only: false,
    },
    // `[[id:…]]` resolves here and does not in the oracle: a single-file `emacs --batch`
    // export has no id database, so org drops the link and keeps its text. This is a
    // property of the harness, not of either exporter.
    Deliberate {
        name: "id-link-resolution",
        matches: |ours, theirs| {
            ours.iter().any(|l| l.starts_with("<a href=\"#"))
                && ours.iter().all(|l| is_tag(l, &["a"]) || l.starts_with("<a href=") || text_of(l).is_some())
                && theirs.iter().all(|l| text_of(l).is_some())
        },
        in_notes_only: false,
    },
    // A change of bullet starts a new list. Org instead continues the list — two blank
    // lines end one, not a switch from `1.` to `-` — so a dash item written under a
    // numbered list is exported *numbered*. We split, which is what the author drew.
    // Deliberate, and the one entry here that is arguably worth revisiting.
    Deliberate {
        name: "list-per-bullet-type",
        matches: |ours, theirs| {
            !ours.is_empty()
                && ours.iter().all(|l| is_tag(l, &["ol", "ul"]))
                && theirs.iter().all(|l| is_tag(l, &["ol", "ul"]))
        },
        in_notes_only: false,
    },
];

/// One run of differing lines: what we wrote, and what Emacs wrote.
struct Hunk {
    ours: Vec<String>,
    theirs: Vec<String>,
}

impl Hunk {
    fn len(&self) -> usize {
        self.ours.len().max(self.theirs.len())
    }

    fn deliberate(&self, in_notes: bool) -> Option<&'static str> {
        DELIBERATE
            .iter()
            .filter(|d| in_notes || !d.in_notes_only)
            .find(|d| (d.matches)(&self.ours, &self.theirs))
            .map(|d| d.name)
    }

    /// Does this hunk start the footnote section?
    fn starts_notes(&self) -> bool {
        self.ours
            .iter()
            .chain(&self.theirs)
            .any(|l| l == "<section>" || text_of(l) == Some("Footnotes:"))
    }
}

enum Op {
    Same(String),
    Differs(Hunk),
}

/// Longest-common-subsequence walk, grouped into runs of agreement and disagreement.
fn align(ours: &[String], theirs: &[String]) -> Vec<Op> {
    let (n, m) = (ours.len(), theirs.len());
    let mut lcs = vec![vec![0usize; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            lcs[i][j] = if ours[i] == theirs[j] {
                lcs[i + 1][j + 1] + 1
            } else {
                lcs[i + 1][j].max(lcs[i][j + 1])
            };
        }
    }

    let mut out: Vec<Op> = Vec::new();
    let push_diff = |out: &mut Vec<Op>, mine: Option<String>, theirs: Option<String>| {
        if let Some(Op::Differs(h)) = out.last_mut() {
            h.ours.extend(mine);
            h.theirs.extend(theirs);
            return;
        }
        out.push(Op::Differs(Hunk {
            ours: mine.into_iter().collect(),
            theirs: theirs.into_iter().collect(),
        }));
    };

    let (mut i, mut j) = (0, 0);
    while i < n && j < m {
        if ours[i] == theirs[j] {
            out.push(Op::Same(ours[i].clone()));
            i += 1;
            j += 1;
        } else if lcs[i + 1][j] >= lcs[i][j + 1] {
            push_diff(&mut out, Some(ours[i].clone()), None);
            i += 1;
        } else {
            push_diff(&mut out, None, Some(theirs[j].clone()));
            j += 1;
        }
    }
    for line in &ours[i..] {
        push_diff(&mut out, Some(line.clone()), None);
    }
    for line in &theirs[j..] {
        push_diff(&mut out, None, Some(line.clone()));
    }
    out
}

/// A unified diff of the two skeletons, with hunks that are deliberate collapsed to a
/// named line. `-` is org-ssg, `+` is Emacs.
///
/// The number that matters is the last one: *unexplained* lines. Agreement can be low
/// while unexplained is zero, and that is a passing state.
fn divergence(ours: &[String], theirs: &[String]) -> String {
    let ops = align(ours, theirs);
    let mut agreed = 0usize;
    let mut deliberate = 0usize;
    let mut unexplained = 0usize;
    let mut by_rule: std::collections::BTreeMap<&str, usize> = std::collections::BTreeMap::new();
    let mut body = String::new();
    let mut in_notes = false;

    for op in &ops {
        match op {
            Op::Same(line) => {
                agreed += 1;
                body.push_str(&format!("  {line}\n"));
            }
            Op::Differs(hunk) => {
                in_notes |= hunk.starts_notes();
                match hunk.deliberate(in_notes) {
                Some(rule) => {
                    deliberate += hunk.len();
                    *by_rule.entry(rule).or_default() += 1;
                    body.push_str(&format!("~ {rule} ({} line(s))\n", hunk.len()));
                }
                None => {
                    unexplained += hunk.len();
                    for line in &hunk.ours {
                        body.push_str(&format!("- {line}\n"));
                    }
                    for line in &hunk.theirs {
                        body.push_str(&format!("+ {line}\n"));
                    }
                }
                }
            }
        }
    }

    let total = ours.len().max(theirs.len());
    let pct = if total == 0 {
        100.0
    } else {
        100.0 * agreed as f64 / total as f64
    };
    let rules: Vec<String> = by_rule
        .iter()
        .map(|(name, n)| format!("{name} ×{n}"))
        .collect();
    format!(
        "agreement: {agreed}/{total} skeleton lines ({pct:.1}%)\n\
         deliberate: {deliberate} line(s){}\n\
         unexplained: {unexplained} line(s)\n\
         (- org-ssg, + emacs, ~ a difference we mean to have)\n\n{body}",
        if rules.is_empty() {
            String::new()
        } else {
            format!(" — {}", rules.join(", "))
        }
    )
}

/// Snapshot the divergence between org-ssg and Emacs for one fixture.
fn compare(fixture: &str) -> Option<String> {
    if !emacs_available() {
        eprintln!("skipping oracle comparison for {fixture}: no emacs on PATH");
        return None;
    }
    let ours = skeleton(&our_export(fixture));
    let theirs = skeleton(&org_export(fixture));
    Some(divergence(&ours, &theirs))
}

macro_rules! oracle_test {
    ($name:ident, $fixture:literal) => {
        #[test]
        fn $name() {
            if let Some(report) = compare($fixture) {
                insta::assert_snapshot!(report);
            }
        }
    };
}

oracle_test!(oracle_minimal, "minimal.org");
oracle_test!(oracle_core, "core.org");
oracle_test!(oracle_headings, "headings.org");
oracle_test!(oracle_lists, "lists.org");
oracle_test!(oracle_blocks, "blocks.org");
oracle_test!(oracle_table, "table.org");
oracle_test!(oracle_footnote, "footnote.org");
oracle_test!(oracle_timestamps, "timestamps.org");
oracle_test!(oracle_images, "images.org");
oracle_test!(oracle_elements, "elements.org");

/// The gate the snapshots cannot be: *every* divergence from Emacs must be one we chose.
///
/// The percentages above are context, not a target — the timestamps fixture agrees on
/// 40% of its lines and is entirely correct, because org writes a date as text where
/// org-ssg writes `<time datetime>`. What must hold is that nothing diverges for a
/// reason nobody has written down. A new unexplained line means either a defect to fix
/// or a decision to record in `DELIBERATE`.
#[test]
fn every_divergence_from_emacs_is_deliberate() {
    let fixtures = [
        "minimal.org",
        "core.org",
        "headings.org",
        "lists.org",
        "blocks.org",
        "table.org",
        "footnote.org",
        "timestamps.org",
        "images.org",
        "elements.org",
    ];
    let mut offenders = Vec::new();
    for fixture in fixtures {
        let Some(report) = compare(fixture) else {
            return; // no emacs on PATH; the suite skips cleanly
        };
        if !report.contains("unexplained: 0 line(s)") {
            let count = report
                .lines()
                .find(|l| l.starts_with("unexplained:"))
                .unwrap_or("unexplained: ?");
            offenders.push(format!("{fixture}: {count}"));
        }
    }
    assert!(
        offenders.is_empty(),
        "these fixtures diverge from Emacs for unrecorded reasons — fix the defect, or \
         add a rule to DELIBERATE saying why the difference is wanted:\n  {}",
        offenders.join("\n  ")
    );
}

// ---------------------------------------------------------------------------
// Invariants that must hold against the oracle, not merely be snapshotted
// ---------------------------------------------------------------------------

/// How many headings a document has and at what depth is the shape of the document.
/// Getting it wrong reorganizes someone's writing, so it is asserted rather than
/// snapshotted. Heading *decoration* (priority cookies, tag markup) is a policy
/// difference and is left to the snapshots.
#[test]
fn heading_structure_matches_emacs() {
    if !emacs_available() {
        eprintln!("skipping: no emacs on PATH");
        return;
    }
    for fixture in ["minimal.org", "core.org", "headings.org", "lists.org"] {
        let ours = heading_levels(&skeleton(&our_export(fixture)));
        let theirs = heading_levels(&skeleton(&org_export(fixture)));
        assert_eq!(
            ours, theirs,
            "heading structure diverges from Emacs in {fixture}"
        );
    }
}

/// The sequence of heading open tags, e.g. `["<h1>", "<h2>", "<h1>"]`.
fn heading_levels(skeleton: &[String]) -> Vec<String> {
    skeleton
        .iter()
        .filter(|l| l.starts_with("<h") && l[2..].starts_with(|c: char| c.is_ascii_digit()))
        .cloned()
        .collect()
}

/// A list is the construct where nesting is easiest to get subtly wrong, and where being
/// wrong changes the meaning of the document rather than its looks.
#[test]
fn list_nesting_matches_emacs() {
    if !emacs_available() {
        eprintln!("skipping: no emacs on PATH");
        return;
    }
    let ours = list_shape(&skeleton(&our_export("lists.org")));
    let theirs = list_shape(&skeleton(&org_export("lists.org")));
    assert_eq!(ours, theirs, "list nesting diverges from Emacs");
}

/// The sequence of list opens/closes, ignoring content — the shape of the nesting.
fn list_shape(skeleton: &[String]) -> Vec<String> {
    skeleton
        .iter()
        .filter(|l| {
            matches!(
                l.as_str(),
                "<ul>" | "</ul>" | "<ol>" | "</ol>" | "<li>" | "</li>" | "<dl>" | "</dl>"
                    | "<dt>" | "</dt>" | "<dd>" | "</dd>"
            )
        })
        .cloned()
        .collect()
}

/// Code must survive verbatim. Highlighting markup differs by construction (syntect
/// spans vs htmlize), but if the *characters of the program* differ, we have corrupted
/// the author's content.
#[test]
fn source_block_text_matches_emacs() {
    if !emacs_available() {
        eprintln!("skipping: no emacs on PATH");
        return;
    }
    for fixture in ["blocks.org", "core.org", "elements.org"] {
        let ours = code_text(&our_export(fixture));
        let theirs = code_text(&org_export(fixture));
        assert_eq!(ours, theirs, "source block text diverges from Emacs in {fixture}");
    }
}

/// All text inside `<pre>` blocks, with tags stripped and whitespace collapsed.
fn code_text(html: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = html;
    while let Some(start) = rest.find("<pre") {
        let after = &rest[start..];
        let Some(open_end) = after.find('>') else { break };
        let Some(close) = after.find("</pre>") else { break };
        let inner = &after[open_end + 1..close];
        out.push(strip_tags(inner));
        rest = &after[close + 6..];
    }
    out
}

/// All text in a fragment with tags removed and entities decoded, then whitespace
/// collapsed once at the end.
///
/// [`skeleton`] cannot do this job: it trims each text run individually, which is
/// invisible for prose (one run per paragraph) but destructive for highlighted code,
/// where syntect splits a line into one run per token and the spaces *between* tokens
/// live at the edges of those runs. Trimming each run turns `def greet` into `defgreet`.
fn strip_tags(html: &str) -> String {
    let mut text = String::new();
    let mut rest = html;
    while let Some(open) = rest.find('<') {
        text.push_str(&rest[..open]);
        match rest[open..].find('>') {
            Some(close) => rest = &rest[open + close + 1..],
            None => {
                rest = "";
                break;
            }
        }
    }
    text.push_str(rest);
    decode_entities(&text)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}
