//! org-ssg — an org-mode static site generator.
//!
//! Org is the source language, not an input to be normalized into markdown. The org
//! element tree ([`model`]) *is* the document model; we render it straight to HTML.
//!
//! Pipeline (spec §2.1), one module per stage:
//! DISCOVER → [`parser`] (PARSE) → [`index`] (INDEX) → [`resolve`] (RESOLVE) →
//! [`render`] (RENDER) → [`template`] (TEMPLATE) → EMIT, with [`incremental`]
//! deciding which pages actually need rewriting.

pub mod audit;
pub mod config;
pub mod incremental;
pub mod index;
pub mod model;
pub mod parser;
pub mod render;
pub mod resolve;
pub mod site;
pub mod template;
pub mod util;
