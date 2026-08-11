# org-ssg

An org-mode static site generator, in Rust. Org is treated as the *source language*,
not an inconvenient input to be normalized into markdown. The org element tree —
headings, drawers, blocks, links with their org-specific semantics — **is** the
document model, and we render that tree straight to HTML. We never round-trip through
a markdown-shaped intermediate representation, because the point is to preserve what
markdown cannot express: property drawers, TODO/priority/tag metadata on headings,
`#+` directives, ID links, named/captioned blocks, footnote semantics.

The one non-obvious early commitment is **incremental builds keyed on content
hashing**, treated as a first-class architectural concern from day one. The discipline
it imposes on the data model — pure, hashable, dependency-tracked units — is the real
deliverable, even while the corpus is small enough that a full rebuild is instant.

## Pipeline

```
DISCOVER → PARSE → INDEX → RESOLVE → RENDER → TEMPLATE → EMIT
```

PARSE and RENDER are pure functions of their inputs (cacheable, hashable). INDEX/RESOLVE
is the only inherently global stage — it is where the link dependency graph is born.

| Stage | Module | Notes |
|---|---|---|
| PARSE | `src/parser.rs` | Hand-written recursive descent: line lexer → element builder → inline tokenizer. |
| audit | `src/audit.rs` | Phase 0 corpus audit: construct frequencies against the IN/OUT line. |
| model | `src/model.rs` | The org element tree — Elements (block) vs Objects (inline). |
| INDEX | `src/index.rs` | Collect link targets into a symbol table. |
| RESOLVE | `src/resolve.rs` | Rewrite links to URLs; return the used-target list (dependency edges). |
| RENDER | `src/render.rs` | Tree → HTML fragment; syntect highlighting; footnote two-pass. |
| TEMPLATE | `src/template.rs` | minijinja: fragment + metadata → full page. |
| incremental | `src/incremental.rs` | Content/config/template hashing, dep graph, cache manifest, invalidation. |

## v1 scope (delivered as of v0.4; still to be reconciled against a corpus audit)

**IN — v1 must handle:** headings with nesting; TODO keywords; priorities `[#A]`; tags;
property drawers; plain lists (unordered/ordered/description, checkboxes, nesting);
tables (with rule rows, no `#+TBLFM:`); source blocks with syntax highlighting;
example/quote/center blocks; links (external, internal `[[*Heading]]`/`[[#custom-id]]`,
`id:`); footnotes (inline and referenced); `#+` keywords/directives; inline markup
(bold/italic/underline/verbatim/code/strike); timestamps (active/inactive, ranges);
paragraphs and horizontal rules; images with `#+CAPTION`/`#+ATTR_HTML`.

**OUT — explicitly not v1 (parse-and-ignore or reject loudly):** Babel execution /
`:results`; `#+TBLFM:` formulas; LaTeX / MathJax; `#+INCLUDE:`; radio targets and
macros; drawers other than PROPERTIES/LOGBOOK; column view / clocking / agenda
semantics; non-HTML export blocks; the full Unicode entity set.

**Scope guardrail:** every IN item gets a golden-file fixture; every OUT item gets a test
asserting it degrades predictably (ignored, no crash). The IN/OUT line is enforced by
`tests/constructs.rs`, defending against the project's #1 risk: scope creep back toward
all-of-org. Phase 0 checked this line against a real 179-file corpus and found it sound
(99.9% of construct uses in scope) — but also found one thing missing from it entirely:
`#+SLUG:`. See [Phase 0](#phase-0-the-corpus-audit-and-the-emacs-oracle).

## Phase plan

| Phase | Scope | Status |
|---|---|---|
| **M0** | **Buildable skeleton: crate layout, module stubs, deps, test harness, fixtures** | **done** |
| **v0.1** | **End-to-end core parse → render: `build` a single `.org` file to HTML** | **done** |
| **v0.2** | **Multi-file SITE build: INDEX + RESOLVE internal links, minijinja templates, `build <src-dir> <out-dir>`, tables + footnotes** | **done** |
| **v0.3** | **Incremental build layer: content/config/template hashing, dependency graph, per-page render keys, persisted cache manifest, invalidation** | **done** |
| **v0.4** | **MVP: the full v1 construct scope — heading metadata, nested/description lists, block types, timestamps, images, syntect highlighting — with the IN/OUT line under test** | **done** |
| **0** | **Corpus audit + `emacs --batch` ground-truth oracle** | **done** |
| 1 | Line lexer + heading/section skeleton | done |
| 2 | Block elements — lists, source blocks, tables, footnote defs, blocks by type, drawers | done |
| 3 | Inline objects — emphasis, links, bare URLs, footnote refs, timestamps | done |
| 4 | Rendering to HTML — tree walk, tables, footnote two-pass, minijinja templating, syntect highlighting | done |
| 5 | Link resolution + symbol table (INDEX + RESOLVE, used-target list, broken-link reporting) | done |
| 6 | Incremental build layer (hashing, dep graph, invalidation) done; `watch` is a simple poll loop | done |
| 7 | Hardening: rayon parallelism, error locations in parse diagnostics | todo |

### v0.2 in / out

**Added in v0.2:** the INDEX stage (`SymbolTable` of `:ID:`/`:CUSTOM_ID:`/heading/`file:`
targets across a directory); the RESOLVE stage — rewrites `[[#custom-id]]`, `[[id:...]]`,
`[[*Heading]]` and `[[file:other.org]]` links to real relative output URLs, returns the
`used_targets` list (the `uses` edges, spec §4.3/R2) and reports unresolved links as
warnings rather than crashing; a minijinja base layout (title, nav, body) applied to every
page; a `build <src-dir> <out-dir>` path that walks the tree, parses + resolves + renders +
templates every `.org` into a linked static site and copies non-`.org` assets through;
plus two new constructs — pipe **tables** (with header band from the rule row) and
**footnotes** (block `[fn:1]` definitions, referenced `[fn:1]`, and inline `[fn:1:text]`,
rendered as a numbered, back-linked notes section).

**Left stubbed at v0.2, all closed in v0.4:** timestamps; TODO keywords and priorities;
generic (non-PROPERTIES) drawers; real syntect tokenizing behind the `Highlighter` trait.

### v0.3 in / out

**Added in v0.3 — the incremental build layer (spec §4, the flagship, non-retrofittable
feature):**

- **Three hash classes (spec §4.1)** in `src/incremental.rs`: a **content hash** (blake3
  of a file's bytes), a **config hash** (blake3 of the resolved `BuildConfig`), and a
  **template hash** (blake3 of the template sources). A change in any one invalidates the
  pages it affects.
- **Dependency graph (spec §4.3)** built from RESOLVE's `defines`/`uses` edges: a page
  depends on the targets it links to, so editing (or renaming a heading in) a file
  invalidates the pages that *link into* it, not just the file itself — the load-bearing
  R2 invariant. On rebuild the graph is merged with the previous build's `defines` so a
  *removed* target still pulls in its linkers.
- **Per-page `render_key`** = `H(content ⊕ resolved-links ⊕ config ⊕ template)`. If a
  page's render key is unchanged, its on-disk output is already correct and it is skipped.
  The config component folds in a **site-structure hash** (every page's `(path, title)`),
  because the shared nav bar is global chrome — a title change or a page add/remove alters
  the nav on every page and so must re-render them all (otherwise byte-equivalence breaks).
- **Persisted cache manifest** (`<out>/.org-ssg-cache.json`, JSON), carrying per-page
  records, the config/template hashes, and the serialized dependency graph, tagged with
  `CACHE_FORMAT_VERSION`. A version mismatch, a missing file, or a corrupt file all fall
  back to a clean full rebuild — the cache is an optimization, never a correctness
  dependency.
- **Wired into `build_site`**: only pages whose render key changed (or that link into a
  changed file's targets) are re-rendered; unchanged outputs are left in place. `--no-cache`
  forces a full rebuild; `clean <out-dir>` removes the output directory (and its cache).
  `SiteReport` now reports `rendered` vs `skipped` counts.

The hard gates are enforced by `tests/incremental.rs`: full-vs-incremental **byte
equivalence** (and a second unchanged build re-rendering **zero** pages); **edit-one-file**
re-renders exactly the changed page plus its linkers; **renamed-heading** invalidates the
linking page and updates its emitted anchor; and cache **version-bump / missing / corrupt**
all fall back to a full rebuild.

**Out of scope in v0.3:** real syntect highlighting; timestamps and TODO keywords (all
landed in v0.4). `watch` is a minimal mtime poll loop (`watch <src-dir> -o <out-dir>`), not
an OS file-watcher — the fs-notify integration is deferred. The parse-tree cache (spec §4.5,
"optionally") is not persisted: PARSE/INDEX/RESOLVE run for every file each build (cheap and
pure); the incremental win is on RENDER + EMIT.

### v0.4 in / out — the MVP

v0.4 closes the gap between the v1 scope above and what the code actually did, so every
construct the IN list claims is now parsed, rendered, and pinned by a golden file:

- **Heading metadata** — TODO keywords (the Emacs default `TODO`/`DONE` set, matched on a
  word boundary so `TODOs` is not one) and `[#A]` priority cookies, rendered with Emacs'
  own export classes so the output stays diffable against an `emacs --batch` oracle.
- **Lists** — indentation-based nesting (a sub-list renders *inside* its parent `<li>`),
  multi-paragraph item bodies, and `term :: definition` description lists as `<dl>`.
- **Blocks by type** — `QUOTE`, `CENTER`, `EXAMPLE`, `EXPORT` and `SRC` are now distinct
  elements rather than all collapsing to a verbatim example block. Block matching is on the
  specific kind, so a source block can nest inside a quote. An `html` export block passes
  through; every other backend drops.
- **Timestamps** — active `<...>` and inactive `[...]`, optional times, same-day time
  ranges and `--`-joined date ranges, rendered as `<time>` with a machine-readable
  `datetime`. Repeater/warning cookies are recognized and discarded.
- **Images** — a description-less link to an image file renders as `<img>`; with an
  affiliated `#+CAPTION:`/`#+ATTR_HTML:` it is promoted to a `<figure>` with the caption as
  both `<figcaption>` and alt text. Links to non-`.org` files are now understood as asset
  links: neither resolved nor reported as broken.
- **Syntax highlighting** — real syntect tokenizing to CSS classes (never inline styles, so
  themes live in the stylesheet). Every build emits the matching `syntax.css` and each page
  links it relative to its own depth. An unknown language degrades to escaped `<pre><code>`.
- **Diagnostics** — broken links are reported as the org syntax the author wrote
  (`warning: b.org: unresolved link [[#setup]]`) rather than a Debug-printed enum.

**The OUT line is now enforced, not just asserted.** `tests/constructs.rs` pins each
excluded construct to a specific degradation: babel is never executed *and* a checked-in
`#+RESULTS:` block is dropped rather than published as if it were verified output;
`#+TBLFM:` is inert; `#+INCLUDE:` is never expanded; LaTeX, macros and radio targets survive
as literal text; drawers other than PROPERTIES are captured and dropped; unmodelled block
types keep their content verbatim.

**Still out at v0.4:** rayon parallelism; parse errors carrying source locations; `#+TODO:`
per-file keyword sequences; planning lines (`SCHEDULED:`/`DEADLINE:`), which render as
ordinary paragraphs; fixed-width `: ` lines; and the `watch` fs-notify integration.

## Phase 0: the corpus audit and the Emacs oracle

The v1 scope was, by its own admission, *recommended* — a guess about which slice of org
matters. Phase 0 replaces both halves of that guess with a measurement: an audit that asks
what a real corpus actually uses, and an oracle that asks whether we render it the way
Emacs does. The corpus is the 179 files behind [cleberg.net](https://cleberg.net), which is
published today by weblorg — a wrapper around org's own HTML exporter. That makes it both
the workload and the incumbent.

```
cargo run -- audit <src-dir>   # what does this corpus use, and is it in scope?
cargo test --test oracle       # how does our HTML differ from Emacs' own export?
```

### What the audit found

**The scope guess was sound.** 99.9% of construct uses in the corpus are in scope. The
whole out-of-scope tail is 8 uses: four `#+TBLFM:` in a post *about* org-mode, three
`\name` entities, and one `#+BEGIN_NOTE`.

**`#+SLUG:` was a hole big enough to sink the project.** 178 of 179 files set it, and the
published URL comes from it, not from the filename: `2018-11-28-aes-encryption.org` is
served at `blog/aes-encryption.html`. org-ssg derived output paths from source filenames,
so **169 of 179 pages would have been published at the wrong URL** — every inbound link and
every search result, broken, by a tool that reported a clean build. Output paths now come
from `#+SLUG:` when present ([`util::output_path`](src/util.rs)); slugs are sanitized so an
author-supplied `../../etc/x` cannot escape the output directory, and two pages claiming one
URL is a build error rather than a silently dropped page. Building the real corpus now
reproduces all 179 of the live site's URLs exactly.

**Some machinery is speculative.** The corpus contains no `id:`, `#custom-id` or `*Heading`
links at all — its cross-page links are hand-written relative URLs. The INDEX/RESOLVE
symbol table that v0.2 was built around is, against this corpus, unexercised.

**An audit can lie too.** The first run reported 23 uses of a custom TODO keyword sequence.
All 23 were false: the detector read the leading word of `* CSS Variables` as the keyword
`CSS`. The corpus defines no `#+TODO:` sequences at all, so the true count was zero. The
detector now matches conventional keyword names only — a tool that overstates a gap argues
for work nobody needs.

### What the oracle found

`tests/oracle.rs` exports each fixture with org's own exporter via `emacs --batch`, reduces
both sides to a semantic skeleton (element opens, closes and text, with layout `div`s,
inline `span`s and all attributes but `href`/`src` dropped), and **snapshots the
disagreement**. Snapshotting rather than asserting is deliberate: a checked-in divergence
report gets reviewed and shows up as a diff, where a permanently red test gets ignored.
Three invariants are asserted outright, and all three hold — heading structure, list
nesting, and source-block text match Emacs exactly.

**No bugs in org-ssg.** Every remaining divergence is a deliberate choice to emit better
HTML than org does:

| | org-ssg | Emacs | why |
|---|---|---|---|
| emphasis | `<em>`/`<strong>` | `<i>`/`<b>` | semantic, not presentational |
| captioned image | `<figure>`/`<figcaption>` | `<p>` + `"Figure 1: …"` | real figure semantics |
| timestamp | `<time datetime="…">` | literal `<2024-01-15 Mon>` | machine-readable |
| footnotes | `<section><ol>` | `<h2>Footnotes:</h2>` | a list of notes is a list |
| heading anchor | slug of the text | `org1a2b3c4` | stable, and what the live site serves |
| code | `<pre><code>` | `<pre>` | the HTML5 idiom |

One genuine semantic difference: org treats a single blank line between a `1.` list and a
`-` list as *one* list and keeps the first item's bullet type, while we start a second list.
We keep ours, on measurement rather than taste — the pattern occurs **zero** times in the
corpus, so matching an org quirk would buy nothing and cost the more obvious reading.

**The oracle's best catch was three bugs in itself.** Naive normalization reported code as
corrupted (it trimmed each of syntect's per-token text runs, turning `def greet` into
`defgreet`) and reported blocks at 36% agreement (syntect's spans flooded the diff). Both
were measurement artifacts. A differential harness is a piece of software like any other,
and the first divergences it reports are usually its own.

**From v0.1 (core subset):** headings with nesting and anchors (every heading is now
anchored — `:CUSTOM_ID:`/`:ID:` else a slug of its text) and trailing tags; paragraphs;
plain lists (unordered + ordered) with checkboxes; source blocks; inline markup (`*bold*`,
`/italic/`, `_underline_`, `+strike+`, `=verbatim=`, `~code~`); links and bare URLs.

## Dependencies

Parser is hand-written recursive descent (not `nom`/`chumsky`/`pest` — org is
line-oriented and context-sensitive, not clean CFG). Key crates: `syntect` (syntax
highlighting, behind a `Highlighter` trait so tree-sitter can be swapped in later),
`minijinja` (runtime templates), `blake3` (content/cache hashing), `chrono`,
`camino`, `walkdir`, `clap`, `anyhow`/`thiserror`. `insta` for snapshot tests.

## Build & test

```
cargo build
cargo test
cargo run -- build fixtures/minimal.org -o minimal.html   # single file
cargo run -- build fixtures/site -o _site                 # whole site (incremental)
cargo run -- audit fixtures/site                          # corpus audit (Phase 0)
cargo run -- build fixtures/site -o _site --no-cache      # force a full rebuild
cargo run -- watch fixtures/site -o _site                 # poll + rebuild on change
cargo run -- clean _site                                  # remove output + cache
```

A second `build` of an unchanged site re-renders nothing; editing a page re-renders only
that page and the pages that link into it (watch the `rendered`/`cached` counts).

A build emits `syntax.css` next to its output (the highlighter emits CSS classes, so the
stylesheet has to come with them) and every page links it.

`fixtures/` holds tiny `.org` samples: the core ones (`minimal.org`, `core.org`,
`elements.org`, `table.org`, `footnote.org`), one per v1 construct group (`headings.org`,
`lists.org`, `blocks.org`, `timestamps.org`, `images.org`), the scope guardrail
(`outofscope.org`), and a linked multi-file site under `fixtures/site/` (`index.org`,
`guide.org`, `about.org` + a `style.css` asset). The real corpus (golden files derived from
actual documents) lands in Phase 0. `cargo test` runs `insta` snapshots of the element tree
and rendered HTML for each fixture, the two templated site pages (proving cross-file link
resolution), and the incremental gates.
