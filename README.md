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
| model | `src/model.rs` | The org element tree — Elements (block) vs Objects (inline). |
| INDEX | `src/index.rs` | Collect link targets into a symbol table. |
| RESOLVE | `src/resolve.rs` | Rewrite links to URLs; return the used-target list (dependency edges). |
| RENDER | `src/render.rs` | Tree → HTML fragment; syntect highlighting; footnote two-pass. |
| TEMPLATE | `src/template.rs` | minijinja: fragment + metadata → full page. |
| incremental | `src/incremental.rs` | Content/config/template hashing, dep graph, cache manifest, invalidation. |

## v1 scope (recommended; must be reconciled against a corpus audit first)

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

**Scope guardrail:** every IN item gets a golden-file fixture from a real document;
every OUT item gets a test asserting it degrades predictably (ignored, no crash). The
IN/OUT line is enforced by tests, defending against the project's #1 risk: scope creep
back toward all-of-org.

## Phase plan

| Phase | Scope | Status |
|---|---|---|
| **M0** | **Buildable skeleton: crate layout, module stubs, deps, test harness, fixtures** | **done** |
| **v0.1** | **End-to-end core parse → render: `build` a single `.org` file to HTML** | **done** |
| **v0.2** | **Multi-file SITE build: INDEX + RESOLVE internal links, minijinja templates, `build <src-dir> <out-dir>`, tables + footnotes** | **done** |
| **v0.3** | **Incremental build layer: content/config/template hashing, dependency graph, per-page render keys, persisted cache manifest, invalidation** | **done** |
| 0 | Corpus audit + `emacs --batch` ground-truth oracle | todo |
| 1 | Line lexer + heading/section skeleton | done |
| 2 | Block elements — lists, source blocks, tables, footnote defs done; generic drawers | partial |
| 3 | Inline objects — emphasis, links, bare URLs, footnote refs done; timestamps | partial |
| 4 | Rendering to HTML — tree walk, tables, footnote two-pass, minijinja templating done; real syntect highlighting | partial |
| 5 | Link resolution + symbol table (INDEX + RESOLVE, used-target list, broken-link reporting) | done |
| 6 | Incremental build layer (hashing, dep graph, invalidation) done; `watch` is a simple poll loop | done |
| 7 | Hardening: rayon parallelism, CLI polish, error locations | todo |

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

**Still stubbed (`todo!`):** timestamps; TODO keywords and priorities; generic
(non-PROPERTIES) drawers. Source-block syntax highlighting remains a `<pre><code>`
passthrough behind the `Highlighter` trait; real syntect tokenizing is deferred.

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

**Out of scope in v0.3 (unchanged from v0.2):** real syntect highlighting; timestamps and
TODO keywords. `watch` is a minimal mtime poll loop (`watch <src-dir> -o <out-dir>`), not an
OS file-watcher — the fs-notify integration is deferred. The parse-tree cache (spec §4.5,
"optionally") is not persisted: PARSE/INDEX/RESOLVE run for every file each build (cheap and
pure); the incremental win is on RENDER + EMIT.

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
cargo run -- build fixtures/site -o _site --no-cache      # force a full rebuild
cargo run -- watch fixtures/site -o _site                 # poll + rebuild on change
cargo run -- clean _site                                  # remove output + cache
```

A second `build` of an unchanged site re-renders nothing; editing a page re-renders only
that page and the pages that link into it (watch the `rendered`/`cached` counts).

`fixtures/` holds tiny `.org` samples: single-file ones (`minimal.org`, `core.org`,
`elements.org`, `table.org`, `footnote.org`) and a linked multi-file site under
`fixtures/site/` (`index.org`, `guide.org`, `about.org` + a `style.css` asset). The
real corpus (golden files derived from actual documents) lands in Phase 0. `cargo test`
includes `insta` snapshots of the element tree and rendered HTML for the single-file
fixtures, the two templated site pages (proving cross-file link resolution), and the
table and footnote constructs.
