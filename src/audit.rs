//! Corpus audit (spec §5, Phase 0): measure which org constructs a real corpus actually
//! uses, and classify each against the supported/unsupported line.
//!
//! This exists because the scope was recommended rather than measured — a guess about
//! which slice of org matters. A guess about a corpus is a hypothesis, and this is the
//! experiment. It answers two questions:
//!
//! 1. **Coverage** — of the constructs this corpus uses, which do we handle? A construct
//!    that is common here and out of scope is a scope bug, not a corpus quirk.
//! 2. **Blind spots** — which constructs are here that the implementation has no opinion
//!    about at all? These are the dangerous ones: not "known unsupported" but unknown.
//!
//! The audit is deliberately a *separate, line-oriented scanner* rather than a reuse of
//! [`crate::parser`]. Auditing with the parser could only ever find constructs the parser
//! already knows about, which is precisely the wrong instrument for question 2 — it would
//! report a blind spot as clean.
//!
//! Nothing here reports document *text*. Counts, construct names, and `file:line`
//! locations only, so an audit of private notes stays publishable.

use std::collections::BTreeMap;

use anyhow::{Context, Result};
use camino::{Utf8Path, Utf8PathBuf};
use walkdir::WalkDir;

/// Where a construct sits relative to the supported set, which
/// `docs/guide/05-org-support.org` defines.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Scope {
    /// orgo handles this.
    In,
    /// orgo deliberately excludes this; it degrades predictably.
    Out,
}

impl Scope {
    fn label(self) -> &'static str {
        match self {
            Scope::In => "IN ",
            Scope::Out => "OUT",
        }
    }
}

/// One construct's tally across the corpus.
#[derive(Debug, Default, Clone)]
pub struct Tally {
    pub occurrences: usize,
    pub files: usize,
    /// First `file:line` the construct was seen at, to make a finding actionable.
    pub first_seen: Option<String>,
    /// Set while scanning one file, to count each file once.
    seen_in_current_file: bool,
}

/// The audit result: the fixed construct catalog plus the dynamic name censuses.
#[derive(Debug, Default)]
pub struct Audit {
    pub files: usize,
    pub lines: usize,
    /// Catalogued constructs → tally.
    pub constructs: BTreeMap<(Scope, &'static str), Tally>,
    /// Every distinct `#+KEYWORD:` seen, by name.
    pub keywords: BTreeMap<String, Tally>,
    /// Every distinct `#+BEGIN_<TYPE>` seen, by type.
    pub blocks: BTreeMap<String, Tally>,
    /// Every distinct `:DRAWER:` seen, by name.
    pub drawers: BTreeMap<String, Tally>,
    /// Every distinct link scheme seen (`https`, `file`, `id`, `denote`, ...).
    pub link_schemes: BTreeMap<String, Tally>,
}

/// Names the implementation understands, so the census can flag everything else. These
/// are the *recognized* sets, not the supported ones: `INCLUDE` is recognized (it is
/// deliberately inert) while an unlisted keyword is a genuine blind spot.
const KNOWN_KEYWORDS: &[&str] = &[
    "TITLE", "AUTHOR", "DATE", "EMAIL", "LANGUAGE", "OPTIONS", "FILETAGS", "DESCRIPTION",
    "KEYWORDS", "CAPTION", "NAME", "ATTR_HTML", "RESULTS", "TBLFM", "INCLUDE", "TODO",
    "STARTUP", "SUBTITLE", "SETUPFILE", "MACRO", "PROPERTY", "HTML_HEAD", "EXCLUDE_TAGS",
    // orgo's own keywords, each read by name: `SLUG` names the output file
    // (`util::output_path`), `DRAFT` decides whether the page publishes at all
    // (`util::is_draft`), and `TEMPLATE` picks the template (`config::page_template`).
    // Leaving them out reported the corpus's most-used keyword as unrecognized.
    "SLUG", "DRAFT", "TEMPLATE",
];
const KNOWN_DRAWERS: &[&str] = &["PROPERTIES", "LOGBOOK", "END"];
/// Keyword names conventional enough to be worth flagging when they lead a heading.
/// A custom sequence is only *real* if some `#+TODO:` declares it, which the census
/// reports separately — this list keeps the heading-level signal honest.
const CONVENTIONAL_TODO_KEYWORDS: &[&str] = &[
    "NEXT", "WAITING", "HOLD", "CANCELLED", "CANCELED", "STARTED", "SOMEDAY", "PROJ",
    "IN-PROGRESS", "BLOCKED", "REVIEW",
];
const KNOWN_SCHEMES: &[&str] = &[
    "http", "https", "mailto", "ftp", "news", "tel", "file", "id", "custom-id", "heading",
    "relative",
];

impl Audit {
    /// Is this name one the implementation recognizes?
    pub fn is_known(kind: Census, name: &str) -> bool {
        let known = match kind {
            Census::Keyword => KNOWN_KEYWORDS,
            // Every block name renders, and renders as org renders it: the names in
            // `block_construct` through dedicated handling, every other name as a special
            // block — a div carrying the name, holding parsed org, which is exactly what
            // org's exporter emits. No block name is a blind spot.
            Census::Block => return true,
            Census::Drawer => KNOWN_DRAWERS,
            Census::Scheme => KNOWN_SCHEMES,
        };
        known.iter().any(|k| k.eq_ignore_ascii_case(name))
    }
}

/// Which dynamic census a name belongs to.
#[derive(Debug, Clone, Copy)]
pub enum Census {
    Keyword,
    Block,
    Drawer,
    Scheme,
}

/// Walk `root`, auditing every `.org` file.
pub fn audit(root: &Utf8Path) -> Result<Audit> {
    let mut audit = Audit::default();
    let mut paths: Vec<Utf8PathBuf> = Vec::new();

    if root.is_file() {
        paths.push(root.to_owned());
    } else {
        for entry in WalkDir::new(root).sort_by_file_name() {
            let entry = entry.with_context(|| format!("walking {root}"))?;
            if !entry.file_type().is_file() {
                continue;
            }
            let path = Utf8PathBuf::from_path_buf(entry.into_path())
                .map_err(|p| anyhow::anyhow!("non-UTF-8 path: {}", p.display()))?;
            if path.extension() == Some("org") {
                paths.push(path);
            }
        }
    }

    for path in &paths {
        // A file that cannot be read is reported and skipped: an audit of 179 files
        // should not be lost to one unreadable one.
        let source = match std::fs::read_to_string(path) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("warning: skipping {path}: {e}");
                continue;
            }
        };
        let rel = path.strip_prefix(root).unwrap_or(path).to_owned();
        audit.scan_file(&rel, &source);
        audit.files += 1;
    }
    Ok(audit)
}

impl Audit {
    fn scan_file(&mut self, path: &Utf8Path, source: &str) {
        // Reset the per-file flags so each construct counts this file at most once.
        for tally in self.constructs.values_mut() {
            tally.seen_in_current_file = false;
        }
        for map in [
            &mut self.keywords,
            &mut self.blocks,
            &mut self.drawers,
            &mut self.link_schemes,
        ] {
            for tally in map.values_mut() {
                tally.seen_in_current_file = false;
            }
        }

        let mut in_block: Option<String> = None;
        for (idx, line) in source.lines().enumerate() {
            self.lines += 1;
            let at = format!("{path}:{}", idx + 1);
            let trimmed = line.trim_start();

            // Inside a verbatim block only the terminator matters — a `*` in a source
            // block is not a heading, and counting it as one would corrupt the audit.
            if let Some(kind) = &in_block {
                if trimmed.to_ascii_uppercase().starts_with("#+END_") {
                    in_block = None;
                } else if kind.eq_ignore_ascii_case("SRC") || kind.eq_ignore_ascii_case("EXAMPLE") {
                    continue;
                }
                continue;
            }
            if let Some(rest) = trimmed.to_ascii_uppercase().strip_prefix("#+BEGIN_") {
                let kind = rest.split_whitespace().next().unwrap_or("").to_string();
                self.count_census(Census::Block, &kind, &at);
                self.count(Scope::In, block_construct(&kind), &at);
                if trimmed.to_ascii_uppercase().contains(":RESULTS") {
                    self.count(Scope::Out, "babel header args (:results)", &at);
                }
                in_block = Some(kind);
                continue;
            }

            self.scan_line(line, trimmed, &at);
        }
    }

    fn scan_line(&mut self, line: &str, trimmed: &str, at: &str) {
        // --- headings and their metadata ---
        if let Some(stars) = heading_stars(line) {
            self.count(Scope::In, "heading", at);
            let rest = line[stars..].trim();
            let word = rest.split_whitespace().next().unwrap_or("");
            if word == "TODO" || word == "DONE" {
                self.count(Scope::In, "TODO keyword (default set)", at);
            } else if CONVENTIONAL_TODO_KEYWORDS.contains(&word) {
                // Only conventional keyword names count. "Any all-caps first word" is
                // the tempting rule and it is wrong: it reads `* CSS Variables` as the
                // keyword `CSS`, which on this corpus produced 23 false positives and
                // zero true ones. An audit that overstates a gap is worse than no audit,
                // because it argues for work nobody needs.
                self.count(Scope::Out, "TODO keyword (custom sequence)", at);
            }
            if rest.contains("[#") {
                self.count(Scope::In, "priority cookie", at);
            }
            if rest.trim_end().ends_with(':') && rest.trim_end().matches(':').count() >= 2 {
                self.count(Scope::In, "heading tags", at);
            }
            if rest.contains("[/") || rest.contains("[%") {
                self.count(Scope::Out, "statistics cookie", at);
            }
            return;
        }

        // --- planning and clocking ---
        for marker in ["SCHEDULED:", "DEADLINE:", "CLOSED:"] {
            if trimmed.starts_with(marker) {
                self.count(Scope::Out, "planning line", at);
            }
        }
        if trimmed.starts_with("CLOCK:") {
            self.count(Scope::Out, "clock entry", at);
        }

        // --- keywords and drawers ---
        if let Some(rest) = trimmed.strip_prefix("#+") {
            if let Some(colon) = rest.find(':') {
                let key = rest[..colon].trim().to_ascii_uppercase();
                if !key.is_empty() && !key.contains(char::is_whitespace) {
                    self.count_census(Census::Keyword, &key, at);
                    match key.as_str() {
                        "CAPTION" | "NAME" | "ATTR_HTML" => {
                            self.count(Scope::In, "affiliated keyword", at)
                        }
                        // Not a gap: org's HTML exporter does not recalculate `#+TBLFM:`
                        // either, so the cells as written are what both exporters emit.
                        // `fixtures/tblfm.org` holds the oracle to that (`tests/oracle.rs`).
                        // Unlike `#+INCLUDE:`, nothing is lost by leaving it inert.
                        "TBLFM" => self.count(Scope::In, "table formula (#+TBLFM:)", at),
                        "INCLUDE" => self.count(Scope::Out, "#+INCLUDE:", at),
                        "RESULTS" => self.count(Scope::Out, "babel results block", at),
                        "TODO" => self.count(Scope::Out, "#+TODO: keyword sequence", at),
                        "MACRO" => self.count(Scope::Out, "macro definition", at),
                        _ => self.count(Scope::In, "#+ keyword", at),
                    }
                }
            }
        } else if is_drawer(trimmed) {
            let name = trimmed[1..trimmed.len() - 1].to_ascii_uppercase();
            if name != "END" {
                self.count_census(Census::Drawer, &name, at);
                match name.as_str() {
                    "PROPERTIES" => self.count(Scope::In, "property drawer", at),
                    _ => self.count(Scope::Out, "non-PROPERTIES drawer", at),
                }
            }
        }

        // --- lists, tables, rules ---
        if let Some(bullet) = list_bullet(trimmed) {
            self.count(Scope::In, "list item", at);
            if bullet == Bullet::Ordered {
                self.count(Scope::In, "ordered list", at);
            }
            let indent = line.len() - trimmed.len();
            if indent > 0 {
                self.count(Scope::In, "nested list item", at);
            }
            if trimmed.contains(" :: ") {
                self.count(Scope::In, "description list", at);
            }
            let after = trimmed.trim_start_matches(['-', '+', '*', ' ']);
            if after.starts_with("[ ]") || after.starts_with("[X]") || after.starts_with("[-]") {
                self.count(Scope::In, "checkbox", at);
            }
        }
        if trimmed.starts_with('|') {
            self.count(Scope::In, "table row", at);
        }
        if trimmed.starts_with(':') && !is_drawer(trimmed) && trimmed.starts_with(": ") {
            self.count(Scope::Out, "fixed-width line", at);
        }

        // --- footnotes ---
        if trimmed.starts_with("[fn:") {
            self.count(Scope::In, "footnote definition", at);
        } else if line.contains("[fn:") {
            self.count(Scope::In, "footnote reference", at);
        }

        // --- inline objects ---
        self.scan_inline(line, at);
    }

    fn scan_inline(&mut self, line: &str, at: &str) {
        // Links: count each `[[target]]`, censusing its scheme.
        let mut rest = line;
        while let Some(start) = rest.find("[[") {
            let after = &rest[start + 2..];
            let Some(end) = after.find("]]") else { break };
            let inner = &after[..end];
            let target = inner.split("][").next().unwrap_or(inner);
            self.count(Scope::In, "link", at);
            self.count_census(Census::Scheme, &link_scheme(target), at);
            rest = &after[end..];
        }

        // The rest read a line that has had its `=verbatim=` and `~code~` spans blanked:
        // a construct shown inside verbatim is displayed rather than rendered, so a page
        // documenting org syntax is not a use of the syntax it names. Emphasis pairs
        // below deliberately still see the raw line — `=` is one of the markers scanned.
        let bare = without_literal_spans(line);

        if has_timestamp(&bare) {
            self.count(Scope::In, "timestamp", at);
        }
        if bare.contains("{{{") {
            self.count(Scope::Out, "macro call", at);
        }
        if bare.contains("<<<") {
            self.count(Scope::Out, "radio target", at);
        } else if bare.contains("<<") && bare.contains(">>") {
            self.count(Scope::Out, "internal target", at);
        }
        if bare.contains("\\begin{") || latex_inline(&bare) {
            self.count(Scope::Out, "LaTeX fragment", at);
        }
        if entity_ref(&bare) {
            // Rendered, and rendered as org renders it: `\alpha` becomes `&alpha;` in
            // both exporters. `fixtures/audit-entities.org` holds the oracle to that.
            self.count(Scope::In, "entity (\\name)", at);
        }
        for (marker, name) in [
            ('*', "bold"),
            ('/', "italic"),
            ('_', "underline"),
            ('+', "strike-through"),
            ('=', "verbatim"),
            ('~', "code"),
        ] {
            if emphasis_pair(line, marker) {
                self.count(Scope::In, name, at);
            }
        }
    }

    fn count(&mut self, scope: Scope, name: &'static str, at: &str) {
        let tally = self.constructs.entry((scope, name)).or_default();
        bump(tally, at);
    }

    fn count_census(&mut self, kind: Census, name: &str, at: &str) {
        let map = match kind {
            Census::Keyword => &mut self.keywords,
            Census::Block => &mut self.blocks,
            Census::Drawer => &mut self.drawers,
            Census::Scheme => &mut self.link_schemes,
        };
        let tally = map.entry(name.to_string()).or_default();
        bump(tally, at);
    }
}

fn bump(tally: &mut Tally, at: &str) {
    tally.occurrences += 1;
    if !tally.seen_in_current_file {
        tally.seen_in_current_file = true;
        tally.files += 1;
    }
    if tally.first_seen.is_none() {
        tally.first_seen = Some(at.to_string());
    }
}

// ---------------------------------------------------------------------------
// Line-level detectors. Deliberately independent of the parser (see module docs).
// ---------------------------------------------------------------------------

fn heading_stars(line: &str) -> Option<usize> {
    if !line.starts_with('*') {
        return None;
    }
    let stars = line.chars().take_while(|c| *c == '*').count();
    let after = &line[stars..];
    (after.starts_with(' ') || after.is_empty()).then_some(stars)
}

#[derive(PartialEq)]
enum Bullet {
    Unordered,
    Ordered,
}

fn list_bullet(trimmed: &str) -> Option<Bullet> {
    let bytes = trimmed.as_bytes();
    if bytes.is_empty() {
        return None;
    }
    if (bytes[0] == b'-' || bytes[0] == b'+') && (bytes.len() == 1 || bytes[1] == b' ') {
        return Some(Bullet::Unordered);
    }
    let digits = trimmed.chars().take_while(|c| c.is_ascii_digit()).count();
    if digits > 0 {
        let after = &trimmed[digits..];
        if (after.starts_with('.') || after.starts_with(')'))
            && (after.len() == 1 || after.as_bytes()[1] == b' ')
        {
            return Some(Bullet::Ordered);
        }
    }
    None
}

fn is_drawer(trimmed: &str) -> bool {
    let t = trimmed.trim_end();
    t.len() >= 3
        && t.starts_with(':')
        && t.ends_with(':')
        && t[1..t.len() - 1]
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        && t.len() > 2
}

fn block_construct(kind: &str) -> &'static str {
    match kind.to_ascii_uppercase().as_str() {
        "SRC" => "source block",
        "QUOTE" => "quote block",
        "EXAMPLE" => "example block",
        "CENTER" => "center block",
        "EXPORT" => "export block",
        "VERSE" => "verse block",
        "COMMENT" => "comment block",
        _ => "special block",
    }
}

/// The scheme of a link target, normalized into the census's vocabulary.
fn link_scheme(target: &str) -> String {
    if let Some(rest) = target.split_once(':') {
        let scheme = rest.0;
        if !scheme.is_empty()
            && scheme
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '+')
        {
            return scheme.to_ascii_lowercase();
        }
    }
    if target.starts_with('#') {
        return "custom-id".to_string();
    }
    if target.starts_with('*') {
        return "heading".to_string();
    }
    "relative".to_string()
}

/// A `<...>`/`[...]` span opening with an ISO date is a timestamp.
fn has_timestamp(line: &str) -> bool {
    let bytes = line.as_bytes();
    for (i, c) in line.char_indices() {
        if c != '<' && c != '[' {
            continue;
        }
        let rest = &bytes[i + 1..];
        if rest.len() >= 10
            && rest[..4].iter().all(u8::is_ascii_digit)
            && rest[4] == b'-'
            && rest[5..7].iter().all(u8::is_ascii_digit)
            && rest[7] == b'-'
            && rest[8..10].iter().all(u8::is_ascii_digit)
        {
            return true;
        }
    }
    false
}

/// `$x$` or `\(x\)` inline math. `$` alone (a price, a shell prompt) is not math.
fn latex_inline(line: &str) -> bool {
    if line.contains("\\(") && line.contains("\\)") {
        return true;
    }
    let dollars = line.matches('$').count();
    dollars >= 2 && line.contains("$\\")
}

/// A `\name` entity reference such as `\alpha`.
///
/// Only names org knows count, matching [`crate::parser`]'s rule for rendering one: a
/// Windows path (`C:\Users\me`) and a namespaced identifier (`Tumblr\API\Client`) are not
/// entity references.
fn entity_ref(line: &str) -> bool {
    let chars: Vec<char> = line.chars().collect();
    for (i, c) in chars.iter().enumerate() {
        if *c != '\\' {
            continue;
        }
        let name: String = chars[i + 1..].iter().take_while(|c| c.is_ascii_alphabetic()).collect();
        if crate::entities::lookup(&name).is_some() {
            return true;
        }
    }
    false
}

/// Blank out `=verbatim=` and `~code~` spans. Deliberately looser than the parser's
/// border rules — the audit measures prevalence, and erring toward blanking keeps it
/// from overstating a gap.
fn without_literal_spans(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut open: Option<char> = None;
    for c in line.chars() {
        match open {
            Some(marker) => {
                out.push(' ');
                if c == marker {
                    open = None;
                }
            }
            None if c == '=' || c == '~' => {
                open = Some(c);
                out.push(' ');
            }
            None => out.push(c),
        }
    }
    out
}

/// A plausible `*bold*`-style emphasis pair: two markers on one line with non-space
/// content between them. Approximate by design — the audit measures prevalence, and the
/// parser owns the exact pre/post-character rules.
fn emphasis_pair(line: &str, marker: char) -> bool {
    let positions: Vec<usize> = line
        .char_indices()
        .filter(|(_, c)| *c == marker)
        .map(|(i, _)| i)
        .collect();
    if positions.len() < 2 {
        return false;
    }
    // A leading `*` is a heading, and `-`/`+` at line start is a bullet.
    let trimmed = line.trim_start();
    if trimmed.starts_with(marker) {
        return false;
    }
    positions.windows(2).any(|w| w[1] > w[0] + 1)
}

// ---------------------------------------------------------------------------
// Report
// ---------------------------------------------------------------------------

/// Render the audit as a readable report. Names, counts and locations only — never
/// document text, so an audit of private notes is safe to paste into an issue.
pub fn report(audit: &Audit) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "corpus: {} file(s), {} line(s)\n",
        audit.files, audit.lines
    ));

    let mut rows: Vec<(&(Scope, &str), &Tally)> = audit.constructs.iter().collect();
    rows.sort_by(|a, b| {
        b.1.occurrences
            .cmp(&a.1.occurrences)
            .then_with(|| a.0 .1.cmp(b.0 .1))
    });

    out.push_str("\nCONSTRUCTS (by frequency)\n");
    out.push_str(&format!(
        "{:<4} {:<32} {:>8} {:>7}  {}\n",
        "", "construct", "uses", "files", "first seen"
    ));
    for ((scope, name), tally) in &rows {
        out.push_str(&format!(
            "{:<4} {:<32} {:>8} {:>7}  {}\n",
            scope.label(),
            name,
            tally.occurrences,
            tally.files,
            tally.first_seen.as_deref().unwrap_or("")
        ));
    }

    let in_uses: usize = rows
        .iter()
        .filter(|((s, _), _)| *s == Scope::In)
        .map(|(_, t)| t.occurrences)
        .sum();
    let out_uses: usize = rows
        .iter()
        .filter(|((s, _), _)| *s == Scope::Out)
        .map(|(_, t)| t.occurrences)
        .sum();
    let total = in_uses + out_uses;
    let pct = |n: usize| {
        if total == 0 {
            0.0
        } else {
            100.0 * n as f64 / total as f64
        }
    };
    out.push_str(&format!(
        "\ncoverage: {in_uses} in-scope use(s) ({:.1}%), {out_uses} out-of-scope ({:.1}%)\n",
        pct(in_uses),
        pct(out_uses)
    ));

    for (title, kind, map) in [
        ("KEYWORDS", Census::Keyword, &audit.keywords),
        ("BLOCK TYPES", Census::Block, &audit.blocks),
        ("DRAWERS", Census::Drawer, &audit.drawers),
        ("LINK SCHEMES", Census::Scheme, &audit.link_schemes),
    ] {
        let mut names: Vec<(&String, &Tally)> = map.iter().collect();
        names.sort_by(|a, b| b.1.occurrences.cmp(&a.1.occurrences).then(a.0.cmp(b.0)));
        out.push_str(&format!("\n{title}\n"));
        for (name, tally) in names {
            let flag = if Audit::is_known(kind, name) {
                "   "
            } else {
                "??? "
            };
            out.push_str(&format!(
                "{flag}{:<32} {:>8} {:>7}  {}\n",
                name,
                tally.occurrences,
                tally.files,
                tally.first_seen.as_deref().unwrap_or("")
            ));
        }
    }
    out.push_str("\n`???` marks a name the implementation does not recognize at all.\n");
    out
}
