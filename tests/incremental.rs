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
            strict: false,
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
