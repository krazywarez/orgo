//! User-facing build configuration (`orgo.toml`).
//!
//! Everything here was once a constant in the source: the page layout, the nav rule, the
//! highlighting theme. That made the generator produce exactly one kind of site — a
//! reasonable place to start from, and a dead end for anyone whose site is not that one.
//!
//! Two properties matter beyond the settings themselves:
//!
//! 1. **Absent config is a valid config.** Every field has a default, so a directory of
//!    `.org` files with no `orgo.toml` still builds. Configuration is how you change
//!    the output, never how you make it work at all.
//! 2. **Config is a hash input** (spec §4.1). [`Config`] serializes deterministically and
//!    its hash is folded into every page's render key, so editing `orgo.toml` re-renders
//!    exactly the pages it affects — which for most settings is all of them.

use anyhow::{Context, Result};
use camino::{Utf8Path, Utf8PathBuf};
use serde::{Deserialize, Serialize};

/// The config file's name, looked for in the source directory.
pub const CONFIG_FILE: &str = "orgo.toml";

/// Resolved build configuration. Serialized into the config hash, so field order and
/// defaults are part of the cache contract.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub site: Site,
    pub nav: Nav,
    pub templates: Templates,
    pub highlight: Highlight,
    pub html: HtmlOutput,
    pub build: Build,
    /// Generated listing pages. Each produces one output file that has no source `.org`
    /// file behind it — a blog index, an archive, a feed.
    pub collections: Vec<Collection>,
    /// Which layout authored pages render through, by source path. Pages matching no
    /// rule use `base.html`.
    pub pages: Vec<PageRule>,
}

/// One layout rule: the pages under `match` render through `template`.
///
/// Sections usually want one layout — every blog post carries the same byline and reply
/// footer — and asking an author to repeat `#+TEMPLATE:` in each of 200 files is asking
/// them to maintain the same fact 200 times. A rule states it once for the directory; a
/// page that differs still says so itself with `#+TEMPLATE:`, which wins.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PageRule {
    /// A source path: a directory, matching every page beneath it, or one `.org` file.
    /// Relative to the source root, like `nav.pages`.
    #[serde(rename = "match")]
    pub pattern: Utf8PathBuf,
    /// Template file name, as it appears in the templates directory.
    pub template: String,
}

impl PageRule {
    /// Does this rule cover `source`? Matching is by path component, so a directory rule
    /// covers everything beneath it however deep — `blog` matches `blog/2026/post.org`,
    /// because a section's layout is a property of the section and not of how its files
    /// happen to be filed — while `blo` matches nothing. An empty `match` covers the
    /// whole site, which is how you change the default layout's name.
    pub fn covers(&self, source: &Utf8Path) -> bool {
        source.starts_with(&self.pattern)
    }

    /// How specific this rule is, for picking between two that both match. Longer paths
    /// are more specific, so `blog/notes` beats `blog`.
    fn specificity(&self) -> usize {
        self.pattern.components().count()
    }
}

/// Which template an authored page renders through: its own `#+TEMPLATE:` if it names
/// one, else the most specific `[[pages]]` rule covering it, else `base.html`.
///
/// A page's own declaration wins because it is the more local statement — the one written
/// with that page in view.
pub fn page_template(config: &Config, source: &Utf8Path, keywords: &crate::model::Keywords) -> String {
    let declared = keywords
        .entries
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("TEMPLATE"))
        .map(|(_, v)| v.trim())
        .filter(|v| !v.is_empty());
    if let Some(name) = declared {
        return name.to_string();
    }
    config
        .pages
        .iter()
        .filter(|rule| rule.covers(source))
        .max_by_key(|rule| rule.specificity())
        .map(|rule| rule.template.clone())
        .unwrap_or_else(|| crate::template::BASE_TEMPLATE_NAME.to_string())
}

/// A generated page that lists other pages.
///
/// This is the one output that is not a translation of some input: a blog index exists
/// because a set of posts exists, not because someone wrote `index.org`. Keeping it
/// declarative — a directory in, a file out, through a template — means a feed is the
/// same mechanism with an XML template rather than a second feature.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Collection {
    /// Directory of source pages to list, relative to the source root. Empty means every
    /// page in the site.
    pub source: Utf8PathBuf,
    /// Where to write the generated page, relative to the output root.
    pub output: Utf8PathBuf,
    /// Template file name, as it appears in the templates directory.
    pub template: String,
    /// Title for the generated page, available to the template as `page.title`.
    pub title: String,
    /// Split the collection into groups and emit one page per group.
    ///
    /// `"tags"` groups by `#+FILETAGS:`, where a page belongs to every tag it carries.
    /// Any other value names a `#+KEYWORD:` and groups by its value, so `"category"`
    /// buckets pages by `#+CATEGORY:`. Empty means one page for the whole collection.
    ///
    /// When set, `output` and `title` may contain `{tag}`, replaced by the group — and
    /// `output` must, or every group would write to the same file.
    pub group_by: String,
    /// Where to write a page listing the groups themselves — a tag index. Empty means
    /// no such page. Only meaningful with `group_by`.
    pub index_output: Utf8PathBuf,
    /// Template for the group-index page. It receives `groups` rather than `pages`.
    pub index_template: String,
    pub index_title: String,
    pub sort: SortKey,
    pub order: SortOrder,
    /// Entries per page. `0` means no pagination — the whole collection on one page.
    ///
    /// Page 1 stays at `output`, so the canonical URL of a section never moves when the
    /// number of pages changes. Pages 2 and up go to `paginate_output`.
    pub paginate: usize,
    /// Where pages 2..N are written. Must contain `{n}`, the 1-based page number, and
    /// `{tag}` as well when the collection is grouped — otherwise page 2 of one group
    /// would overwrite page 2 of another.
    pub paginate_output: Utf8PathBuf,
    /// Give the template each entry's rendered HTML as `entry.content`.
    ///
    /// Off by default, and only worth turning on for a feed: it renders every listed
    /// page's body whenever the listing is rebuilt. A reader subscribed to a
    /// full-content feed and then handed excerpts has lost something, which is the one
    /// case where that cost is the right trade.
    pub include_content: bool,
    /// Add this listing page to the site navigation. This is how a section landing page
    /// — `/blog/`, `/notes/` — gets into a nav built from top-level pages.
    pub nav: bool,
}

impl Default for Collection {
    fn default() -> Self {
        Collection {
            source: Utf8PathBuf::new(),
            output: Utf8PathBuf::from("index.html"),
            template: "list.html".to_string(),
            title: "Index".to_string(),
            group_by: String::new(),
            index_output: Utf8PathBuf::new(),
            index_template: "tags.html".to_string(),
            index_title: "Tags".to_string(),
            sort: SortKey::default(),
            order: SortOrder::default(),
            paginate: 0,
            paginate_output: Utf8PathBuf::new(),
            include_content: false,
            nav: false,
        }
    }
}

/// The `{tag}` placeholder in a grouped collection's `output` and `title`.
pub const GROUP_PLACEHOLDER: &str = "{tag}";
/// The page-number placeholder in `paginate_output`.
pub const PAGE_PLACEHOLDER: &str = "{n}";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SortKey {
    /// By `#+DATE:`, newest first by default. Pages with no parseable date sort last,
    /// keeping undated drafts out of the way of a dated archive.
    #[default]
    Date,
    Title,
    /// Output path — stable and predictable when dates are absent or unreliable.
    Path,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SortOrder {
    /// Newest or last first — the useful default for a blog.
    #[default]
    Desc,
    Asc,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Build {
    /// Include pages marked `#+DRAFT:` in the build.
    ///
    /// Off by default, because the point of marking something a draft is that it is not
    /// ready to be read. `--drafts` turns it on for a session, which is what you want
    /// under `watch` while writing one.
    pub drafts: bool,
    /// Extra directories whose contents are copied to the *site root*, on top of the
    /// non-`.org` files found in the source directory. Relative to the source root, and
    /// allowed to point outside it.
    ///
    /// This exists because a site's static files do not always live where its writing
    /// does: weblorg publishes `theme/static/` to `/`, and a repository migrating from it
    /// should not have to move `robots.txt` next to its blog posts to keep the URL.
    pub assets: Vec<Utf8PathBuf>,
    /// Write `sitemap.xml` listing every published page.
    ///
    /// On, but a sitemap requires absolute URLs — the format has nowhere to put a
    /// relative one — so nothing is written until `site.base_url` is set. That is why a
    /// zero-config build produces no sitemap and no complaint: there is no URL to give a
    /// search engine yet.
    pub sitemap: bool,
}

impl Default for Build {
    fn default() -> Self {
        Build {
            drafts: false,
            assets: Vec::new(),
            sitemap: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct HtmlOutput {
    /// How far to push heading levels down: a level-1 org heading becomes
    /// `<h{1 + heading_offset}>`.
    ///
    /// Defaults to 1, matching Emacs' own `org-html-toplevel-hlevel`, because the page
    /// layout supplies the `<h1>` — the document's title — and section headings sit
    /// beneath it. Set to 0 if your template renders no title of its own, so the
    /// document does not start at `<h2>` with nothing above it.
    pub heading_offset: u8,
    /// Make each page's table of contents available to templates as `page.toc`.
    ///
    /// On by default: it is *data*, and whether it appears is the template's business.
    /// A document turns it off for itself with org's own `#+OPTIONS: toc:nil`, which is
    /// how ~2% of the reference corpus does it.
    pub toc: bool,
    /// Number headings, `1.`, `1.1.`, and so on.
    ///
    /// Off by default, which differs from Emacs — `org-export-with-section-numbers` is
    /// on there, and the reference site inherits numbered headings from it. Most sites
    /// do not want them, so the default is the taste rather than the inheritance;
    /// `#+OPTIONS: num:t` or `section_numbers = true` gets Emacs' behaviour back.
    pub section_numbers: bool,
    /// Convert org's special strings in prose: `--` to an en dash, `---` to an em dash,
    /// `...` to an ellipsis.
    ///
    /// On, as in Emacs. A document turns it off for itself with `#+OPTIONS: -:nil`.
    /// Never applied inside verbatim, code, or a source block.
    pub special_strings: bool,
    /// Whether `x^2` and `H_{2}O` become `<sup>`/`<sub>`.
    ///
    /// `"yes"` (the default, and Emacs') also treats the braceless `a_b` as a subscript,
    /// which is what makes `snake_case` in prose render as `snake<sub>case</sub>` —
    /// surprising, but what Emacs does with the same file. `"braces"` limits it to the
    /// explicit `a_{b}` form, and `"no"` leaves both alone. A document chooses for itself
    /// with `#+OPTIONS: ^:nil` or `^:{}`.
    pub sub_superscript: SubSuperscript,
    /// Whether `\alpha` and the rest of org's entity table become their characters.
    ///
    /// On, as in Emacs. A name org does not know is left as the literal text that was
    /// typed. A document turns the whole thing off with `#+OPTIONS: e:nil`.
    pub entities: bool,
}

/// How `_` and `^` are treated in prose. Mirrors org's `^:` export option.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SubSuperscript {
    /// `a_b` and `a_{b}` both convert.
    #[default]
    Yes,
    /// Only the braced `a_{b}` converts.
    Braces,
    /// Neither converts.
    No,
}

impl SubSuperscript {
    /// Read org's `^:` option value: `nil` is off, `{}` is braces-only, anything else on.
    pub fn from_option(value: &str) -> SubSuperscript {
        match value.trim() {
            "nil" | "false" | "no" | "off" => SubSuperscript::No,
            "{}" => SubSuperscript::Braces,
            _ => SubSuperscript::Yes,
        }
    }
}

impl Default for HtmlOutput {
    fn default() -> Self {
        HtmlOutput {
            heading_offset: 1,
            toc: true,
            section_numbers: false,
            special_strings: true,
            sub_superscript: SubSuperscript::Yes,
            entities: true,
        }
    }
}

/// Site-wide metadata, exposed to templates as `site`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Site {
    /// Shown in the default layout's header and available as `site.title`.
    pub title: String,
    /// Absolute base URL (no trailing slash), for feeds and canonical links. Empty means
    /// the site is built with relative URLs only, which is the portable default.
    pub base_url: String,
    /// Free-form description, available as `site.description`.
    pub description: String,
    /// `<html lang="…">` in the default layout.
    pub language: String,
    /// A built-in theme name — see [`crate::theme::THEMES`] — written to the output root
    /// as `theme.css` and linked by the built-in layout and the starter templates.
    ///
    /// Empty by default, which emits no stylesheet and leaves the HTML unstyled. A theme
    /// is a convenience for a site that has not grown its own CSS yet, and defaulting
    /// one on would restyle every existing site on upgrade and fight the stylesheets
    /// people already ship as assets.
    pub theme: String,
}

impl Default for Site {
    fn default() -> Self {
        Site {
            title: "orgo site".to_string(),
            base_url: String::new(),
            description: String::new(),
            language: "en".to_string(),
            theme: String::new(),
        }
    }
}

/// Which pages appear in the shared navigation.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum NavMode {
    /// Pages at the site root. A nav is a map of the top level, not an index of the
    /// whole site, and this keeps nav size independent of how many pages exist.
    #[default]
    TopLevel,
    /// Every page. Fine for a small site; note that it makes total output quadratic in
    /// page count, since each of `n` pages then carries `n` nav links.
    All,
    /// Only the pages listed in `nav.pages`, in that order.
    Explicit,
    /// No navigation at all.
    None,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Nav {
    pub mode: NavMode,
    /// Source paths (relative to the source root, e.g. `about.org`) used when
    /// `mode = "explicit"`. Order is preserved, so this doubles as nav ordering.
    pub pages: Vec<Utf8PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Templates {
    /// Directory of `.html` templates, relative to the source root. Each file is
    /// registered under its stem, so `base.html` overrides the built-in layout and
    /// anything else is available to `{% include %}`/`{% extends %}`.
    pub dir: Utf8PathBuf,
    /// Give templates a `pages` list of every page's metadata, so a template can build
    /// an index or archive.
    ///
    /// Off by default because it is not free: if any page can read every page's
    /// metadata, then adding one page can change any page's output, so the whole site
    /// must re-render on every add, rename or retitle. Turning this on trades that
    /// incremental precision for the ability to write listing pages.
    pub expose_page_list: bool,
}

impl Default for Templates {
    fn default() -> Self {
        Templates {
            dir: Utf8PathBuf::from("templates"),
            expose_page_list: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Highlight {
    /// Directory of extra `.sublime-syntax` files, relative to the source root.
    ///
    /// syntect bundles a long list of languages and this crate adds TOML and Org, but a
    /// missing language should not need a new release — drop a definition here and it is
    /// picked up. A file that fails to parse is reported and skipped.
    pub syntaxes_dir: Utf8PathBuf,
    /// A syntect built-in theme name — `InspiredGitHub`, `Solarized (dark)`,
    /// `base16-ocean.dark`, `base16-eighties.dark`, `base16-mocha.dark`,
    /// `base16-ocean.light`. Highlighting emits CSS classes, and this theme is what the
    /// generated `syntax.css` colours them with.
    pub theme: String,
    /// A second syntect theme for readers whose system asks for dark mode. `syntax.css`
    /// then carries both, each behind its own `prefers-color-scheme` query, so one
    /// stylesheet serves both schemes — see [`crate::render::syntax_stylesheet`]. Empty,
    /// the default, means `theme` colours every reader whatever their scheme.
    pub theme_dark: String,
}

impl Default for Highlight {
    fn default() -> Self {
        Highlight {
            syntaxes_dir: Utf8PathBuf::from("syntaxes"),
            theme: "InspiredGitHub".to_string(),
            theme_dark: String::new(),
        }
    }
}

impl Config {
    /// Load `orgo.toml` from `dir`, or return defaults if there is none.
    ///
    /// A *missing* config is normal and silent. A *malformed* one is an error: someone
    /// who wrote a config meant it, and silently building the default site would hide
    /// their typo behind plausible-looking output.
    pub fn load(dir: &Utf8Path) -> Result<Config> {
        Self::load_file(&dir.join(CONFIG_FILE))
    }

    /// Load a config from an explicit path. Missing is still fine; malformed is not.
    pub fn load_file(path: &Utf8Path) -> Result<Config> {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Config::default()),
            Err(e) => return Err(e).with_context(|| format!("reading {path}")),
        };
        toml::from_str(&text).with_context(|| format!("parsing {path}"))
    }

    /// Validate settings that only make sense in combination. Catching these up front
    /// beats emitting a site with a silently empty nav.
    pub fn validate(&self) -> Result<()> {
        if self.nav.mode == NavMode::Explicit && self.nav.pages.is_empty() {
            anyhow::bail!(
                "nav.mode is \"explicit\" but nav.pages is empty: list the pages to \
                 include, or use mode = \"top-level\"/\"all\"/\"none\""
            );
        }
        if self.nav.mode != NavMode::Explicit && !self.nav.pages.is_empty() {
            anyhow::bail!(
                "nav.pages is set but nav.mode is \"{}\", so it would be ignored; set \
                 mode = \"explicit\" to use it",
                toml::to_string(&self.nav.mode)
                    .unwrap_or_default()
                    .trim()
                    .trim_matches('"')
            );
        }
        let mut seen: Vec<&Utf8PathBuf> = Vec::new();
        for collection in &self.collections {
            let grouped = !collection.group_by.is_empty();
            if collection.output.as_str().is_empty() && collection.index_output.as_str().is_empty()
            {
                anyhow::bail!("a collection has no `output`; it needs a file to write");
            }
            if grouped
                && !collection.output.as_str().is_empty()
                && !collection.output.as_str().contains(GROUP_PLACEHOLDER)
            {
                anyhow::bail!(
                    "collection output {} groups by \"{}\" but has no {GROUP_PLACEHOLDER} in \
                     its path, so every group would overwrite the same file",
                    collection.output,
                    collection.group_by
                );
            }
            if !grouped && collection.output.as_str().contains(GROUP_PLACEHOLDER) {
                anyhow::bail!(
                    "collection output {} uses {GROUP_PLACEHOLDER} but sets no `group_by`",
                    collection.output
                );
            }
            if !grouped && !collection.index_output.as_str().is_empty() {
                anyhow::bail!(
                    "collection writes an `index_output` of {} but sets no `group_by`; \
                     there are no groups to index",
                    collection.index_output
                );
            }
            if collection.paginate > 0 {
                let pattern = collection.paginate_output.as_str();
                if pattern.is_empty() {
                    anyhow::bail!(
                        "collection output {} sets `paginate` but no `paginate_output`;                          pages 2 and up need somewhere to go, e.g. \"blog/page/{PAGE_PLACEHOLDER}.html\"",
                        collection.output
                    );
                }
                if !pattern.contains(PAGE_PLACEHOLDER) {
                    anyhow::bail!(
                        "collection `paginate_output` {pattern} has no {PAGE_PLACEHOLDER},                          so every page after the first would overwrite the same file"
                    );
                }
                if grouped && !pattern.contains(GROUP_PLACEHOLDER) {
                    anyhow::bail!(
                        "collection `paginate_output` {pattern} groups by \"{}\" but has no                          {GROUP_PLACEHOLDER}, so page 2 of one group would overwrite page 2                          of another",
                        collection.group_by
                    );
                }
            }
            if collection.paginate == 0 && !collection.paginate_output.as_str().is_empty() {
                anyhow::bail!(
                    "collection sets `paginate_output` {} but `paginate` is 0, so it would                      never be used; set `paginate` to a page size",
                    collection.paginate_output
                );
            }
            for path in [&collection.output, &collection.index_output] {
                if path.as_str().is_empty() || path.as_str().contains(GROUP_PLACEHOLDER) {
                    continue;
                }
                if seen.contains(&path) {
                    anyhow::bail!(
                        "two collections both write to {path}; give them different \
                         `output` paths"
                    );
                }
                seen.push(path);
            }
        }
        for rule in &self.pages {
            if rule.template.trim().is_empty() {
                anyhow::bail!(
                    "the [[pages]] rule matching {:?} names no `template`; it has nothing \
                     to select",
                    rule.pattern.as_str()
                );
            }
        }
        if !self.site.theme.is_empty() && crate::theme::theme_css(&self.site.theme).is_none() {
            anyhow::bail!(
                "unknown site.theme {:?}. Available: {} — or leave it empty for no \
                 stylesheet",
                self.site.theme,
                crate::theme::available_themes().join(", ")
            );
        }
        if !self.site.base_url.is_empty() && self.site.base_url.ends_with('/') {
            anyhow::bail!(
                "site.base_url must not end with a slash (got {:?}) — URLs are joined \
                 with an explicit separator",
                self.site.base_url
            );
        }
        Ok(())
    }
}

/// The starter config written by `orgo init`, and the documentation of record for
/// what is configurable. Every value shown is the default — except `site.theme`, which
/// picks a stylesheet so a new site looks like something on its first build — so
/// deleting any line is safe.
pub const STARTER_CONFIG: &str = r#"# orgo configuration. Every setting here is optional and shown at its default — apart
# from `theme`, noted below — so you can delete any line you do not need, or the whole
# file.

[site]
title = "orgo site"
# Absolute base URL, no trailing slash. Needed for feeds and canonical links, which
# cannot be relative — set it and uncomment the [[collections]] feed block below.
base_url = ""
description = ""
language = "en"
# A built-in stylesheet, written to the output as theme.css: "plain" (readable defaults
# to build your own CSS on), "blog" (serif prose, masthead, styled post lists), "wiki"
# (wide and dense, contents in the margin, TODO states shown) or "docs" (a guide read in
# order). The one line here that is not a default: the default is "", which emits no
# stylesheet at all. Your own base.html can ignore theme.css and link whatever it likes.
theme = "blog"

[nav]
# Which pages appear in the shared navigation:
#   "top-level" — pages at the site root (default; keeps nav size independent of site size)
#   "all"       — every page (fine when small; output grows quadratically with page count)
#   "explicit"  — only nav.pages, in the order listed
#   "none"      — no navigation
mode = "top-level"
# pages = ["index.org", "about.org"]

[templates]
# Directory of .html templates, relative to this file. `base.html` replaces the built-in
# layout; any other file can be pulled in with {% include %} or {% extends %}.
dir = "templates"
# Give templates a `pages` list of every page's metadata, so you can build an index or
# archive. Costs incremental precision: with this on, adding a page re-renders the site.
expose_page_list = false

# Which layout a page renders through. Without a rule, every page uses base.html.
# `match` is a source path — a directory (covering everything beneath it) or one .org
# file — and the most specific rule wins. A page overrides any rule with `#+TEMPLATE:`.
# [[pages]]
# match = "blog"
# template = "post.html"

[highlight]
# A syntect theme name: InspiredGitHub, Solarized (dark), base16-ocean.dark,
# base16-eighties.dark, base16-mocha.dark, base16-ocean.light.
theme = "InspiredGitHub"
# A second theme for readers in dark mode. Set it and syntax.css carries both themes,
# each behind its own prefers-color-scheme query. Empty means `theme` colours everyone.
theme_dark = ""
# Extra .sublime-syntax files for languages neither syntect nor orgo bundles.
syntaxes_dir = "syntaxes"

[build]
# Include pages marked `#+DRAFT:`. Off by default — the point of marking a draft is that
# it is not ready to be read. `--drafts` turns it on for one run, handy under `watch`.
drafts = false
# Extra directories copied to the site root, for static files that live outside the
# source directory. `assets = ["../theme/static"]` publishes that directory's contents at
# `/`, not at `/static/`.
assets = []
# Write sitemap.xml. Needs site.base_url — a sitemap has nowhere to put a relative URL —
# so nothing is written until you set one.
sitemap = true

[html]
# How far to push heading levels down: a level-1 org heading becomes <h(1 + offset)>.
# The default of 1 matches Emacs, and assumes your layout renders the page title as the
# <h1>. Set to 0 if your template renders no title of its own.
heading_offset = 1
# Make page.toc available to templates. A document opts out with `#+OPTIONS: toc:nil`.
toc = true
# Number headings (1., 1.1., …). Emacs defaults this on; most sites do not.
# A document overrides with `#+OPTIONS: num:t`.
section_numbers = false
# Convert `--` to an en dash, `---` to an em dash and `...` to an ellipsis in prose, as
# Emacs does. Never inside code. A document overrides with `#+OPTIONS: -:nil`.
special_strings = true
# Whether `x^2` and `H_{2}O` become <sup>/<sub>: "yes" (as Emacs, and so `snake_case`
# becomes snake<sub>case</sub>), "braces" for the `a_{b}` form only, or "no".
# A document overrides with `#+OPTIONS: ^:nil` or `^:{}`.
sub_superscript = "yes"
# Convert `\alpha` and the rest of org's entity table. An unknown name stays literal.
# A document overrides with `#+OPTIONS: e:nil`.
entities = true

# Generated listing pages: output files with no source .org behind them. Repeat the
# [[collections]] block for each one. A feed is the same thing with an XML template.
[[collections]]
source = "blog"            # directory to list; empty means every page
output = "blog/index.html" # where to write it
template = "list.html"     # template file name
title = "Blog"
sort = "date"              # date | title | path
order = "desc"             # desc | asc
nav = true                 # put this listing page in the site nav
# paginate = 10            # entries per page; page 1 stays at `output`
# paginate_output = "blog/page/{n}.html"   # where pages 2..N go; needs {n}

# An RSS feed is a listing page with an XML template. It needs site.base_url above,
# because a feed is read away from the site that served it and relative links break.
# [[collections]]
# source = "blog"
# output = "feed.xml"
# template = "feed.xml"
# title = "Feed"

# One page per tag, plus an index of all tags. `{tag}` in `output`/`title` is replaced
# by each tag; the index gets `groups` instead of `pages`.
[[collections]]
source = "blog"
group_by = "tags"            # "tags", or any #+KEYWORD: name to group by its value
output = "tags/{tag}.html"
template = "list.html"
title = "Tagged: {tag}"
index_output = "tags/index.html"
index_template = "tags.html"
index_title = "Tags"
nav = true                   # adds the tag *index*, not every tag
"#;
