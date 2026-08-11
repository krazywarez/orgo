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

// ---------------------------------------------------------------------------
// Generated listing pages
// ---------------------------------------------------------------------------

/// A site with dated posts, a listing template, and a collection configured over them.
fn write_blog(src: &Utf8PathBuf, extra_config: &str) {
    std::fs::create_dir_all(src.join("blog")).unwrap();
    std::fs::create_dir_all(src.join("templates")).unwrap();
    std::fs::write(src.join("index.org"), "#+TITLE: Home\n\nWelcome.\n").unwrap();
    for (name, title, date) in [
        ("old", "Older Post", "<2024-01-02 Tue>"),
        ("new", "Newer Post", "[2025-06-30 Mon 09:15:00]"),
        ("mid", "Middle Post", "2024-08-05"),
    ] {
        std::fs::write(
            src.join(format!("blog/{name}.org")),
            format!("#+TITLE: {title}\n#+DATE: {date}\n\nBody.\n"),
        )
        .unwrap();
    }
    std::fs::write(
        src.join("templates/list.html"),
        "<html><body><h1>{{ page.title }}</h1><ul>\
         {% for p in pages %}<li>{{ p.date_iso }}|{{ p.title }}|{{ root }}{{ p.url }}</li>\
         {% endfor %}</ul></body></html>",
    )
    .unwrap();
    std::fs::write(
        src.join("org-ssg.toml"),
        format!(
            "[[collections]]\nsource = \"blog\"\noutput = \"blog/index.html\"\n\
             template = \"list.html\"\ntitle = \"Blog\"\n{extra_config}"
        ),
    )
    .unwrap();
}

/// The whole point: an output file with no source `.org` behind it.
#[test]
fn a_collection_generates_a_listing_page_sorted_newest_first() {
    let root = tmpdir("listing");
    let src = root.join("src");
    std::fs::create_dir_all(&src).unwrap();
    write_blog(&src, "");
    let out = root.join("out");
    let report = build(&src, &out);

    assert!(
        report.pages.contains(&Utf8PathBuf::from("blog/index.html")),
        "the listing page is part of the build: {:?}",
        report.pages
    );

    let listing = page(&out, "blog/index.html");
    let order: Vec<&str> = ["Newer Post", "Middle Post", "Older Post"]
        .into_iter()
        .filter(|t| listing.contains(t))
        .collect();
    assert_eq!(
        order,
        vec!["Newer Post", "Middle Post", "Older Post"],
        "all three posts appear:\n{listing}"
    );
    let pos = |t: &str| listing.find(t).unwrap();
    assert!(
        pos("Newer Post") < pos("Middle Post") && pos("Middle Post") < pos("Older Post"),
        "newest first by default:\n{listing}"
    );
    assert!(!listing.contains("Home"), "only the collection's pages are listed");
}

/// Org dates arrive as `[2025-06-30 Mon 09:15:00]`, `<2024-01-02 Tue>` or bare
/// `2024-08-05`. A listing needs one key it can sort and print.
#[test]
fn dates_are_normalized_from_every_org_shape() {
    let root = tmpdir("dates");
    let src = root.join("src");
    std::fs::create_dir_all(&src).unwrap();
    write_blog(&src, "");
    let out = root.join("out");
    build(&src, &out);

    let listing = page(&out, "blog/index.html");
    for iso in ["2025-06-30", "2024-08-05", "2024-01-02"] {
        assert!(listing.contains(iso), "{iso} normalized out of its org syntax:\n{listing}");
    }
}

#[test]
fn sort_and_order_are_configurable() {
    let root = tmpdir("sortorder");
    let src = root.join("src");
    std::fs::create_dir_all(&src).unwrap();
    write_blog(&src, "sort = \"title\"\norder = \"asc\"\n");
    let out = root.join("out");
    build(&src, &out);

    let listing = page(&out, "blog/index.html");
    let pos = |t: &str| listing.find(t).unwrap();
    assert!(
        pos("Middle Post") < pos("Newer Post") && pos("Newer Post") < pos("Older Post"),
        "ascending by title:\n{listing}"
    );
}

/// A dateless draft leading a dated archive is almost never what anyone wants.
#[test]
fn undated_pages_sort_last_whichever_direction() {
    let root = tmpdir("undated");
    let src = root.join("src");
    std::fs::create_dir_all(&src).unwrap();
    write_blog(&src, "");
    std::fs::write(src.join("blog/draft.org"), "#+TITLE: No Date Here\n\nBody.\n").unwrap();
    let out = root.join("out");
    build(&src, &out);

    let listing = page(&out, "blog/index.html");
    let undated = listing.find("No Date Here").unwrap();
    for dated in ["Newer Post", "Middle Post", "Older Post"] {
        assert!(
            listing.find(dated).unwrap() < undated,
            "{dated} must precede the undated draft:\n{listing}"
        );
    }
}

/// A listing page is exactly what a section's nav entry should point at — `/blog/`
/// rather than any one post.
#[test]
fn a_collection_can_join_the_nav() {
    let root = tmpdir("listnav");
    let src = root.join("src");
    std::fs::create_dir_all(&src).unwrap();
    write_blog(&src, "nav = true\n");
    let out = root.join("out");
    build(&src, &out);

    let home_nav = nav_of(&page(&out, "index.html"));
    assert!(
        home_nav.contains("blog/index.html"),
        "the listing page is in the nav:\n{home_nav}"
    );
    // And the URL has to be right from a nested page too.
    let post = page(&out, "blog/new.html");
    assert!(
        nav_of(&post).contains("href=\"index.html\"") || nav_of(&post).contains("blog/index.html"),
        "the nav link resolves from a nested page:\n{}",
        nav_of(&post)
    );
}

/// A listing page depends on every page it lists — and on nothing else. Adding a post
/// must re-render the index without re-rendering the rest of the site.
#[test]
fn adding_a_post_rebuilds_only_the_listing_and_the_post() {
    let root = tmpdir("listinc");
    let src = root.join("src");
    std::fs::create_dir_all(&src).unwrap();
    write_blog(&src, "");
    let out = root.join("out");

    build(&src, &out);
    let second = build(&src, &out);
    assert!(
        second.rendered.is_empty(),
        "an unchanged rebuild renders nothing, including the listing: {:?}",
        second.rendered
    );

    std::fs::write(
        src.join("blog/fresh.org"),
        "#+TITLE: Fresh Post\n#+DATE: 2026-01-01\n\nBody.\n",
    )
    .unwrap();
    let report = build(&src, &out);

    let mut rendered = report.rendered.clone();
    rendered.sort();
    assert_eq!(
        rendered,
        vec![
            Utf8PathBuf::from("blog/fresh.html"),
            Utf8PathBuf::from("blog/index.html")
        ],
        "exactly the new post and the listing it belongs to"
    );
    assert!(
        page(&out, "blog/index.html").contains("Fresh Post"),
        "and the listing actually picked it up"
    );
}

/// Editing a post's body changes no listing metadata, so the index must not churn.
#[test]
fn editing_a_post_body_does_not_rebuild_the_listing() {
    let root = tmpdir("listbody");
    let src = root.join("src");
    std::fs::create_dir_all(&src).unwrap();
    write_blog(&src, "");
    let out = root.join("out");
    build(&src, &out);

    std::fs::write(
        src.join("blog/mid.org"),
        "#+TITLE: Middle Post\n#+DATE: 2024-08-05\n\nEdited body.\n",
    )
    .unwrap();
    let report = build(&src, &out);

    assert_eq!(
        report.rendered,
        vec![Utf8PathBuf::from("blog/mid.html")],
        "only the post itself; the listing shows unchanged metadata"
    );
}

/// Retitling a post *does* change the listing, since the title is what it displays.
#[test]
fn retitling_a_post_rebuilds_the_listing() {
    let root = tmpdir("listtitle");
    let src = root.join("src");
    std::fs::create_dir_all(&src).unwrap();
    write_blog(&src, "");
    let out = root.join("out");
    build(&src, &out);

    std::fs::write(
        src.join("blog/mid.org"),
        "#+TITLE: Renamed Post\n#+DATE: 2024-08-05\n\nBody.\n",
    )
    .unwrap();
    let report = build(&src, &out);

    assert!(
        report.rendered.contains(&Utf8PathBuf::from("blog/index.html")),
        "the listing must follow a title change: {:?}",
        report.rendered
    );
    assert!(page(&out, "blog/index.html").contains("Renamed Post"));
}

/// A feed is a listing page with an XML template, not a separate feature — which is why
/// templates are loaded by full filename and any extension.
#[test]
fn a_feed_is_just_a_listing_page_with_an_xml_template() {
    let root = tmpdir("feed");
    let src = root.join("src");
    std::fs::create_dir_all(&src).unwrap();
    write_blog(&src, "");
    std::fs::write(
        src.join("templates/feed.xml"),
        "<?xml version=\"1.0\"?><rss version=\"2.0\"><channel><title>{{ site.title }}</title>\
         {% for p in pages %}<item><title>{{ p.title }}</title>\
         <pubDate>{{ p.date_iso }}</pubDate></item>{% endfor %}</channel></rss>",
    )
    .unwrap();
    let mut config = std::fs::read_to_string(src.join("org-ssg.toml")).unwrap();
    config.push_str(
        "\n[[collections]]\nsource = \"blog\"\noutput = \"feed.xml\"\n\
         template = \"feed.xml\"\ntitle = \"Feed\"\n",
    );
    std::fs::write(src.join("org-ssg.toml"), config).unwrap();
    let out = root.join("out");
    build(&src, &out);

    let feed = page(&out, "feed.xml");
    assert!(feed.starts_with("<?xml"), "an XML document, not HTML:\n{feed}");
    assert!(feed.contains("<pubDate>2025-06-30</pubDate>"), "entries carry dates:\n{feed}");
}

/// A listing template can inherit the site layout instead of duplicating it.
#[test]
fn a_listing_template_can_extend_the_base_layout() {
    let root = tmpdir("listextends");
    let src = root.join("src");
    std::fs::create_dir_all(&src).unwrap();
    write_blog(&src, "");
    std::fs::write(
        src.join("templates/base.html"),
        "<html><body class=\"shared\">{% block main %}{{ body | safe }}{% endblock %}</body></html>",
    )
    .unwrap();
    std::fs::write(
        src.join("templates/list.html"),
        "{% extends \"base.html\" %}{% block main %}<ul>\
         {% for p in pages %}<li>{{ p.title }}</li>{% endfor %}</ul>{% endblock %}",
    )
    .unwrap();
    let out = root.join("out");
    build(&src, &out);

    let listing = page(&out, "blog/index.html");
    assert!(listing.contains("class=\"shared\""), "inherits the layout:\n{listing}");
    assert!(listing.contains("Newer Post"), "and adds its own content:\n{listing}");
}

/// URLs are most of a template's output. Escaping `/` as `&#x2f;` is valid but makes
/// every link unreadable; escaping user content is not optional.
#[test]
fn urls_stay_readable_while_user_content_is_still_escaped() {
    let root = tmpdir("escaping");
    let src = root.join("src");
    std::fs::create_dir_all(&src).unwrap();
    write_blog(&src, "");
    std::fs::write(
        src.join("blog/evil.org"),
        "#+TITLE: <script>alert(1)</script>\n#+DATE: 2026-02-02\n\nBody.\n",
    )
    .unwrap();
    let out = root.join("out");
    build(&src, &out);

    let listing = page(&out, "blog/index.html");
    assert!(listing.contains("../blog/new.html"), "URLs read as URLs:\n{listing}");
    assert!(!listing.contains("&#x2f;"), "no escaped slashes:\n{listing}");
    assert!(
        listing.contains("&lt;script&gt;"),
        "a title is user content and stays escaped:\n{listing}"
    );
    assert!(!listing.contains("<script>"), "never unescaped:\n{listing}");
}

/// Two generated pages writing the same file, or a listing writing over a real page,
/// silently loses one of them.
#[test]
fn colliding_collection_outputs_are_rejected() {
    let root = tmpdir("listcollide");
    let src = root.join("src");
    std::fs::create_dir_all(&src).unwrap();
    write_blog(&src, "");

    let mut config = std::fs::read_to_string(src.join("org-ssg.toml")).unwrap();
    config.push_str("\n[[collections]]\nsource = \"\"\noutput = \"blog/index.html\"\n");
    std::fs::write(src.join("org-ssg.toml"), &config).unwrap();
    let err = build_site(&src, &root.join("out"), &BuildOptions::default())
        .expect_err("two collections writing one file must fail");
    assert!(format!("{err:#}").contains("blog/index.html"), "{err:#}");

    // And a listing that would overwrite a real page.
    std::fs::write(
        src.join("org-ssg.toml"),
        "[[collections]]\nsource = \"blog\"\noutput = \"index.html\"\ntemplate = \"list.html\"\n",
    )
    .unwrap();
    let err = build_site(&src, &root.join("out2"), &BuildOptions::default())
        .expect_err("a listing over a real page must fail");
    assert!(format!("{err:#}").contains("index.org"), "names the page it would replace: {err:#}");
}

/// A missing template is a typo; listing what exists turns it into a one-second fix.
#[test]
fn a_missing_collection_template_names_the_ones_that_exist() {
    let root = tmpdir("listnotpl");
    let src = root.join("src");
    std::fs::create_dir_all(&src).unwrap();
    write_blog(&src, "");
    std::fs::write(
        src.join("org-ssg.toml"),
        "[[collections]]\nsource = \"blog\"\noutput = \"blog/index.html\"\ntemplate = \"nope.html\"\n",
    )
    .unwrap();

    let err = build_site(&src, &root.join("out"), &BuildOptions::default())
        .expect_err("missing template must fail");
    let message = format!("{err:#}");
    assert!(message.contains("nope.html"), "names the missing one: {message}");
    assert!(message.contains("list.html"), "lists what is available: {message}");
}

// ---------------------------------------------------------------------------
// Grouped collections: tag pages and the tag index
// ---------------------------------------------------------------------------

/// Posts carrying tags, a per-tag template, a tag-index template, and a grouped
/// collection over them.
fn write_tagged_blog(src: &Utf8PathBuf, extra: &str) {
    std::fs::create_dir_all(src.join("blog")).unwrap();
    std::fs::create_dir_all(src.join("templates")).unwrap();
    std::fs::write(src.join("index.org"), "#+TITLE: Home\n\nWelcome.\n").unwrap();
    for (name, title, date, tags) in [
        ("a", "Post A", "2024-01-01", ":rust:web:"),
        ("b", "Post B", "2024-02-02", ":rust:"),
        ("c", "Post C", "2024-03-03", ":emacs:"),
        ("d", "Post D", "2024-04-04", ""),
    ] {
        let filetags = if tags.is_empty() {
            String::new()
        } else {
            format!("#+FILETAGS: {tags}\n")
        };
        std::fs::write(
            src.join(format!("blog/{name}.org")),
            format!("#+TITLE: {title}\n#+DATE: {date}\n{filetags}\nBody.\n"),
        )
        .unwrap();
    }
    std::fs::write(
        src.join("templates/tag.html"),
        "<html><body><h1>{{ page.title }}</h1><p>slug={{ group.slug }} count={{ group.count }}</p>\
         <ul>{% for p in pages %}<li>{{ p.title }}</li>{% endfor %}</ul></body></html>",
    )
    .unwrap();
    std::fs::write(
        src.join("templates/tags.html"),
        "<html><body><h1>{{ page.title }}</h1><ul>\
         {% for g in groups %}<li>{{ g.name }}={{ g.count }}@{{ root }}{{ g.url }}</li>\
         {% endfor %}</ul></body></html>",
    )
    .unwrap();
    std::fs::write(
        src.join("org-ssg.toml"),
        format!(
            "[[collections]]\nsource = \"blog\"\ngroup_by = \"tags\"\n\
             output = \"tags/{{tag}}.html\"\ntemplate = \"tag.html\"\ntitle = \"Tagged: {{tag}}\"\n\
             index_output = \"tags/index.html\"\nindex_template = \"tags.html\"\n\
             index_title = \"All tags\"\n{extra}"
        ),
    )
    .unwrap();
}

/// One collection, many outputs — the shape the earlier listing feature could not express.
#[test]
fn a_grouped_collection_emits_one_page_per_tag() {
    let root = tmpdir("tags");
    let src = root.join("src");
    std::fs::create_dir_all(&src).unwrap();
    write_tagged_blog(&src, "");
    let out = root.join("out");
    build(&src, &out);

    for (tag, expected) in [("rust", vec!["Post A", "Post B"]), ("emacs", vec!["Post C"])] {
        let html = page(&out, &format!("tags/{tag}.html"));
        for title in &expected {
            assert!(html.contains(title), "{tag} lists {title}:\n{html}");
        }
        assert!(
            html.contains(&format!("count={}", expected.len())),
            "{tag} knows its own size:\n{html}"
        );
    }
    assert!(
        !out.join("tags/.html").exists(),
        "an untagged post creates no empty group"
    );
    assert!(
        !page(&out, "tags/rust.html").contains("Post C"),
        "a tag page lists only its own posts"
    );
}

/// The index lists the groups themselves, not the pages.
#[test]
fn the_tag_index_lists_every_tag_with_counts() {
    let root = tmpdir("tagindex");
    let src = root.join("src");
    std::fs::create_dir_all(&src).unwrap();
    write_tagged_blog(&src, "");
    let out = root.join("out");
    build(&src, &out);

    let index = page(&out, "tags/index.html");
    assert!(index.contains("All tags"), "uses index_title:\n{index}");
    assert!(index.contains("rust=2@../tags/rust.html"), "counts and links:\n{index}");
    assert!(index.contains("emacs=1@"), "every tag appears:\n{index}");
    assert!(index.contains("web=1@"), "every tag appears:\n{index}");
    // Alphabetical, so the index reads predictably rather than in discovery order.
    let pos = |t: &str| index.find(t).unwrap();
    assert!(pos("emacs") < pos("rust") && pos("rust") < pos("web"), "sorted:\n{index}");
}

/// A tag page depends on its own posts. Adding a post tagged `rust` must not re-render
/// the `emacs` page — invalidation that scales with tag count would undo the point.
#[test]
fn adding_a_tagged_post_rebuilds_only_the_affected_pages() {
    let root = tmpdir("tagsinc");
    let src = root.join("src");
    std::fs::create_dir_all(&src).unwrap();
    write_tagged_blog(&src, "");
    let out = root.join("out");
    build(&src, &out);
    assert!(build(&src, &out).rendered.is_empty(), "unchanged rebuild renders nothing");

    std::fs::write(
        src.join("blog/e.org"),
        "#+TITLE: Post E\n#+DATE: 2024-05-05\n#+FILETAGS: :rust:\n\nBody.\n",
    )
    .unwrap();
    let report = build(&src, &out);

    let mut rendered = report.rendered.clone();
    rendered.sort();
    assert_eq!(
        rendered,
        vec![
            Utf8PathBuf::from("blog/e.html"),
            Utf8PathBuf::from("tags/index.html"),
            Utf8PathBuf::from("tags/rust.html"),
        ],
        "the post, its tag page, and the index whose counts changed — nothing else"
    );
    assert!(page(&out, "tags/rust.html").contains("Post E"));
}

/// A new tag has to produce a new page and reach the index.
#[test]
fn a_new_tag_creates_its_page_and_joins_the_index() {
    let root = tmpdir("newtag");
    let src = root.join("src");
    std::fs::create_dir_all(&src).unwrap();
    write_tagged_blog(&src, "");
    let out = root.join("out");
    build(&src, &out);
    assert!(!out.join("tags/zig.html").exists());

    std::fs::write(
        src.join("blog/f.org"),
        "#+TITLE: Post F\n#+DATE: 2024-06-06\n#+FILETAGS: :zig:\n\nBody.\n",
    )
    .unwrap();
    build(&src, &out);

    assert!(out.join("tags/zig.html").exists(), "the new tag gets a page");
    assert!(
        page(&out, "tags/index.html").contains("zig=1@"),
        "and the index knows about it"
    );
}

/// Grouping by any `#+KEYWORD:`, not just tags — same mechanism, single-valued.
#[test]
fn a_collection_can_group_by_any_keyword() {
    let root = tmpdir("groupkw");
    let src = root.join("src");
    std::fs::create_dir_all(&src).unwrap();
    write_tagged_blog(&src, "");
    std::fs::write(
        src.join("blog/a.org"),
        "#+TITLE: Post A\n#+DATE: 2024-01-01\n#+CATEGORY: Notes\n\nBody.\n",
    )
    .unwrap();
    std::fs::write(
        src.join("org-ssg.toml"),
        "[[collections]]\nsource = \"blog\"\ngroup_by = \"category\"\n\
         output = \"cat/{tag}.html\"\ntemplate = \"tag.html\"\ntitle = \"{tag}\"\n",
    )
    .unwrap();
    let out = root.join("out");
    build(&src, &out);

    assert!(out.join("cat/notes.html").exists(), "grouped by #+CATEGORY:");
    assert!(page(&out, "cat/notes.html").contains("Post A"));
}

/// A grouped collection puts its *index* in the nav. A nav listing every tag is the same
/// mistake as a nav listing every page.
#[test]
fn a_grouped_collection_contributes_its_index_to_the_nav() {
    let root = tmpdir("tagnav");
    let src = root.join("src");
    std::fs::create_dir_all(&src).unwrap();
    write_tagged_blog(&src, "nav = true\n");
    let out = root.join("out");
    build(&src, &out);

    let nav = nav_of(&page(&out, "index.html"));
    assert!(nav.contains("tags/index.html"), "the index is in the nav:\n{nav}");
    assert!(!nav.contains("tags/rust.html"), "individual tags are not:\n{nav}");
}

/// An output path with no `{tag}` would have every group overwrite one file — a config
/// that looks reasonable and silently produces one page instead of many.
#[test]
fn grouping_without_a_placeholder_is_rejected() {
    let root = tmpdir("noplaceholder");
    let src = root.join("src");
    std::fs::create_dir_all(&src).unwrap();
    write_tagged_blog(&src, "");
    std::fs::write(
        src.join("org-ssg.toml"),
        "[[collections]]\nsource = \"blog\"\ngroup_by = \"tags\"\n\
         output = \"tags/all.html\"\ntemplate = \"tag.html\"\n",
    )
    .unwrap();

    let err = build_site(&src, &root.join("out"), &BuildOptions::default())
        .expect_err("grouping without {tag} must fail");
    assert!(format!("{err:#}").contains("{tag}"), "explains what is missing: {err:#}");
}

/// Two tags that differ only in punctuation slugify to the same path, and one page would
/// silently overwrite the other.
#[test]
fn tags_that_collide_in_a_url_are_rejected() {
    let root = tmpdir("tagcollide");
    let src = root.join("src");
    std::fs::create_dir_all(&src).unwrap();
    write_tagged_blog(&src, "");
    std::fs::write(
        src.join("blog/a.org"),
        "#+TITLE: Post A\n#+DATE: 2024-01-01\n#+FILETAGS: :web_dev:\n\nBody.\n",
    )
    .unwrap();
    std::fs::write(
        src.join("blog/b.org"),
        "#+TITLE: Post B\n#+DATE: 2024-02-02\n#+FILETAGS: :web@dev:\n\nBody.\n",
    )
    .unwrap();

    let err = build_site(&src, &root.join("out"), &BuildOptions::default())
        .expect_err("colliding tag slugs must fail");
    let message = format!("{err:#}");
    assert!(message.contains("web_dev") && message.contains("web@dev"), "{message}");
}

// ---------------------------------------------------------------------------
// Pagination
// ---------------------------------------------------------------------------

/// A blog of `count` dated posts with a paginating collection over them.
fn write_paginated_blog(src: &Utf8PathBuf, count: usize, extra: &str) {
    std::fs::create_dir_all(src.join("blog")).unwrap();
    std::fs::create_dir_all(src.join("templates")).unwrap();
    std::fs::write(src.join("index.org"), "#+TITLE: Home\n\nWelcome.\n").unwrap();
    for i in 0..count {
        std::fs::write(
            src.join(format!("blog/p{i:02}.org")),
            format!(
                "#+TITLE: Post {i:02}\n#+DATE: 2024-01-{:02}\n\nBody.\n",
                i + 1
            ),
        )
        .unwrap();
    }
    std::fs::write(
        src.join("templates/list.html"),
        "<html><body><h1>{{ page.title }}</h1>\
         <ul>{% for p in pages %}<li>{{ p.title }}</li>{% endfor %}</ul>\
         {% if paginator %}<p>page {{ paginator.current }}/{{ paginator.total }} \
         of {{ paginator.total_entries }}</p>\
         {% if paginator.prev_url %}<a id=\"prev\" href=\"{{ paginator.prev_url }}\">p</a>{% endif %}\
         {% if paginator.next_url %}<a id=\"next\" href=\"{{ paginator.next_url }}\">n</a>{% endif %}\
         <nav>{% for pg in paginator.pages %}<a href=\"{{ pg.url }}\"{% if pg.current %} \
         class=\"here\"{% endif %}>{{ pg.number }}</a>{% endfor %}</nav>{% endif %}</body></html>",
    )
    .unwrap();
    std::fs::write(
        src.join("org-ssg.toml"),
        format!(
            "[[collections]]\nsource = \"blog\"\noutput = \"blog/index.html\"\n\
             template = \"list.html\"\ntitle = \"Blog\"\n{extra}"
        ),
    )
    .unwrap();
}

/// Page 1 keeps the collection's `output`, so a section's canonical URL never moves as
/// its page count changes.
#[test]
fn pagination_splits_entries_and_keeps_page_one_canonical() {
    let root = tmpdir("paginate");
    let src = root.join("src");
    std::fs::create_dir_all(&src).unwrap();
    write_paginated_blog(&src, 7, "paginate = 3\npaginate_output = \"blog/page/{n}.html\"\n");
    let out = root.join("out");
    build(&src, &out);

    assert!(out.join("blog/index.html").exists(), "page 1 is the canonical URL");
    for n in [2, 3] {
        assert!(out.join(format!("blog/page/{n}.html")).exists(), "page {n} exists");
    }
    assert!(!out.join("blog/page/4.html").exists(), "7 entries at 3/page is 3 pages");
    assert!(!out.join("blog/page/1.html").exists(), "page 1 is not duplicated");

    // Newest first, so page 1 holds posts 06, 05, 04.
    let first = page(&out, "blog/index.html");
    assert!(first.contains("page 1/3 of 7"), "paginator counts:\n{first}");
    assert!(first.contains("Post 06") && first.contains("Post 04"));
    assert!(!first.contains("Post 03"), "page 1 holds only its own slice:\n{first}");

    let last = page(&out, "blog/page/3.html");
    assert!(last.contains("Post 00"), "the remainder lands on the last page:\n{last}");
    assert_eq!(last.matches("<li>").count(), 1, "7 = 3 + 3 + 1");
}

/// Paginator URLs have to be relative to the page carrying them, and pages 2..N sit at a
/// different depth than page 1.
#[test]
fn paginator_urls_resolve_from_each_pages_own_depth() {
    let root = tmpdir("pageurls");
    let src = root.join("src");
    std::fs::create_dir_all(&src).unwrap();
    write_paginated_blog(&src, 7, "paginate = 3\npaginate_output = \"blog/page/{n}.html\"\n");
    let out = root.join("out");
    build(&src, &out);

    let first = page(&out, "blog/index.html");
    assert!(first.contains("id=\"next\" href=\"page/2.html\""), "down a level:\n{first}");
    assert!(!first.contains("id=\"prev\""), "page 1 has no previous");

    let middle = page(&out, "blog/page/2.html");
    assert!(middle.contains("id=\"prev\" href=\"../index.html\""), "back up:\n{middle}");
    assert!(middle.contains("id=\"next\" href=\"3.html\""), "sideways:\n{middle}");

    let last = page(&out, "blog/page/3.html");
    assert!(!last.contains("id=\"next\""), "the last page has no next:\n{last}");
}

/// The numbered strip marks the page it is on, so a template does not compare numbers.
#[test]
fn the_paginator_exposes_a_numbered_page_list() {
    let root = tmpdir("pagenums");
    let src = root.join("src");
    std::fs::create_dir_all(&src).unwrap();
    write_paginated_blog(&src, 7, "paginate = 3\npaginate_output = \"blog/page/{n}.html\"\n");
    let out = root.join("out");
    build(&src, &out);

    let second = page(&out, "blog/page/2.html");
    assert!(second.contains(">1</a>") && second.contains(">3</a>"), "all pages listed");
    assert!(
        second.contains("class=\"here\">2</a>"),
        "the current page is marked:\n{second}"
    );
}

/// An unpaginated collection must not grow a paginator, so `{% if paginator %}` is a
/// reliable test in a shared template.
#[test]
fn an_unpaginated_collection_has_no_paginator() {
    let root = tmpdir("nopaginator");
    let src = root.join("src");
    std::fs::create_dir_all(&src).unwrap();
    write_paginated_blog(&src, 4, "");
    let out = root.join("out");
    build(&src, &out);

    let listing = page(&out, "blog/index.html");
    assert!(!listing.contains("page 1/"), "no paginator block:\n{listing}");
    assert_eq!(listing.matches("<li>").count(), 4, "everything on one page");
}

/// A section with nothing in it should be a page saying so, not a 404.
#[test]
fn an_empty_paginated_collection_still_emits_page_one() {
    let root = tmpdir("pageempty");
    let src = root.join("src");
    std::fs::create_dir_all(&src).unwrap();
    write_paginated_blog(&src, 0, "paginate = 3\npaginate_output = \"blog/page/{n}.html\"\n");
    let out = root.join("out");
    build(&src, &out);

    let listing = page(&out, "blog/index.html");
    assert!(listing.contains("page 1/1 of 0"), "one empty page:\n{listing}");
    assert!(!out.join("blog/page/2.html").exists());
}

/// Grouped and paginated together: each group paginates independently, which is why
/// `paginate_output` needs both placeholders.
#[test]
fn groups_paginate_independently() {
    let root = tmpdir("pagegroups");
    let src = root.join("src");
    std::fs::create_dir_all(&src).unwrap();
    write_paginated_blog(&src, 0, "");
    for (name, tag, n) in [("a", "rust", 0), ("b", "rust", 1), ("c", "rust", 2), ("d", "web", 3)] {
        std::fs::write(
            src.join(format!("blog/{name}.org")),
            format!("#+TITLE: Post {name}\n#+DATE: 2024-01-0{}\n#+FILETAGS: :{tag}:\n\nBody.\n", n + 1),
        )
        .unwrap();
    }
    std::fs::write(
        src.join("org-ssg.toml"),
        "[[collections]]\nsource = \"blog\"\ngroup_by = \"tags\"\n\
         output = \"tags/{tag}.html\"\ntemplate = \"list.html\"\ntitle = \"{tag}\"\n\
         paginate = 2\npaginate_output = \"tags/{tag}/page/{n}.html\"\n",
    )
    .unwrap();
    let out = root.join("out");
    build(&src, &out);

    assert!(out.join("tags/rust.html").exists(), "3 rust posts, page 1");
    assert!(out.join("tags/rust/page/2.html").exists(), "3 rust posts at 2/page needs page 2");
    assert!(out.join("tags/web.html").exists(), "1 web post");
    assert!(
        !out.join("tags/web/page/2.html").exists(),
        "one post needs no second page — groups paginate independently"
    );
}

/// A `paginate_output` without `{n}` would have every page overwrite one file; without
/// `{tag}` on a grouped collection, page 2 of one group would overwrite page 2 of
/// another.
#[test]
fn pagination_placeholders_are_validated() {
    let root = tmpdir("pagevalidate");
    let src = root.join("src");
    std::fs::create_dir_all(&src).unwrap();

    let cases = [
        ("paginate = 3\n", "paginate_output"),
        ("paginate = 3\npaginate_output = \"blog/more.html\"\n", "{n}"),
        ("paginate_output = \"blog/page/{n}.html\"\n", "paginate"),
    ];
    for (extra, expect) in cases {
        write_paginated_blog(&src, 4, extra);
        let err = build_site(&src, &root.join("out"), &BuildOptions::default())
            .expect_err("invalid pagination config must fail");
        let message = format!("{err:#}");
        assert!(message.contains(expect), "expected {expect:?} in: {message}");
    }

    // Grouped without {tag} in the page pattern.
    std::fs::write(
        src.join("org-ssg.toml"),
        "[[collections]]\nsource = \"blog\"\ngroup_by = \"tags\"\n\
         output = \"tags/{tag}.html\"\ntemplate = \"list.html\"\n\
         paginate = 2\npaginate_output = \"tags/page/{n}.html\"\n",
    )
    .unwrap();
    let err = build_site(&src, &root.join("out2"), &BuildOptions::default())
        .expect_err("grouped pagination without {tag} must fail");
    assert!(format!("{err:#}").contains("{tag}"), "{err:#}");
}

/// Adding a post shifts every entry across page boundaries, so all pages of that
/// collection change — but nothing else does. And when the count shrinks, the pages that
/// no longer exist have to be deleted rather than left serving stale content.
#[test]
fn page_count_changes_add_and_remove_page_files() {
    let root = tmpdir("pageshrink");
    let src = root.join("src");
    std::fs::create_dir_all(&src).unwrap();
    write_paginated_blog(&src, 7, "paginate = 3\npaginate_output = \"blog/page/{n}.html\"\n");
    let out = root.join("out");
    build(&src, &out);
    assert!(build(&src, &out).rendered.is_empty(), "unchanged rebuild renders nothing");
    assert!(out.join("blog/page/3.html").exists());

    // Drop below two pages' worth.
    for i in 2..7 {
        std::fs::remove_file(src.join(format!("blog/p{i:02}.org"))).unwrap();
    }
    build(&src, &out);

    assert!(
        !out.join("blog/page/2.html").exists() && !out.join("blog/page/3.html").exists(),
        "pages that no longer exist are deleted, not left serving stale posts"
    );
    let first = page(&out, "blog/index.html");
    assert!(first.contains("page 1/1 of 2"), "the paginator reflects the new size:\n{first}");
}
