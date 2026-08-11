//! TEMPLATE stage (spec §2.1, §2.4, §3.3): rendered fragment + page metadata → full HTML.
//!
//! minijinja (Jinja2 semantics, runtime templates: edit-and-rebuild, no recompile).
//!
//! Templates come from the configured directory when it exists, and fall back to a
//! built-in layout when it does not. That fallback is what lets a bare directory of
//! `.org` files build into a real site with no setup, while `base.html` in the templates
//! directory replaces the layout entirely for anyone who wants their own.
//!
//! Template sources are a hashing input for incrementality (spec §4.1): editing a layout
//! invalidates the pages that use it, and that has to hold for user templates too, or a
//! design change would leave a site half-updated.

use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, Result};
use camino::Utf8Path;
use minijinja::{context, Environment};
use serde::Serialize;

/// A navigation entry: a page title and the URL to reach it from the current page.
#[derive(Debug, Clone, Serialize)]
pub struct NavItem {
    pub title: String,
    pub url: String,
}

/// Site-wide values, exposed to templates as `site`.
#[derive(Debug, Clone, Serialize)]
pub struct SiteContext {
    pub title: String,
    pub base_url: String,
    pub description: String,
    pub language: String,
}

/// One page's metadata, exposed to templates as `page` — and, when
/// `templates.expose_page_list` is on, as entries of `pages`.
#[derive(Debug, Clone, Serialize)]
pub struct PageContext {
    pub title: String,
    /// Output path relative to the site root, e.g. `blog/post.html`.
    pub url: String,
    /// Source path relative to the source root, e.g. `blog/post.org`.
    pub source: String,
    /// `#+DATE:` verbatim, if present — org date syntax is not normalized here because
    /// templates are better placed to decide how a date should read.
    pub date: Option<String>,
    /// The `YYYY-MM-DD` found inside `date`, if there is one. Org dates arrive in many
    /// shapes (`[2025-09-05 Fri 10:21:00]`, `<2024-05-01>`, `2024-05-01`), and a listing
    /// wants one it can sort and print. `None` when the date is free text like "someday".
    pub date_iso: Option<String>,
    /// The year from `date_iso`, so a listing can group by it with minijinja's
    /// `groupby` filter — which takes an attribute name and cannot slice a date itself.
    pub year: Option<String>,
    /// `#+FILETAGS:` split on `:`.
    pub tags: Vec<String>,
    /// A short summary for listings: `#+DESCRIPTION:` when the page sets one, otherwise
    /// its first paragraph. Empty only when the page has neither.
    pub excerpt: String,
    /// Words of prose, excluding code and example blocks.
    pub word_count: usize,
    /// Minutes to read at 200 words per minute, rounded up; at least 1 for a page with
    /// any prose at all.
    pub reading_time: usize,
    /// Every `#+KEYWORD:` in the file, keyed by lowercased name, so a template can use
    /// project-specific metadata this crate has never heard of.
    pub keywords: BTreeMap<String, String>,
    /// The page's headings as a tree. Empty when the page has none, when the site turns
    /// `html.toc` off, or when the document opts out with `#+OPTIONS: toc:nil`.
    pub toc: Vec<crate::util::TocEntry>,
}

/// The built-in layout, used when the templates directory has no `base.html`.
/// Deliberately plain: it should be a working starting point and an obvious thing to
/// replace, not a design anyone has to live with.
const BASE_TEMPLATE: &str = r##"<!DOCTYPE html>
<html lang="{{ site.language }}">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>{{ page.title }} &middot; {{ site.title }}</title>
{%- if site.base_url %}
<link rel="canonical" href="{{ page.url | absolute }}">
{%- endif %}
{%- if page.description %}
<meta name="description" content="{{ page.description }}">
{%- endif %}
{%- if stylesheet %}
<link rel="stylesheet" href="{{ stylesheet }}">
{%- endif %}
</head>
<body>
<header>
<a class="site-title" href="{{ root }}index.html">{{ site.title }}</a>
{%- if nav %}
<nav>
{%- for item in nav %}
<a href="{{ item.url }}">{{ item.title }}</a>
{%- endfor %}
</nav>
{%- endif %}
</header>
<main>
<h1>{{ page.title }}</h1>
{%- if page.date %}
<p class="page-date">{{ page.date }}</p>
{%- endif %}
{%- if page.toc | length > 1 %}
{%- macro toc_list(entries) %}
<ul>
{%- for entry in entries %}
<li><a href="#{{ entry.anchor }}">{{ entry.title }}</a>
{%- if entry.children %}{{ toc_list(entry.children) }}{% endif %}</li>
{%- endfor %}
</ul>
{%- endmacro %}
<nav class="toc" aria-label="Table of contents">
<h2>Contents</h2>
{{- toc_list(page.toc) }}
</nav>
{%- endif %}
{{ body | safe }}</main>
</body>
</html>
"##;

/// The name a template must have to serve as the page layout.
pub const BASE_TEMPLATE_NAME: &str = "base.html";

#[derive(Debug, thiserror::Error)]
pub enum TemplateError {
    #[error("template error: {0}")]
    Render(String),
}

/// Wraps a rendered fragment in its page template.
pub struct Templater {
    env: Environment<'static>,
    /// `(name, source)` for every registered template, for the template hash. Sorted by
    /// name so the hash does not depend on directory iteration order.
    sources: Vec<(String, String)>,
}

impl Templater {
    /// Load templates from `dir`, falling back to the built-in layout.
    ///
    /// A missing directory is fine — that is the zero-config path. A directory that
    /// exists but contains a template that does not compile is an error: it means
    /// someone is actively editing their layout, and rendering the built-in default
    /// instead would look like their edit silently did nothing.
    pub fn load(dir: Option<&Utf8Path>, base_url: &str) -> Result<Self> {
        let mut sources: Vec<(String, String)> = Vec::new();

        if let Some(dir) = dir.filter(|d| d.is_dir()) {
            // Registered by full relative filename — `base.html`, `partials/head.html` —
            // because that is what `{% extends "base.html" %}` names, and a stem-based
            // scheme silently breaks the include syntax every Jinja user already knows.
            // Any extension is loaded, so a feed can be a listing page with an XML
            // template rather than a separate mechanism.
            for entry in walkdir::WalkDir::new(dir).sort_by_file_name() {
                let entry = entry.with_context(|| format!("reading templates from {dir}"))?;
                if !entry.file_type().is_file() {
                    continue;
                }
                let path = Utf8Path::from_path(entry.path())
                    .map(Utf8Path::to_owned)
                    .ok_or_else(|| anyhow::anyhow!("non-UTF-8 template path"))?;
                let name = path
                    .strip_prefix(dir)
                    .unwrap_or(&path)
                    .as_str()
                    .replace('\\', "/");
                if name.starts_with('.') || name.contains("/.") {
                    continue;
                }
                let source = std::fs::read_to_string(&path)
                    .with_context(|| format!("reading template {path}"))?;
                sources.push((name, source));
            }
        }

        if !sources.iter().any(|(n, _)| n == BASE_TEMPLATE_NAME) {
            sources.push((BASE_TEMPLATE_NAME.to_string(), BASE_TEMPLATE.to_string()));
        }
        sources.sort_by(|a, b| a.0.cmp(&b.0));

        let mut env = Environment::new();
        env.set_formatter(html_formatter);
        add_filters(&mut env, base_url);
        for (name, source) in &sources {
            // `Environment<'static>` needs owned sources; leaking is bounded by the
            // template count and lives as long as the build anyway.
            let name: &'static str = Box::leak(name.clone().into_boxed_str());
            let source: &'static str = Box::leak(source.clone().into_boxed_str());
            env.add_template(name, source)
                .with_context(|| format!("compiling template {name}"))?;
        }

        Ok(Templater { env, sources })
    }

    /// `(name, source)` for every registered template — the template hash's input
    /// (spec §4.1), covering user templates so editing one invalidates its pages.
    pub fn sources(&self) -> &[(String, String)] {
        &self.sources
    }

    /// The sources a page rendered through `name` actually depends on: that template plus
    /// everything it extends, includes or imports, transitively.
    ///
    /// This is what keeps a layout edit proportional. Hashing *all* templates into every
    /// page means touching `feed.xml` re-renders a 200-page site, which is most of the
    /// wait in a `serve` session spent on design.
    ///
    /// A template whose include is computed at render time — `{% include chooser %}` —
    /// cannot be followed statically, so it depends on everything. Over-invalidating is
    /// slow; under-invalidating publishes a stale page.
    pub fn sources_for(&self, name: &str) -> Vec<(String, String)> {
        let mut seen: BTreeSet<String> = BTreeSet::new();
        let mut queue = vec![name.to_string()];
        while let Some(current) = queue.pop() {
            if !seen.insert(current.clone()) {
                continue;
            }
            let Some((_, source)) = self.sources.iter().find(|(n, _)| *n == current) else {
                continue;
            };
            let (deps, dynamic) = referenced_templates(source);
            if dynamic {
                return self.sources.clone();
            }
            queue.extend(deps);
        }
        self.sources
            .iter()
            .filter(|(n, _)| seen.contains(n))
            .cloned()
            .collect()
    }

    /// Is a template with this name registered?
    pub fn has(&self, name: &str) -> bool {
        self.env.get_template(name).is_ok()
    }

    /// Every registered template name, for error messages.
    pub fn names(&self) -> Vec<&str> {
        self.sources.iter().map(|(n, _)| n.as_str()).collect()
    }

    /// Render through a named template. Generated pages use this to reach their own
    /// layout; the context is identical to a normal page's, so a listing template can
    /// `{% extends "base.html" %}` and inherit the site's chrome for free.
    pub fn render(&self, template: &str, ctx: &RenderContext) -> Result<String, TemplateError> {
        let tmpl = self
            .env
            .get_template(template)
            .map_err(|e| TemplateError::Render(e.to_string()))?;
        tmpl.render(context! {
            site => ctx.site,
            page => ctx.page,
            body => ctx.body,
            nav => ctx.nav,
            stylesheet => ctx.stylesheet,
            root => ctx.root,
            pages => ctx.pages,
            group => ctx.group,
            groups => ctx.groups,
            paginator => ctx.paginator,
        })
        .map_err(|e| TemplateError::Render(render_error_detail(e)))
    }

    /// Render through the site's base layout.
    pub fn render_page(&self, ctx: &RenderContext) -> Result<String, TemplateError> {
        self.render(BASE_TEMPLATE_NAME, ctx)
    }
}

/// One group of a grouped collection — a tag, or a `#+CATEGORY:` value.
#[derive(Debug, Clone, Serialize)]
pub struct GroupContext {
    /// The term as written, e.g. `Rust Lang`.
    pub name: String,
    /// URL-safe form used in the output path, e.g. `rust-lang`.
    pub slug: String,
    /// Output path of this group's page, relative to the site root. Empty when the
    /// collection emits no per-group pages.
    pub url: String,
    /// How many pages carry this term.
    pub count: usize,
}

/// One page of a paginated listing, exposed to templates as `paginator`.
///
/// Every URL here is relative to the page being rendered, so a template can emit them
/// directly however deep the page sits.
#[derive(Debug, Clone, Serialize)]
pub struct Paginator {
    /// 1-based number of this page.
    pub current: usize,
    /// How many pages the listing splits into.
    pub total: usize,
    /// Entries per page, as configured.
    pub per_page: usize,
    /// Entries across the whole listing, not just this page.
    pub total_entries: usize,
    pub prev_url: Option<String>,
    pub next_url: Option<String>,
    pub first_url: String,
    pub last_url: String,
    /// Every page, for a numbered strip.
    pub pages: Vec<PaginatorPage>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PaginatorPage {
    pub number: usize,
    pub url: String,
    /// True for the page currently being rendered, so a template can mark it without
    /// comparing numbers itself.
    pub current: bool,
}

/// Everything a template can see. A struct rather than a dozen positional arguments,
/// because the list grows every time templates learn something new.
pub struct RenderContext<'a> {
    pub site: &'a SiteContext,
    pub page: &'a PageContext,
    /// Rendered page HTML. Empty for generated pages, which build their body from
    /// `pages`/`groups` instead.
    pub body: &'a str,
    pub nav: &'a [NavItem],
    /// URL of the syntax stylesheet, relative to this page.
    pub stylesheet: &'a str,
    /// `../`-prefix back to the site root from this page.
    pub root: &'a str,
    /// The pages this listing shows, or every page when `expose_page_list` is on.
    pub pages: Option<&'a [PageContext]>,
    /// The group this page is for, on a grouped collection's per-group page.
    pub group: Option<&'a GroupContext>,
    /// Every group of a grouped collection — the group index's content. Empty on a
    /// per-group page, which depends on its own entries and not on the other groups.
    pub groups: &'a [GroupContext],
    /// Present only on a page of a paginated listing.
    pub paginator: Option<&'a Paginator>,
}

impl<'a> RenderContext<'a> {
    /// A context with only the universally-present parts filled in.
    pub fn new(
        site: &'a SiteContext,
        page: &'a PageContext,
        nav: &'a [NavItem],
        stylesheet: &'a str,
        root: &'a str,
    ) -> Self {
        RenderContext {
            site,
            page,
            body: "",
            nav,
            stylesheet,
            root,
            pages: None,
            group: None,
            groups: &[],
            paginator: None,
        }
    }
}

/// The starter tag-index template written by `org-ssg init`: shows how `groups` is
/// iterated, and how a group page is linked.
pub const STARTER_TAGS_TEMPLATE: &str = r#"<!DOCTYPE html>
<html lang="{{ site.language }}">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>{{ page.title }} &middot; {{ site.title }}</title>
{%- if stylesheet %}
<link rel="stylesheet" href="{{ stylesheet }}">
{%- endif %}
</head>
<body>
<header>
<a class="site-title" href="{{ root }}index.html">{{ site.title }}</a>
{%- if nav %}
<nav>
{%- for item in nav %}
<a href="{{ item.url }}">{{ item.title }}</a>
{%- endfor %}
</nav>
{%- endif %}
</header>
<main>
<h1>{{ page.title }}</h1>
<ul class="tag-list">
{%- for tag in groups %}
<li><a href="{{ root }}{{ tag.url }}">{{ tag.name }}</a> ({{ tag.count }})</li>
{%- endfor %}
</ul>
</main>
</body>
</html>
"#;

/// The starter listing template written by `org-ssg init`: a blog index, showing how a
/// collection's `pages` are iterated.
pub const STARTER_LIST_TEMPLATE: &str = r#"<!DOCTYPE html>
<html lang="{{ site.language }}">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>{{ page.title }} &middot; {{ site.title }}</title>
{%- if stylesheet %}
<link rel="stylesheet" href="{{ stylesheet }}">
{%- endif %}
</head>
<body>
<header>
<a class="site-title" href="{{ root }}index.html">{{ site.title }}</a>
{%- if nav %}
<nav>
{%- for item in nav %}
<a href="{{ item.url }}">{{ item.title }}</a>
{%- endfor %}
</nav>
{%- endif %}
</header>
<main>
<h1>{{ page.title }}</h1>
<ul class="post-list">
{%- for post in pages %}
<li>
{%- if post.date_iso %}<time datetime="{{ post.date_iso }}">{{ post.date_iso }}</time> {% endif %}
<a href="{{ root }}{{ post.url }}">{{ post.title }}</a>
{%- if post.excerpt %}
<p class="excerpt">{{ post.excerpt | truncate(180) }}</p>
{%- endif %}
<span class="reading-time">{{ post.reading_time }} min read</span>
</li>
{%- endfor %}
</ul>
{%- if paginator and paginator.total > 1 %}
<nav class="pagination">
{%- if paginator.prev_url %}
<a rel="prev" href="{{ paginator.prev_url }}">Newer</a>
{%- endif %}
<span>Page {{ paginator.current }} of {{ paginator.total }}</span>
{%- if paginator.next_url %}
<a rel="next" href="{{ paginator.next_url }}">Older</a>
{%- endif %}
</nav>
{%- endif %}
</main>
</body>
</html>
"#;

/// Filters a template can use beyond minijinja's built-ins.
///
/// Both exist for the same reason: a syndication feed has requirements an HTML page does
/// not, and satisfying them by hand in a template is the kind of thing that produces a
/// feed which *looks* right and fails validation.
fn add_filters(env: &mut Environment<'static>, base_url: &str) {
    let base = base_url.trim_end_matches('/').to_string();

    // `absolute`: a site-root-relative path → an absolute URL.
    //
    // Feeds are read away from the site that served them, so relative links in one are
    // simply broken. Applies to the site-root-relative paths — `page.url`, `pages[].url`,
    // `group.url` — and not to `nav[].url`, `paginator.*_url`, `stylesheet` or `root`,
    // which are relative to the page carrying them and already correct in a page.
    env.add_filter(
        "absolute",
        move |path: &str| -> Result<String, minijinja::Error> {
            if base.is_empty() {
                // Returning the relative path would produce a feed that validates
                // nowhere and looks fine everywhere. Say what is missing instead.
                return Err(minijinja::Error::new(
                    minijinja::ErrorKind::InvalidOperation,
                    "the `absolute` filter needs site.base_url, which is empty; \
                     set it in org-ssg.toml (e.g. base_url = \"https://example.com\")",
                ));
            }
            if path.starts_with("http://") || path.starts_with("https://") {
                return Ok(path.to_string());
            }
            Ok(format!("{base}/{}", path.trim_start_matches('/')))
        },
    );

    // `truncate`: shorten to at most N characters, on a word boundary, with an ellipsis.
    //
    // minijinja ships no truncate, and an excerpt is usually a whole first paragraph —
    // so without this the only options in a listing are the full paragraph or nothing.
    env.add_filter(
        "truncate",
        |text: &str, limit: Option<usize>| -> String {
            let limit = limit.unwrap_or(160);
            if text.chars().count() <= limit {
                return text.to_string();
            }
            let head: String = text.chars().take(limit).collect();
            // Cut at the last space so a word is never sliced in half; if there is no
            // space at all, the hard cut is the only option.
            let cut = head.rfind(char::is_whitespace).unwrap_or(head.len());
            format!("{}…", head[..cut].trim_end())
        },
    );

    // `rfc822`: an org or ISO date → the format RSS `pubDate` requires.
    env.add_filter("rfc822", |raw: &str| -> Result<String, minijinja::Error> {
        let iso = crate::util::iso_date(raw).ok_or_else(|| {
            minijinja::Error::new(
                minijinja::ErrorKind::InvalidOperation,
                format!("cannot read a date out of {raw:?} for an RSS pubDate"),
            )
        })?;
        let date = chrono::NaiveDate::parse_from_str(&iso, "%Y-%m-%d").map_err(|e| {
            minijinja::Error::new(
                minijinja::ErrorKind::InvalidOperation,
                format!("{iso} is not a valid date: {e}"),
            )
        })?;
        // Org dates carry no timezone, so midnight UTC is the honest reading of one.
        Ok(date
            .and_hms_opt(0, 0, 0)
            .expect("midnight is a valid time")
            .format("%a, %d %b %Y %H:%M:%S +0000")
            .to_string())
    });
}

/// The starter RSS feed written by `org-ssg init`. A listing page with an XML template:
/// no feed-specific machinery, just `absolute` and `rfc822` doing what syndication needs.
///
/// Emitted commented-out guidance rather than a broken feed when `site.base_url` is
/// unset — see the `init` scaffold, which leaves the feed collection commented out until
/// there is a base URL to make absolute links from.
pub const STARTER_FEED_TEMPLATE: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<rss version="2.0" xmlns:atom="http://www.w3.org/2005/Atom">
<channel>
<title>{{ site.title }}</title>
<link>{{ "index.html" | absolute }}</link>
<description>{{ site.description }}</description>
<language>{{ site.language }}</language>
<atom:link href="{{ page.url | absolute }}" rel="self" type="application/rss+xml"/>
{%- for post in pages %}
<item>
<title>{{ post.title }}</title>
<link>{{ post.url | absolute }}</link>
<guid isPermaLink="true">{{ post.url | absolute }}</guid>
{%- if post.date_iso %}
<pubDate>{{ post.date_iso | rfc822 }}</pubDate>
{%- endif %}
{%- for tag in post.tags %}
<category>{{ tag }}</category>
{%- endfor %}
</item>
{%- endfor %}
</channel>
</rss>
"#;

/// Template names a source refers to, and whether any reference is computed at render
/// time rather than written as a literal.
///
/// A hand-rolled scan rather than a parse: minijinja does not expose the dependency
/// graph, and the three tags that pull in another template all name it as the first
/// string literal in the tag.
fn referenced_templates(source: &str) -> (Vec<String>, bool) {
    const TAGS: &[&str] = &["extends", "include", "import", "from"];
    let mut names = Vec::new();
    let mut dynamic = false;
    let mut rest = source;
    while let Some(start) = rest.find("{%") {
        let after = &rest[start + 2..];
        let Some(end) = after.find("%}") else { break };
        let tag = &after[..end];
        rest = &after[end + 2..];

        let keyword = tag
            .trim_start()
            .trim_start_matches('-')
            .split_whitespace()
            .next()
            .unwrap_or("");
        if !TAGS.contains(&keyword) {
            continue;
        }
        match string_literal(tag) {
            Some(name) => names.push(name),
            // `{% include some_variable %}` or `{% include ["a", "b"] %}` past the first
            // entry: the set cannot be known here.
            None => dynamic = true,
        }
    }
    if source.contains("{% include [") || source.contains("{%- include [") {
        dynamic = true;
    }
    (names, dynamic)
}

/// The first single- or double-quoted string in a tag body.
fn string_literal(tag: &str) -> Option<String> {
    let bytes = tag.as_bytes();
    let quote = bytes.iter().position(|b| *b == b'"' || *b == b'\'')?;
    let delim = bytes[quote];
    let after = &tag[quote + 1..];
    let end = after.find(delim as char)?;
    Some(after[..end].to_string())
}

/// HTML-escape template output, escaping the same characters Jinja2 does.
///
/// minijinja additionally escapes `/` as `&#x2f;`, which is a defence for values
/// interpolated into JavaScript. It is correct but, since `<` is escaped anyway, it buys
/// nothing in an HTML document — and it makes every generated URL read
/// `..&#x2f;index.html`. Templates emit a lot of URLs, so that is most of the output.
///
/// Auto-escaping itself stays on: page titles come from `#+TITLE:` and are user content.
fn html_formatter(
    out: &mut minijinja::Output,
    state: &minijinja::State,
    value: &minijinja::Value,
) -> Result<(), minijinja::Error> {
    if state.auto_escape() == minijinja::AutoEscape::Html && !value.is_safe() {
        if let Some(text) = value.as_str() {
            let mut escaped = String::with_capacity(text.len());
            for c in text.chars() {
                match c {
                    '&' => escaped.push_str("&amp;"),
                    '<' => escaped.push_str("&lt;"),
                    '>' => escaped.push_str("&gt;"),
                    '"' => escaped.push_str("&quot;"),
                    '\'' => escaped.push_str("&#x27;"),
                    _ => escaped.push(c),
                }
            }
            return out.write_str(&escaped).map_err(minijinja::Error::from);
        }
    }
    minijinja::escape_formatter(out, state, value)
}

/// minijinja's `Display` gives only the top-level message; the useful part (which
/// template, which line) is in the source and cause chain.
fn render_error_detail(error: minijinja::Error) -> String {
    let mut out = error.to_string();
    if let Some(name) = error.template_source().map(|_| error.name().unwrap_or("?")) {
        if let Some(line) = error.line() {
            out = format!("{out} (in template {name}, line {line})");
        }
    }
    let mut source = std::error::Error::source(&error);
    while let Some(cause) = source {
        out.push_str(&format!(": {cause}"));
        source = cause.source();
    }
    out
}

/// The starter layout written by `org-ssg init`: the built-in template, on disk, ready
/// to edit.
pub fn starter_template() -> &'static str {
    BASE_TEMPLATE
}
