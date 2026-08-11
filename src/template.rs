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

use std::collections::BTreeMap;

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
    /// `#+FILETAGS:` split on `:`.
    pub tags: Vec<String>,
    /// Every `#+KEYWORD:` in the file, keyed by lowercased name, so a template can use
    /// project-specific metadata this crate has never heard of.
    pub keywords: BTreeMap<String, String>,
}

/// The built-in layout, used when the templates directory has no `base.html`.
/// Deliberately plain: it should be a working starting point and an obvious thing to
/// replace, not a design anyone has to live with.
const BASE_TEMPLATE: &str = r#"<!DOCTYPE html>
<html lang="{{ site.language }}">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>{{ page.title }} &middot; {{ site.title }}</title>
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
{{ body | safe }}</main>
</body>
</html>
"#;

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
    pub fn load(dir: Option<&Utf8Path>) -> Result<Self> {
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
</li>
{%- endfor %}
</ul>
</main>
</body>
</html>
"#;

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
