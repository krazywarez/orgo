//! User-facing build configuration (`org-ssg.toml`).
//!
//! Everything here was once a constant in the source: the page layout, the nav rule, the
//! highlighting theme. That made the generator produce exactly one kind of site — a
//! reasonable place to start from, and a dead end for anyone whose site is not that one.
//!
//! Two properties matter beyond the settings themselves:
//!
//! 1. **Absent config is a valid config.** Every field has a default, so a directory of
//!    `.org` files with no `org-ssg.toml` still builds. Configuration is how you change
//!    the output, never how you make it work at all.
//! 2. **Config is a hash input** (spec §4.1). [`Config`] serializes deterministically and
//!    its hash is folded into every page's render key, so editing `org-ssg.toml` re-renders
//!    exactly the pages it affects — which for most settings is all of them.

use anyhow::{Context, Result};
use camino::{Utf8Path, Utf8PathBuf};
use serde::{Deserialize, Serialize};

/// The config file's name, looked for in the source directory.
pub const CONFIG_FILE: &str = "org-ssg.toml";

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
    /// Generated listing pages. Each produces one output file that has no source `.org`
    /// file behind it — a blog index, an archive, a feed.
    pub collections: Vec<Collection>,
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
pub struct HtmlOutput {
    /// How far to push heading levels down: a level-1 org heading becomes
    /// `<h{1 + heading_offset}>`.
    ///
    /// Defaults to 1, matching Emacs' own `org-html-toplevel-hlevel`, because the page
    /// layout supplies the `<h1>` — the document's title — and section headings sit
    /// beneath it. Set to 0 if your template renders no title of its own, so the
    /// document does not start at `<h2>` with nothing above it.
    pub heading_offset: u8,
}

impl Default for HtmlOutput {
    fn default() -> Self {
        HtmlOutput { heading_offset: 1 }
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
}

impl Default for Site {
    fn default() -> Self {
        Site {
            title: "org-ssg site".to_string(),
            base_url: String::new(),
            description: String::new(),
            language: "en".to_string(),
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
    /// A syntect built-in theme name — `InspiredGitHub`, `Solarized (dark)`,
    /// `base16-ocean.dark`, `base16-eighties.dark`, `base16-mocha.dark`,
    /// `base16-ocean.light`. Highlighting emits CSS classes, and this theme is what the
    /// generated `syntax.css` colours them with.
    pub theme: String,
}

impl Default for Highlight {
    fn default() -> Self {
        Highlight {
            theme: "InspiredGitHub".to_string(),
        }
    }
}

impl Config {
    /// Load `org-ssg.toml` from `dir`, or return defaults if there is none.
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

/// The starter config written by `org-ssg init`, and the documentation of record for
/// what is configurable. Every value shown is the default, so deleting any line is safe.
pub const STARTER_CONFIG: &str = r#"# org-ssg configuration. Every setting here is optional and shown at its default,
# so you can delete any line you do not need — or the whole file.

[site]
title = "org-ssg site"
# Absolute base URL, no trailing slash. Needed for feeds and canonical links, which
# cannot be relative — set it and uncomment the [[collections]] feed block below.
base_url = ""
description = ""
language = "en"

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

[highlight]
# A syntect theme name: InspiredGitHub, Solarized (dark), base16-ocean.dark,
# base16-eighties.dark, base16-mocha.dark, base16-ocean.light.
theme = "InspiredGitHub"

[html]
# How far to push heading levels down: a level-1 org heading becomes <h(1 + offset)>.
# The default of 1 matches Emacs, and assumes your layout renders the page title as the
# <h1>. Set to 0 if your template renders no title of its own.
heading_offset = 1

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
