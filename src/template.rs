//! TEMPLATE stage (spec §2.1, §2.4, §3.3): rendered fragment + page metadata → full HTML.
//!
//! minijinja (Jinja2 semantics, runtime templates: edit-and-rebuild, no recompile).
//! Templates are a hashing input for incrementality (spec §4.1): a base-layout edit
//! invalidates every page that transitively uses it. Keep the fragment/template
//! boundary sharp so content HTML can be snapshot-tested independently of chrome.

use minijinja::{context, Environment};
use serde::Serialize;

/// A navigation entry: a page title and the URL to reach it from the current page.
#[derive(Debug, Clone, Serialize)]
pub struct NavItem {
    pub title: String,
    pub url: String,
}

/// The base layout applied to every page: `<title>`, a nav bar, and the body.
/// Minimal but real — a single `base` template, no partials yet.
const BASE_TEMPLATE: &str = r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<title>{{ title }}</title>
</head>
<body>
<nav>
{%- for item in nav %}
<a href="{{ item.url }}">{{ item.title }}</a>
{%- endfor %}
</nav>
<main>
{{ body | safe }}</main>
</body>
</html>
"#;

/// The source text of every template that participates in the page layout. Hashed by
/// the incremental layer (spec §4.1): a base-layout edit invalidates every page that
/// uses it. There is a single `base` template today; when partials arrive this returns
/// the transitive closure so a single-partial edit invalidates only its users.
pub fn template_sources() -> &'static [(&'static str, &'static str)] {
    &[("base", BASE_TEMPLATE)]
}

#[derive(Debug, thiserror::Error)]
pub enum TemplateError {
    #[error("template error: {0}")]
    Render(String),
}

/// Wraps a rendered fragment in its page template.
pub struct Templater {
    env: Environment<'static>,
}

impl Templater {
    pub fn new() -> Self {
        let mut env = Environment::new();
        env.add_template("base", BASE_TEMPLATE)
            .expect("base template compiles");
        Templater { env }
    }

    /// fragment + page metadata → full HTML page.
    pub fn render_page(
        &self,
        title: &str,
        body: &str,
        nav: &[NavItem],
    ) -> Result<String, TemplateError> {
        let tmpl = self
            .env
            .get_template("base")
            .map_err(|e| TemplateError::Render(e.to_string()))?;
        tmpl.render(context! { title => title, body => body, nav => nav })
            .map_err(|e| TemplateError::Render(e.to_string()))
    }
}

impl Default for Templater {
    fn default() -> Self {
        Self::new()
    }
}
