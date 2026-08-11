//! Incremental build layer gates (spec §4, Phase 6). These are the hard correctness
//! tests the incremental design exists to satisfy:
//!
//! - **Byte-equivalence**: a full (`--no-cache`) build and an incremental rebuild of an
//!   unchanged site produce byte-identical output, and the second build re-renders ZERO
//!   pages (spec §4.5, R5).
//! - **Edit-one-file**: editing a page re-renders exactly that page plus the pages that
//!   link into it — no more, no less (spec §4.3).
//! - **Renamed-heading**: renaming a heading a cross-page link points at invalidates the
//!   linking page and updates its emitted anchor (spec §4.3, R2 — the load-bearing case).
//! - **Cache fallback**: a version bump, a missing cache, or a corrupt cache all fall
//!   back to a full rebuild (spec §4.5).

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU32, Ordering};

use camino::Utf8PathBuf;

use org_ssg::incremental::{manifest_path, Manifest, CACHE_FORMAT_VERSION};
use org_ssg::site::{build_site, BuildOptions};

/// A fresh, empty temp directory unique to this process + call.
fn tmpdir(tag: &str) -> Utf8PathBuf {
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    let base = Utf8PathBuf::from_path_buf(std::env::temp_dir())
        .expect("utf-8 temp dir")
        .join(format!("org-ssg-it-{}-{tag}-{n}", std::process::id()));
    if base.exists() {
        std::fs::remove_dir_all(&base).unwrap();
    }
    std::fs::create_dir_all(&base).unwrap();
    base
}

fn write(dir: &Utf8PathBuf, name: &str, content: &str) {
    std::fs::write(dir.join(name), content).unwrap();
}

/// Every output file (relative path → bytes) except the cache manifest, which is an
/// internal artifact with non-deterministic map ordering.
fn output_files(out: &Utf8PathBuf) -> BTreeMap<String, Vec<u8>> {
    let mut map = BTreeMap::new();
    for entry in walkdir::WalkDir::new(out).sort_by_file_name() {
        let entry = entry.unwrap();
        if !entry.file_type().is_file() {
            continue;
        }
        let path = Utf8PathBuf::from_path_buf(entry.path().to_owned()).unwrap();
        if path.file_name() == Some(".org-ssg-cache.json") {
            continue;
        }
        let rel = path.strip_prefix(out).unwrap().to_string();
        map.insert(rel, std::fs::read(&path).unwrap());
    }
    map
}

fn out(p: &str) -> Utf8PathBuf {
    Utf8PathBuf::from(p)
}

/// Two linked pages: `b.org` links to a `:CUSTOM_ID:` heading in `a.org`, plus a css asset.
fn write_linked_site(src: &Utf8PathBuf) {
    write(
        src,
        "a.org",
        "#+TITLE: A\n\n* Setup\n:PROPERTIES:\n:CUSTOM_ID: setup\n:END:\nOriginal body.\n",
    );
    write(src, "b.org", "#+TITLE: B\n\nSee [[#setup][the setup]].\n");
    write(src, "style.css", "body { color: black; }\n");
}

#[test]
fn full_and_incremental_are_byte_identical_and_second_build_renders_nothing() {
    let root = tmpdir("byteeq");
    let src = root.join("src");
    std::fs::create_dir_all(&src).unwrap();
    write_linked_site(&src);

    // Full build (cache bypassed) to a reference directory.
    let full = root.join("full");
    let rfull = build_site(
        &src,
        &full,
        &BuildOptions {
            no_cache: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(rfull.rendered.len(), 2, "full build renders every page");

    // Incremental directory: first build populates the cache and renders everything.
    let inc = root.join("inc");
    let r1 = build_site(&src, &inc, &BuildOptions::default()).unwrap();
    assert_eq!(r1.rendered.len(), 2, "first incremental build renders all");

    // Second incremental build of the UNCHANGED site must re-render ZERO pages.
    let r2 = build_site(&src, &inc, &BuildOptions::default()).unwrap();
    assert!(
        r2.rendered.is_empty(),
        "unchanged rebuild must render nothing, rendered: {:?}",
        r2.rendered
    );
    assert_eq!(r2.skipped.len(), 2, "both pages reused from cache");

    // Full output == incremental output, byte for byte.
    assert_eq!(
        output_files(&full),
        output_files(&inc),
        "incremental output must be byte-identical to a full build"
    );
}

#[test]
fn editing_a_page_rebuilds_it_and_its_linkers_exactly() {
    let root = tmpdir("editone");
    let src = root.join("src");
    std::fs::create_dir_all(&src).unwrap();
    write_linked_site(&src);
    let out_dir = root.join("out");

    // Prime the cache.
    build_site(&src, &out_dir, &BuildOptions::default()).unwrap();

    // Edit a.org's body (not its heading/custom-id): b.org links into a.org, so the
    // invalidation set is exactly {a, b} — b is re-rendered because it links to a.
    write(
        &src,
        "a.org",
        "#+TITLE: A\n\n* Setup\n:PROPERTIES:\n:CUSTOM_ID: setup\n:END:\nEdited body.\n",
    );
    let r = build_site(&src, &out_dir, &BuildOptions::default()).unwrap();

    let mut rendered = r.rendered.clone();
    rendered.sort();
    assert_eq!(
        rendered,
        vec![out("a.html"), out("b.html")],
        "editing a.org re-renders exactly a.html and its linker b.html"
    );
    assert_eq!(r.skipped, Vec::<Utf8PathBuf>::new(), "nothing else exists to skip");
}

#[test]
fn editing_a_leaf_page_rebuilds_only_itself() {
    let root = tmpdir("editleaf");
    let src = root.join("src");
    std::fs::create_dir_all(&src).unwrap();
    write_linked_site(&src);
    let out_dir = root.join("out");

    build_site(&src, &out_dir, &BuildOptions::default()).unwrap();

    // b.org has NO inbound links, so editing it invalidates only itself.
    write(&src, "b.org", "#+TITLE: B\n\nSee [[#setup][the setup]]. Edited.\n");
    let r = build_site(&src, &out_dir, &BuildOptions::default()).unwrap();

    assert_eq!(r.rendered, vec![out("b.html")], "only the edited leaf re-renders");
    assert!(
        r.skipped.contains(&out("a.html")),
        "the unlinked page a.html is reused"
    );
}

#[test]
fn renaming_a_linked_heading_invalidates_the_linking_page() {
    let root = tmpdir("rename");
    let src = root.join("src");
    std::fs::create_dir_all(&src).unwrap();
    // b.org links to a.org's heading BY TEXT (the fragile `[[*Heading]]` case, spec §4.3).
    write(&src, "a.org", "#+TITLE: A\n\n* Target Heading\nBody.\n");
    write(&src, "b.org", "#+TITLE: B\n\nJump to [[*Target Heading][there]].\n");
    let out_dir = root.join("out");

    build_site(&src, &out_dir, &BuildOptions::default()).unwrap();
    let b_before = std::fs::read_to_string(out_dir.join("b.html")).unwrap();
    assert!(
        b_before.contains("a.html#target-heading"),
        "b.html should link to the target heading anchor initially:\n{b_before}"
    );

    // Rename the heading a.org owns. b.org's [[*Target Heading]] now dangles.
    write(&src, "a.org", "#+TITLE: A\n\n* Renamed Heading\nBody.\n");
    let r = build_site(&src, &out_dir, &BuildOptions::default()).unwrap();

    assert!(
        r.rendered.contains(&out("b.html")),
        "the linking page must be invalidated by the rename, rendered: {:?}",
        r.rendered
    );
    assert!(
        r.rendered.contains(&out("a.html")),
        "the renamed page itself is re-rendered"
    );

    let b_after = std::fs::read_to_string(out_dir.join("b.html")).unwrap();
    assert_ne!(b_before, b_after, "b.html's emitted link must change");
    assert!(
        !b_after.contains("a.html#target-heading"),
        "the stale cross-file anchor must be gone:\n{b_after}"
    );
    assert!(
        !r.broken.is_empty(),
        "the now-dangling link should be reported as broken"
    );
}

#[test]
fn changing_a_title_rebuilds_every_page_for_the_shared_nav() {
    let root = tmpdir("navtitle");
    let src = root.join("src");
    std::fs::create_dir_all(&src).unwrap();
    write_linked_site(&src);
    let out_dir = root.join("out");

    build_site(&src, &out_dir, &BuildOptions::default()).unwrap();
    let b_before = std::fs::read_to_string(out_dir.join("b.html")).unwrap();

    // a.org's #+TITLE feeds the nav bar on every page, so changing it must re-render all.
    write(
        &src,
        "a.org",
        "#+TITLE: A Renamed\n\n* Setup\n:PROPERTIES:\n:CUSTOM_ID: setup\n:END:\nOriginal body.\n",
    );
    let r = build_site(&src, &out_dir, &BuildOptions::default()).unwrap();

    assert_eq!(r.rendered.len(), 2, "a title change re-renders every page");
    let b_after = std::fs::read_to_string(out_dir.join("b.html")).unwrap();
    assert_ne!(b_before, b_after, "b.html's nav must reflect a.org's new title");
    assert!(b_after.contains("A Renamed"), "b.html nav shows the updated title");
}

#[test]
fn missing_cache_falls_back_to_full_rebuild() {
    let root = tmpdir("nocache");
    let src = root.join("src");
    std::fs::create_dir_all(&src).unwrap();
    write_linked_site(&src);
    let out_dir = root.join("out");

    build_site(&src, &out_dir, &BuildOptions::default()).unwrap();
    // Delete the cache manifest → next build has nothing to skip against.
    std::fs::remove_file(manifest_path(&out_dir)).unwrap();

    let r = build_site(&src, &out_dir, &BuildOptions::default()).unwrap();
    assert_eq!(r.rendered.len(), 2, "a missing cache forces a full rebuild");
    assert!(r.skipped.is_empty());
}

#[test]
fn cache_version_mismatch_falls_back_to_full_rebuild() {
    let root = tmpdir("versionbump");
    let src = root.join("src");
    std::fs::create_dir_all(&src).unwrap();
    write_linked_site(&src);
    let out_dir = root.join("out");

    build_site(&src, &out_dir, &BuildOptions::default()).unwrap();

    // Rewrite the manifest with a future cache-format version. On mismatch the loader
    // discards it (spec §4.5), so the next build re-renders everything.
    let stale = Manifest {
        format_version: CACHE_FORMAT_VERSION + 1,
        ..Default::default()
    };
    std::fs::write(
        manifest_path(&out_dir),
        serde_json::to_vec(&stale).unwrap(),
    )
    .unwrap();

    let r = build_site(&src, &out_dir, &BuildOptions::default()).unwrap();
    assert_eq!(
        r.rendered.len(),
        2,
        "a cache-format version bump forces a full rebuild"
    );
    assert!(r.skipped.is_empty());
}

#[test]
fn corrupt_cache_falls_back_without_crashing() {
    let root = tmpdir("corrupt");
    let src = root.join("src");
    std::fs::create_dir_all(&src).unwrap();
    write_linked_site(&src);
    let out_dir = root.join("out");

    build_site(&src, &out_dir, &BuildOptions::default()).unwrap();
    std::fs::write(manifest_path(&out_dir), b"this is not json{{{").unwrap();

    let r = build_site(&src, &out_dir, &BuildOptions::default()).unwrap();
    assert_eq!(r.rendered.len(), 2, "a corrupt cache is never a correctness dependency");
}

/// PARSE, RESOLVE and RENDER/EMIT all run in parallel (rayon). Parallelism must not be
/// observable in the result: the emitted bytes and the *ordering* of the build report
/// have to be identical run to run, or a build stops being reproducible.
///
/// The report ordering is the fragile half. Pushing to `rendered`/`skipped` from inside
/// the parallel pass would order them by thread scheduling, giving a non-deterministic
/// report over a deterministic site — so the report is assembled sequentially afterwards,
/// and this test is what holds that line. Enough pages to make a race likely if one exists.
#[test]
fn parallel_builds_are_deterministic_in_output_and_report_order() {
    let root = tmpdir("parallel");
    let src = root.join("src");
    std::fs::create_dir_all(src.join("deep")).unwrap();

    for i in 0..40 {
        // Cross-link every page to its neighbour so RESOLVE has real work, and give each
        // a source block so RENDER does too.
        let body = format!(
            "#+TITLE: Page {i}\n#+SLUG: page-{i}\n\n\
             See [[#anchor-{next}][the next page]].\n\n\
             * Heading {i}\n:PROPERTIES:\n:CUSTOM_ID: anchor-{i}\n:END:\n\n\
             #+BEGIN_SRC rust\nfn page_{i}() -> u32 {{ {i} }}\n#+END_SRC\n",
            next = (i + 1) % 40
        );
        let dir = if i % 3 == 0 { src.join("deep") } else { src.clone() };
        std::fs::write(dir.join(format!("p{i}.org")), body).unwrap();
    }

    let build = |out: &Utf8PathBuf| {
        build_site(
            &src,
            out,
            &BuildOptions {
                no_cache: true,
                ..Default::default()
            },
        )
        .unwrap()
    };

    let first_out = root.join("first");
    let first = build(&first_out);
    assert_eq!(first.rendered.len(), 40, "every page renders");

    for _ in 0..3 {
        let out = tmpdir("parallel-again").join("out");
        let again = build(&out);
        assert_eq!(
            first.pages, again.pages,
            "page ordering in the report must be deterministic"
        );
        assert_eq!(
            first.rendered, again.rendered,
            "rendered ordering in the report must be deterministic"
        );
        assert_eq!(
            first.skipped, again.skipped,
            "skipped ordering in the report must be deterministic"
        );
        assert_eq!(
            output_files(&first_out),
            output_files(&out),
            "emitted bytes must be identical across runs"
        );
    }
}

/// A site with pages in subdirectories.
fn write_nested_site(src: &Utf8PathBuf) {
    std::fs::create_dir_all(src.join("blog")).unwrap();
    write(src, "index.org", "#+TITLE: Home\n\nWelcome.\n");
    write(src, "about.org", "#+TITLE: About\n\nAbout me.\n");
    write(&src.join("blog"), "first.org", "#+TITLE: First Post\n\nPost body.\n");
    write(&src.join("blog"), "second.org", "#+TITLE: Second Post\n\nPost body.\n");
}

/// The nav is a map of the site's top level, not an index of its contents. Listing every
/// page made an n-page site emit n² nav links: 1,790 pages produced 284 MB of output,
/// nearly all of it nav.
#[test]
fn nav_lists_only_top_level_pages() {
    let root = tmpdir("navtop");
    let src = root.join("src");
    std::fs::create_dir_all(&src).unwrap();
    write_nested_site(&src);
    let out_dir = root.join("out");

    build_site(&src, &out_dir, &BuildOptions::default()).unwrap();
    let home = std::fs::read_to_string(out_dir.join("index.html")).unwrap();
    let nav = home
        .split("<nav>")
        .nth(1)
        .and_then(|s| s.split("</nav>").next())
        .expect("a nav element");

    assert!(nav.contains("About"), "a root-level page belongs in the nav:\n{nav}");
    assert!(nav.contains("Home"), "the index page belongs in the nav:\n{nav}");
    assert!(
        !nav.contains("First Post") && !nav.contains("Second Post"),
        "pages in subdirectories must not appear in the nav:\n{nav}"
    );

    // Nested pages still get the nav — they just are not *in* it.
    let post = std::fs::read_to_string(out_dir.join("blog/first.html")).unwrap();
    assert!(
        post.contains("href=\"../about.html\"") && post.contains("href=\"../index.html\""),
        "a nested page links up to the top-level nav:\n{post}"
    );
}

/// The payoff for narrowing the site-structure hash to nav entries. Adding a blog post
/// cannot change any other page's nav, so it must not re-render the site — which is what
/// hashing *every* page's (path, title) used to force.
#[test]
fn adding_a_nested_page_does_not_rebuild_the_site() {
    let root = tmpdir("navadd");
    let src = root.join("src");
    std::fs::create_dir_all(&src).unwrap();
    write_nested_site(&src);
    let out_dir = root.join("out");

    build_site(&src, &out_dir, &BuildOptions::default()).unwrap();

    write(&src.join("blog"), "third.org", "#+TITLE: Third Post\n\nBody.\n");
    let r = build_site(&src, &out_dir, &BuildOptions::default()).unwrap();

    assert_eq!(
        r.rendered,
        vec![out("blog/third.html")],
        "only the new nested page renders, got: {:?}",
        r.rendered
    );
    assert_eq!(r.skipped.len(), 4, "every pre-existing page is reused");
}

/// The other half of the same rule: a page that IS in the nav still invalidates
/// everything when its title changes, because every page renders that title.
#[test]
fn retitling_a_top_level_page_still_rebuilds_the_site() {
    let root = tmpdir("navretitle");
    let src = root.join("src");
    std::fs::create_dir_all(&src).unwrap();
    write_nested_site(&src);
    let out_dir = root.join("out");

    build_site(&src, &out_dir, &BuildOptions::default()).unwrap();
    write(&src, "about.org", "#+TITLE: Colophon\n\nAbout me.\n");
    let r = build_site(&src, &out_dir, &BuildOptions::default()).unwrap();

    assert_eq!(r.rendered.len(), 4, "a nav title change re-renders every page");
    let post = std::fs::read_to_string(out_dir.join("blog/first.html")).unwrap();
    assert!(post.contains("Colophon"), "nested pages show the updated nav title");
}

/// Editing one layout must re-render the pages that use it, and only those. Hashing every
/// template into every page means a change to the feed template rewrites the whole site,
/// which is most of the wait in a `serve` session spent on design.
#[test]
fn editing_one_template_rebuilds_only_the_pages_that_use_it() {
    let root = tmpdir("tmplscope");
    let src = root.join("src");
    std::fs::create_dir_all(src.join("blog")).unwrap();
    std::fs::create_dir_all(src.join("templates")).unwrap();
    std::fs::write(src.join("index.org"), "#+TITLE: Home\n\nWelcome.\n").unwrap();
    std::fs::write(src.join("about.org"), "#+TITLE: About\n\nAbout.\n").unwrap();
    std::fs::write(
        src.join("blog/post.org"),
        "#+TITLE: Post\n#+DATE: 2026-01-01\n\nBody.\n",
    )
    .unwrap();
    std::fs::write(
        src.join("templates/base.html"),
        "<html><body>{% block content %}{{ body | safe }}{% endblock %}</body></html>",
    )
    .unwrap();
    std::fs::write(
        src.join("templates/post.html"),
        "{% extends \"base.html\" %}{% block content %}{{ body | safe }}<p>reply</p>{% endblock %}",
    )
    .unwrap();
    std::fs::write(
        src.join("org-ssg.toml"),
        "[[pages]]\nmatch = \"blog\"\ntemplate = \"post.html\"\n",
    )
    .unwrap();
    let out_dir = root.join("out");
    build_site(&src, &out_dir, &BuildOptions::default()).unwrap();

    // post.html is used by one page.
    std::fs::write(
        src.join("templates/post.html"),
        "{% extends \"base.html\" %}{% block content %}{{ body | safe }}<p>reply now</p>{% endblock %}",
    )
    .unwrap();
    let r = build_site(&src, &out_dir, &BuildOptions::default()).unwrap();
    assert_eq!(
        r.rendered,
        vec![Utf8PathBuf::from("blog/post.html")],
        "only the page whose layout changed"
    );
    assert!(std::fs::read_to_string(out_dir.join("blog/post.html"))
        .unwrap()
        .contains("reply now"));

    // base.html is extended by post.html, so editing it reaches both.
    std::fs::write(
        src.join("templates/base.html"),
        "<html><body class=\"new\">{% block content %}{{ body | safe }}{% endblock %}</body></html>",
    )
    .unwrap();
    let r = build_site(&src, &out_dir, &BuildOptions::default()).unwrap();
    assert_eq!(
        r.rendered.len(),
        3,
        "a layout everything inherits still re-renders everything: {:?}",
        r.rendered
    );
}
