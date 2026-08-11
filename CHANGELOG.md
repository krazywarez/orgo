# Changelog

What changed and why, newest first. Entries name the *behaviour* that moved, since that is
what a rebuild will show you.

Two conventions worth knowing before reading:

- **A cache-format bump is not a change you need to act on.** The incremental cache is
  versioned and discards itself; a bump means the next build re-renders everything once.
- **Output changes are called out.** org-ssg aims at what Emacs exports from the same
  file, so an entry that says "now renders X" means your pages will change. That is the
  product, not a regression — but it belongs in a changelog rather than a diff you find
  later.

Versions follow the compatibility promise in the README: config keys, template variables,
CLI flags and URLs are the stable surface.

## 0.19.1

- Footnote back-links carry `aria-label="Back to reference N"`, and the notes section is
  labelled. A link whose only visible content is `↩` has that glyph as its whole
  accessible name, so a screen reader announced "left arrow with hook" once per note with
  no way to tell them apart.

## 0.19.0

- **Full-content collections.** `include_content = true` gives a listing template each
  entry's rendered HTML as `entry.content` — a feed that carries whole posts rather than
  excerpts. Rendered only when the listing is actually rebuilt, so a cached feed costs
  nothing.
- **Fixed: a listing could show a stale excerpt.** Its cache key covered a hand-picked set
  of fields, and the excerpt was not among them, so rewriting a post's first paragraph
  left the old text on the index until something unrelated invalidated it. Entries are now
  hashed through their serialization, which cannot drift from what a template can read.
  Editing a post's body now rebuilds the listings that show it.
- `page.toc` entries carry `number`, so a site with section numbering on can number its
  contents list to match its headings.

## 0.18.0

Release engineering, so that a version number is worth reading.

- **A written compatibility promise.** Config keys, template variables, CLI flags and URLs
  are the stable surface; the incremental cache, HTML details and the Rust API are not.
  In the README, and in the guide under *Versioning and upgrades*.
- **CI** on Linux and macOS: build, test, clippy as an error, and the documentation site
  built with `--strict`. Emacs is installed on both, so the oracle suite runs for real
  instead of skipping.
- **A checked MSRV**, 1.88 — which is how it came to be 1.88 rather than the 1.82
  org-ssg's own code needs. The floor comes from dependencies, and nobody finds that out
  by reasoning about it.
- **Release binaries** for macOS (arm64, x86_64) and Linux (gnu, musl), built on tag into
  a draft release. The tag is checked against `Cargo.toml` before anything is built.
- A `LICENSE` file to go with the MIT declaration, crates.io metadata, and a release
  profile that produces a 5.0 MB binary rather than 6.5 MB.
- This changelog, and `RELEASING.md`.

## 0.17.0

- **Asset directories outside the source.** `[build] assets = ["../theme/static"]` copies
  a directory's contents to the site root. A site's static files do not always live where
  its writing does, and copying them next to the writing is how a repository ends up with
  two of every stylesheet. `watch` and `serve` watch these directories too. Two files
  claiming one URL is a build error naming both.
- **Template hashing is per template.** A page's render key covered every template, so
  editing a feed template re-rendered the whole site. It now covers the layout the page
  uses plus what that layout extends, includes or imports. On a 196-page site, editing the
  feed template renders one page instead of 196.
- Cache format 7.

## 0.16.0

- **Org's entity table.** `\alpha`, `\rarr`, `20\deg` and the other 412 names, generated
  from Emacs' own `org-entities`. An unknown name stays literal; `#+OPTIONS: e:nil` turns
  the table off. *Output changes* for any page using entities.
- **Table captions.** `#+CAPTION:` above a table becomes a numbered `<caption>`.
- **`#+INCLUDE:` reports itself.** It was inert and silent, which publishes a page with
  content missing and nobody told. Now a diagnostic, and `--strict` makes it a failure.
- The Emacs oracle separates deliberate divergence from defects. Every difference from
  org's exporter is named and justified, and a test asserts there are no others.

## 0.15.0

Export parity, from a page-by-page diff of a 179-file corpus against the site Emacs
publishes from the same sources. **All of these change output.**

- Heading levels are relative to a document's shallowest heading, as org exports them.
- Org's text conversions: `--`, `---`, `...`, and `x^2` / `a_{b}`. Never inside verbatim,
  code, source blocks or LaTeX. `#+OPTIONS: -:nil`, `^:nil` and `^:{}` all work.
- Captioned figures are numbered `Figure N:`.
- A caption attaches to the element *directly* below it; a blank line between attaches to
  nothing.
- Checkboxes render as org writes them, which keeps the `[-]` partly-done state a disabled
  `<input>` could not express. `[@4]` sets a list item's number.
- A table's special marker column and its marker rows stay out of the output.
- `#+BEGIN_NOTE` and any other unrecognised name is a special block: a div holding parsed
  org rather than a `<pre>` of literal text. Verse keeps its line breaks.
- Emphasis borders forbid whitespace and nothing else, so `="proxied":false=` is verbatim
  and `~~/.config/doom/config.el~` is a path that starts with a tilde.
- Listings sort on the time of day when a timestamp carries one.
- Cache format 6.

## 0.14.0

- **Per-page layouts.** `[[pages]]` rules map a source path to a template, and
  `#+TEMPLATE:` on a page overrides any rule. A missing template fails the build naming
  the page, the template, and what does exist.
- `page.year`, for grouping a listing by year with minijinja's `groupby`.
- An explicit nav can order generated pages among authored ones. `nav.mode = "none"` now
  really means none.

## 0.13.0

- Bundled TOML and Org syntax definitions, a `syntaxes_dir` for your own, and org's comma
  escape (`,* heading` inside a block).

## 0.12.0

- `serve`: a development server with live reload, bound to loopback.
- A documentation site under `docs/`, built by org-ssg itself.

## 0.11.0

- Table of contents as `page.toc`, section numbers, and org's `#+OPTIONS:` per-file
  switches.

## 0.10.0

- Excerpts, word count, reading time, a `truncate` filter, and `#+DRAFT:` pages.

## 0.9.0

- `watch`: rebuilds on OS filesystem events, debounced.

## 0.8.0

- `site.base_url`, the `absolute` and `rfc822` filters, canonical links, and an RSS feed
  in the scaffold that validates.

## 0.7.0

- Pagination for large listings, with a `paginator` template context that composes with
  grouping.

## 0.6.0

- Grouped collections: one page per tag plus a tag index.
- Generated listing pages (`[[collections]]`), sorted indexes, and feeds via XML
  templates.
- A config file, user templates, nav modes, an `init` scaffold, and discovery that will
  not publish `.git`.

## 0.5.0

- Parse diagnostics carry `file:line`, and pages render in parallel.
- The corpus audit (`org-ssg audit`) and the `emacs --batch` oracle.
- `#+SLUG:` decides a page's output filename — found by auditing a real corpus, where it
  affected 169 of 182 URLs.

## 0.4.0

- The full v1 construct scope, with the IN/OUT line under test.

## 0.3.0

- The incremental build layer: content, config and template hashing, a dependency graph,
  per-page render keys, and a persisted cache manifest. A full build and an incremental
  build produce byte-identical output.

## 0.2.0

- Multi-file site builds: a symbol table, internal link resolution, minijinja templates,
  tables and footnotes.

## 0.1.0

- Parse and render a single `.org` file to HTML.
