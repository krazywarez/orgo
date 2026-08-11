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
pub const BASE_TEMPLATE_NAME: &str = "base";

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
            let mut entries: Vec<_> = std::fs::read_dir(dir)
                .with_context(|| format!("reading template directory {dir}"))?
                .collect::<std::io::Result<Vec<_>>>()
                .with_context(|| format!("reading template directory {dir}"))?;
            entries.sort_by_key(|e| e.file_name());

            for entry in entries {
                let path = Utf8Path::from_path(&entry.path())
                    .map(Utf8Path::to_owned)
                    .ok_or_else(|| anyhow::anyhow!("non-UTF-8 template path"))?;
                if path.extension() != Some("html") || !path.is_file() {
                    continue;
                }
                let name = path
                    .file_stem()
                    .ok_or_else(|| anyhow::anyhow!("template with no name: {path}"))?
                    .to_string();
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

    /// fragment + page metadata → full HTML page.
    ///
    /// `stylesheet` and `root` are URLs relative to *this* page, so a template works the
    /// same at any directory depth.
    #[allow(clippy::too_many_arguments)]
    pub fn render_page(
        &self,
        site: &SiteContext,
        page: &PageContext,
        body: &str,
        nav: &[NavItem],
        stylesheet: &str,
        root: &str,
        pages: Option<&[PageContext]>,
    ) -> Result<String, TemplateError> {
        let tmpl = self
            .env
            .get_template(BASE_TEMPLATE_NAME)
            .map_err(|e| TemplateError::Render(e.to_string()))?;
        tmpl.render(context! {
            site => site,
            page => page,
            body => body,
            nav => nav,
            stylesheet => stylesheet,
            root => root,
            pages => pages,
        })
        .map_err(|e| TemplateError::Render(render_error_detail(e)))
    }
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
