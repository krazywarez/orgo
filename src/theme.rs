//! Built-in site themes: whole stylesheets compiled into the binary.
//!
//! A theme is one CSS file and nothing else. It styles the markup the RENDER stage
//! emits — org's headings, tags, TODO keywords, checkbox lists, footnotes — plus the
//! chrome the built-in layout and the starter templates put around it. There is no
//! theme-specific HTML, so a theme can be switched, or removed, without touching a
//! template.
//!
//! Compiled in for the same reason the syntax definitions are: `cargo install orgo`
//! gives you one binary, and a site that needs a stylesheet fetched from somewhere else
//! before it looks like anything is not that. The chosen theme is written to the output
//! root as `theme.css` on every build, the way [`crate::render::syntax_css`] writes
//! `syntax.css`.
//!
//! Nothing here is a wrapper you have to work through: `site.theme` empty emits no
//! stylesheet at all, and a `base.html` of your own can ignore `theme.css` and link
//! whatever it likes.

/// Every built-in theme, as `(name, stylesheet)`, in the order they are offered.
///
/// - `plain` — readable defaults with no design opinion, to build your own CSS on.
/// - `blog` — dated writing: serif prose, a masthead, styled listing pages.
/// - `wiki` — a dense reference site: wide, sidebar contents, tables and TODO states.
/// - `docs` — a guide read in order: prominent contents, code-forward, `#+LEDE:`.
pub const THEMES: &[(&str, &str)] = &[
    ("plain", include_str!("../themes/plain.css")),
    ("blog", include_str!("../themes/blog.css")),
    ("wiki", include_str!("../themes/wiki.css")),
    ("docs", include_str!("../themes/docs.css")),
];

/// The stylesheet for a built-in theme, or `None` if no theme goes by that name.
pub fn theme_css(name: &str) -> Option<&'static str> {
    THEMES
        .iter()
        .find(|(theme, _)| *theme == name)
        .map(|(_, css)| *css)
}

/// Every theme name [`theme_css`] accepts, for error messages and documentation.
pub fn available_themes() -> Vec<&'static str> {
    THEMES.iter().map(|(name, _)| *name).collect()
}
