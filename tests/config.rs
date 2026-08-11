//! Configuration, templating and discovery — the surface that decides whether this is a
//! generator for one site or for anyone's.
//!
//! The theme running through these tests is that **the zero-config path has to work**.
//! A directory of `.org` files with no `org-ssg.toml`, no templates and no knowledge of
//! this tool must build into a real site; configuration is how you change the output,
//! never how you make it work at all.

use std::sync::atomic::{AtomicU32, Ordering};

use camino::Utf8PathBuf;

use org_ssg::config::{Config, NavMode};
use org_ssg::site::{build_site, BuildOptions};

fn tmpdir(tag: &str) -> Utf8PathBuf {
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    let base = Utf8PathBuf::from_path_buf(std::env::temp_dir())
        .expect("utf-8 temp dir")
        .join(format!("org-ssg-cfg-{}-{tag}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    base
}

/// A site with a root page, a second root page, and one nested page.
fn write_site(src: &Utf8PathBuf) {
    std::fs::create_dir_all(src.join("blog")).unwrap();
    std::fs::write(src.join("index.org"), "#+TITLE: Home\n\nWelcome.\n").unwrap();
    std::fs::write(src.join("about.org"), "#+TITLE: About\n\nAbout.\n").unwrap();
    std::fs::write(
        src.join("blog/post.org"),
        "#+TITLE: A Post\n#+DATE: 2024-05-01\n#+FILETAGS: :rust:web:\n\nBody.\n",
    )
    .unwrap();
}

fn build(src: &Utf8PathBuf, out: &Utf8PathBuf) -> org_ssg::site::SiteReport {
    build_site(src, out, &BuildOptions::default()).expect("build")
}

fn page(out: &Utf8PathBuf, rel: &str) -> String {
    std::fs::read_to_string(out.join(rel)).unwrap_or_else(|e| panic!("reading {rel}: {e}"))
}

// ---------------------------------------------------------------------------
// Zero config
// ---------------------------------------------------------------------------

/// The headline promise: point it at a directory of org files and get a site.
#[test]
fn a_bare_directory_of_org_files_builds_with_no_config() {
    let root = tmpdir("bare");
    let src = root.join("src");
    std::fs::create_dir_all(&src).unwrap();
    write_site(&src);
    let out = root.join("out");

    let report = build(&src, &out);
    assert_eq!(report.pages.len(), 3);

    let home = page(&out, "index.html");
    assert!(home.contains("<!DOCTYPE html>"), "a full page, not a fragment");
    assert!(home.contains("Welcome."), "the content is there");
    assert!(
        out.join("syntax.css").exists(),
        "the stylesheet the highlighter needs is emitted too"
    );
}

/// A missing config is normal. A *malformed* one is not: someone who wrote a config
/// meant it, and quietly building the default site would hide their typo behind
/// plausible-looking output.
#[test]
fn a_malformed_config_is_an_error_but_a_missing_one_is_not() {
    let root = tmpdir("malformed");
    let src = root.join("src");
    std::fs::create_dir_all(&src).unwrap();
    write_site(&src);

    assert_eq!(Config::load(&src).unwrap(), Config::default());

    std::fs::write(src.join("org-ssg.toml"), "[site\ntitle = broken").unwrap();
    let err = Config::load(&src).expect_err("malformed config must fail");
    assert!(format!("{err:#}").contains("org-ssg.toml"), "names the file: {err:#}");
}

/// A misspelled key is a silent no-op in most config formats, which is exactly how
/// someone spends an afternoon wondering why a setting does nothing.
#[test]
fn an_unknown_config_key_is_rejected() {
    let root = tmpdir("unknownkey");
    let src = root.join("src");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(src.join("org-ssg.toml"), "[site]\ntittle = \"typo\"\n").unwrap();

    let err = Config::load(&src).expect_err("unknown key must fail");
    assert!(
        format!("{err:#}").contains("tittle"),
        "the error names the offending key: {err:#}"
    );
}

// ---------------------------------------------------------------------------
// Nav modes
// ---------------------------------------------------------------------------

fn nav_of(html: &str) -> String {
    html.split("<nav>")
        .nth(1)
        .and_then(|s| s.split("</nav>").next())
        .unwrap_or("")
        .to_string()
}

#[test]
fn nav_modes_select_different_pages() {
    for (mode, expect_post, expect_about) in [
        ("top-level", false, true),
        ("all", true, true),
        ("none", false, false),
    ] {
        let root = tmpdir(&format!("nav-{mode}"));
        let src = root.join("src");
        std::fs::create_dir_all(&src).unwrap();
        write_site(&src);
        std::fs::write(
            src.join("org-ssg.toml"),
            format!("[nav]\nmode = \"{mode}\"\n"),
        )
        .unwrap();
        let out = root.join("out");
        build(&src, &out);

        let nav = nav_of(&page(&out, "index.html"));
        assert_eq!(
            nav.contains("A Post"),
            expect_post,
            "mode {mode} nested page presence, nav was:\n{nav}"
        );
        assert_eq!(
            nav.contains("About"),
            expect_about,
            "mode {mode} root page presence, nav was:\n{nav}"
        );
    }
}

/// An explicit nav is a designed sequence, so configured order beats discovery order.
#[test]
fn explicit_nav_uses_the_configured_order() {
    let root = tmpdir("navexplicit");
    let src = root.join("src");
    std::fs::create_dir_all(&src).unwrap();
    write_site(&src);
    std::fs::write(
        src.join("org-ssg.toml"),
        "[nav]\nmode = \"explicit\"\npages = [\"blog/post.org\", \"index.org\"]\n",
    )
    .unwrap();
    let out = root.join("out");
    build(&src, &out);

    let nav = nav_of(&page(&out, "index.html"));
    let post = nav.find("A Post").expect("post in nav");
    let home = nav.find("Home").expect("home in nav");
    assert!(post < home, "configured order wins:\n{nav}");
    assert!(!nav.contains("About"), "unlisted pages stay out:\n{nav}");
}

/// A nav entry naming a page that does not exist is a typo, and a silently shorter nav
/// is a poor way to find out.
#[test]
fn explicit_nav_rejects_a_page_that_does_not_exist() {
    let root = tmpdir("navmissing");
    let src = root.join("src");
    std::fs::create_dir_all(&src).unwrap();
    write_site(&src);
    std::fs::write(
        src.join("org-ssg.toml"),
        "[nav]\nmode = \"explicit\"\npages = [\"nope.org\"]\n",
    )
    .unwrap();

    let err = build_site(&src, &root.join("out"), &BuildOptions::default())
        .expect_err("missing nav page must fail");
    assert!(format!("{err:#}").contains("nope.org"), "names it: {err:#}");
}

/// `mode` and `pages` disagreeing means one of them is being ignored.
#[test]
fn contradictory_nav_settings_are_rejected() {
    let mut config = Config::default();
    config.nav.pages = vec![Utf8PathBuf::from("index.org")];
    assert!(config.validate().is_err(), "pages without explicit mode");

    let mut config = Config::default();
    config.nav.mode = NavMode::Explicit;
    assert!(config.validate().is_err(), "explicit mode without pages");
}

// ---------------------------------------------------------------------------
// Templates
// ---------------------------------------------------------------------------

/// The single biggest blocker to general use: without this every site built with this
/// tool looks identical.
#[test]
fn a_user_template_replaces_the_built_in_layout() {
    let root = tmpdir("template");
    let src = root.join("src");
    std::fs::create_dir_all(src.join("templates")).unwrap();
    write_site(&src);
    std::fs::write(
        src.join("templates/base.html"),
        "<html><body class=\"mine\"><h1>{{ page.title }}</h1>{{ body | safe }}</body></html>",
    )
    .unwrap();
    let out = root.join("out");
    build(&src, &out);

    let home = page(&out, "index.html");
    assert!(home.contains("class=\"mine\""), "the user layout is used:\n{home}");
    assert!(!home.contains("<nav>"), "nothing of the default layout leaks in");
    assert!(home.contains("Welcome."), "content still renders");
}

/// Templates are a hashing input (spec §4.1). If editing a layout did not invalidate,
/// a design change would leave a site half-updated — the worst kind of caching bug,
/// because it looks like it worked.
#[test]
fn editing_a_template_re_renders_every_page_that_uses_it() {
    let root = tmpdir("templatehash");
    let src = root.join("src");
    std::fs::create_dir_all(src.join("templates")).unwrap();
    write_site(&src);
    let tpl = src.join("templates/base.html");
    std::fs::write(&tpl, "<html><body>v1{{ body | safe }}</body></html>").unwrap();
    let out = root.join("out");

    build(&src, &out);
    std::fs::write(&tpl, "<html><body>v2{{ body | safe }}</body></html>").unwrap();
    let report = build(&src, &out);

    assert_eq!(report.rendered.len(), 3, "a layout edit re-renders every page");
    assert!(page(&out, "index.html").contains("v2"), "and the change lands");
}

/// A template that does not compile means someone is actively editing their layout.
/// Falling back to the built-in would look like their edit silently did nothing.
#[test]
fn a_broken_template_fails_the_build() {
    let root = tmpdir("badtemplate");
    let src = root.join("src");
    std::fs::create_dir_all(src.join("templates")).unwrap();
    write_site(&src);
    std::fs::write(src.join("templates/base.html"), "{% if %}unclosed").unwrap();

    let err = build_site(&src, &root.join("out"), &BuildOptions::default())
        .expect_err("a broken template must fail the build");
    assert!(
        format!("{err:#}").contains("base"),
        "the error names the template: {err:#}"
    );
}

/// Templates get page metadata, including arbitrary `#+KEYWORD:`s this crate has never
/// heard of — otherwise every new bit of metadata would need a release.
#[test]
fn templates_receive_page_metadata_including_unknown_keywords() {
    let root = tmpdir("meta");
    let src = root.join("src");
    std::fs::create_dir_all(src.join("templates")).unwrap();
    write_site(&src);
    std::fs::write(
        src.join("blog/post.org"),
        "#+TITLE: A Post\n#+DATE: 2024-05-01\n#+FILETAGS: :rust:web:\n#+CUSTOM_THING: hello\n\nBody.\n",
    )
    .unwrap();
    std::fs::write(
        src.join("templates/base.html"),
        "<html><body>date={{ page.date }} tags={{ page.tags | join(\",\") }} \
         custom={{ page.keywords.custom_thing }} url={{ page.url }} \
         site={{ site.title }}{{ body | safe }}</body></html>",
    )
    .unwrap();
    let out = root.join("out");
    build(&src, &out);

    let post = page(&out, "blog/post.html");
    assert!(post.contains("date=2024-05-01"), "#+DATE: reaches the template:\n{post}");
    assert!(post.contains("tags=rust,web"), "#+FILETAGS: is split:\n{post}");
    assert!(post.contains("custom=hello"), "unknown keywords pass through:\n{post}");
    assert!(post.contains("url=blog/post.html"), "the page URL is available:\n{post}");
}

/// Off by default, because it trades incremental precision for the ability to write
/// listing pages — and that trade should be a choice.
#[test]
fn the_page_list_is_opt_in_and_widens_invalidation() {
    let root = tmpdir("pagelist");
    let src = root.join("src");
    std::fs::create_dir_all(src.join("templates")).unwrap();
    write_site(&src);
    std::fs::write(
        src.join("org-ssg.toml"),
        "[templates]\nexpose_page_list = true\n",
    )
    .unwrap();
    std::fs::write(
        src.join("templates/base.html"),
        "<html><body><ul>{% for p in pages %}<li>{{ p.title }}</li>{% endfor %}</ul>\
         {{ body | safe }}</body></html>",
    )
    .unwrap();
    let out = root.join("out");
    build(&src, &out);

    let home = page(&out, "index.html");
    for title in ["Home", "About", "A Post"] {
        assert!(home.contains(title), "an index can list {title}:\n{home}");
    }

    // With every page visible to every template, adding one must re-render them all —
    // the opposite of the default, and the documented cost of turning this on.
    std::fs::write(src.join("blog/second.org"), "#+TITLE: Second\n\nBody.\n").unwrap();
    let report = build(&src, &out);
    assert_eq!(
        report.rendered.len(),
        4,
        "with the page list exposed, adding a page re-renders the site"
    );
}

// ---------------------------------------------------------------------------
// Output settings
// ---------------------------------------------------------------------------

/// The default layout renders the page title as `<h1>`, so section headings belong
/// beneath it — which is also what Emacs does by default.
#[test]
fn heading_offset_shifts_content_headings_below_the_page_title() {
    let root = tmpdir("hoffset");
    let src = root.join("src");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(src.join("index.org"), "#+TITLE: T\n\n* Section\n\nBody.\n").unwrap();
    let out = root.join("out");
    build(&src, &out);
    assert!(
        page(&out, "index.html").contains("<h2 id=\"section\">"),
        "a level-1 org heading renders as <h2> by default"
    );

    std::fs::write(src.join("org-ssg.toml"), "[html]\nheading_offset = 0\n").unwrap();
    let out2 = root.join("out2");
    build(&src, &out2);
    assert!(
        page(&out2, "index.html").contains("<h1 id=\"section\">"),
        "offset 0 leaves headings where the document put them"
    );
}

/// An unknown theme silently produces an empty stylesheet, which looks exactly like
/// highlighting being broken. Naming the valid options turns a mystery into a typo.
#[test]
fn an_unknown_highlight_theme_is_rejected_with_the_available_ones() {
    let root = tmpdir("theme");
    let src = root.join("src");
    std::fs::create_dir_all(&src).unwrap();
    write_site(&src);
    std::fs::write(src.join("org-ssg.toml"), "[highlight]\ntheme = \"nope\"\n").unwrap();

    let err = build_site(&src, &root.join("out"), &BuildOptions::default())
        .expect_err("unknown theme must fail");
    let message = format!("{err:#}");
    assert!(message.contains("nope"), "names the bad theme: {message}");
    assert!(
        message.contains("InspiredGitHub"),
        "lists what is available: {message}"
    );
}

// ---------------------------------------------------------------------------
// Discovery
// ---------------------------------------------------------------------------

/// `org-ssg build . -o _site` is the obvious thing to type. Without excluding the output
/// directory, the build copies its own output back into itself, growing `_site/_site/…`
/// on every run.
#[test]
fn an_output_directory_inside_the_source_is_not_swallowed() {
    let root = tmpdir("nested");
    let src = root.join("src");
    std::fs::create_dir_all(&src).unwrap();
    write_site(&src);
    let out = src.join("_site");

    for _ in 0..3 {
        build(&src, &out);
    }
    assert!(!out.join("_site").exists(), "output must not nest inside itself");

    let report = build(&src, &out);
    assert_eq!(report.pages.len(), 3, "still exactly the source pages");
    assert!(
        report.assets.is_empty(),
        "no output file is mistaken for an asset: {:?}",
        report.assets
    );
}

/// A source directory is very often a git repository. Publishing `.git` alongside the
/// homepage leaks a project's entire history.
#[test]
fn dot_directories_and_build_inputs_are_never_published() {
    let root = tmpdir("dotfiles");
    let src = root.join("src");
    std::fs::create_dir_all(src.join(".git")).unwrap();
    std::fs::create_dir_all(src.join("templates")).unwrap();
    write_site(&src);
    std::fs::write(src.join(".git/config"), "[remote]\nurl = private\n").unwrap();
    std::fs::write(src.join(".env"), "SECRET=hunter2\n").unwrap();
    std::fs::write(src.join("org-ssg.toml"), "[site]\ntitle = \"T\"\n").unwrap();
    std::fs::write(src.join("templates/base.html"), "<html>{{ body | safe }}</html>").unwrap();
    std::fs::write(src.join("style.css"), "body{}\n").unwrap();
    let out = root.join("out");

    let report = build(&src, &out);
    assert!(!out.join(".git").exists(), ".git must never be published");
    assert!(!out.join(".env").exists(), "dotfiles must never be published");
    assert!(
        !out.join("org-ssg.toml").exists(),
        "the config is a build input, not content"
    );
    assert!(
        !out.join("templates").exists(),
        "templates are build inputs, not content"
    );
    assert_eq!(
        report.assets,
        vec![Utf8PathBuf::from("style.css")],
        "genuine assets still copy through"
    );
}
