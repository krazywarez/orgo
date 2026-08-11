//! `watch`: the change filter, and one end-to-end run against real filesystem events.
//!
//! The filter carries the weight here. `org-ssg watch . -o _site` puts the output inside
//! the source, so a rebuild writes files, writing files raises events, and events trigger
//! a rebuild — a loop that never stops. That it is a pure function is what makes the
//! guarantee testable without waiting on a filesystem.

use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use camino::{Utf8Path, Utf8PathBuf};

use org_ssg::site::{build_site, BuildOptions};
use org_ssg::watch::ChangeFilter;

fn tmpdir(tag: &str) -> Utf8PathBuf {
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    let base = Utf8PathBuf::from_path_buf(std::env::temp_dir())
        .expect("utf-8 temp dir")
        .join(format!("org-ssg-watch-{}-{tag}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    base
}

// ---------------------------------------------------------------------------
// The change filter
// ---------------------------------------------------------------------------

/// The one that matters: without it, `watch . -o _site` rebuilds forever.
#[test]
fn changes_under_the_output_directory_are_ignored() {
    let root = tmpdir("filterout");
    let src = root.join("src");
    let out = src.join("_site");
    std::fs::create_dir_all(&out).unwrap();

    let filter = ChangeFilter::new(&src, &out);
    assert!(!filter.is_relevant(Utf8Path::new("_site/index.html")));
    assert!(!filter.is_relevant(Utf8Path::new("_site/blog/post.html")));
    assert!(!filter.is_relevant(Utf8Path::new("_site/.org-ssg-cache.json")));
    assert!(filter.is_relevant(Utf8Path::new("index.org")), "real sources still count");
}

/// An output directory outside the source cannot cause a loop, and must not accidentally
/// suppress a similarly-named source directory.
#[test]
fn an_external_output_directory_suppresses_nothing() {
    let root = tmpdir("filterext");
    let src = root.join("src");
    let out = root.join("out");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::create_dir_all(&out).unwrap();

    let filter = ChangeFilter::new(&src, &out);
    assert!(filter.is_relevant(Utf8Path::new("index.org")));
    assert!(filter.is_relevant(Utf8Path::new("out/notes.org")), "a source dir named `out`");
}

/// A change to the config or a template changes the output, so both must rebuild — even
/// though build-time *discovery* skips them as non-content. The watch rule is "would this
/// change the site?", not "is this a page?".
#[test]
fn build_inputs_trigger_a_rebuild_even_though_discovery_skips_them() {
    let root = tmpdir("filterinputs");
    let src = root.join("src");
    std::fs::create_dir_all(&src).unwrap();
    let filter = ChangeFilter::new(&src, &root.join("out"));

    assert!(filter.is_relevant(Utf8Path::new("org-ssg.toml")));
    assert!(filter.is_relevant(Utf8Path::new("templates/base.html")));
    assert!(filter.is_relevant(Utf8Path::new("templates/feed.xml")));
    assert!(filter.is_relevant(Utf8Path::new("style.css")), "assets are copied through");
}

/// `.git` churns on every command, and rebuilding the site because git wrote an index
/// lock would make watch useless in any repository.
#[test]
fn dot_directories_and_editor_scratch_files_are_ignored() {
    let root = tmpdir("filterdots");
    let src = root.join("src");
    std::fs::create_dir_all(&src).unwrap();
    let filter = ChangeFilter::new(&src, &root.join("out"));

    for ignored in [
        ".git/index",
        ".git/objects/ab/cdef",
        ".DS_Store",
        "blog/.#post.org",  // Emacs lock
        "post.org~",        // Emacs backup
        ".post.org.swp",    // vim
        "#post.org#",       // Emacs auto-save
        "build.tmp",
    ] {
        assert!(
            !filter.is_relevant(Utf8Path::new(ignored)),
            "{ignored} should not trigger a rebuild"
        );
    }
    for relevant in ["post.org", "blog/post.org", "a-file~with-tilde.org"] {
        assert!(
            filter.is_relevant(Utf8Path::new(relevant)),
            "{relevant} should trigger a rebuild"
        );
    }
}

/// Events arrive as absolute paths and in bursts, often naming one file several times.
#[test]
fn absolute_event_paths_are_reduced_to_a_sorted_unique_set() {
    let root = tmpdir("filterrel");
    let src = root.join("src");
    let out = src.join("_site");
    std::fs::create_dir_all(&out).unwrap();

    let filter = ChangeFilter::new(&src, &out);
    let events = vec![
        src.join("b.org"),
        src.join("a.org"),
        src.join("b.org"),
        src.join("_site/a.html"),
        src.join("a.org~"),
    ];
    assert_eq!(
        filter.relevant(events),
        vec![Utf8PathBuf::from("a.org"), Utf8PathBuf::from("b.org")]
    );
}

// ---------------------------------------------------------------------------
// End to end
// ---------------------------------------------------------------------------

/// Drive the real watcher against a real edit. Timing-dependent by nature, so it polls
/// for the expected result with a generous ceiling rather than sleeping a fixed amount.
#[test]
fn watching_rebuilds_the_site_when_a_source_file_changes() {
    let root = tmpdir("watchrun");
    let src = root.join("src");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(src.join("index.org"), "#+TITLE: Home\n\nFirst version.\n").unwrap();
    let out = src.join("_site"); // deliberately inside the source: the loop case

    build_site(&src, &out, &BuildOptions::default()).unwrap();
    assert!(std::fs::read_to_string(out.join("index.html"))
        .unwrap()
        .contains("First version."));

    let (src_t, out_t) = (src.clone(), out.clone());
    let handle = std::thread::spawn(move || {
        let _ = org_ssg::watch::run(&src_t, &out_t, &BuildOptions::default());
    });

    // Give the watcher a moment to register before making the change it should see.
    std::thread::sleep(Duration::from_millis(300));
    std::fs::write(src.join("index.org"), "#+TITLE: Home\n\nSecond version.\n").unwrap();

    let deadline = Instant::now() + Duration::from_secs(20);
    let mut rebuilt = false;
    while Instant::now() < deadline {
        if std::fs::read_to_string(out.join("index.html"))
            .map(|h| h.contains("Second version."))
            .unwrap_or(false)
        {
            rebuilt = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(rebuilt, "an edit should trigger a rebuild within 20s");

    // The output lives inside the source, so the rebuild's own writes raised events. If
    // those are not filtered out, watch spins forever.
    //
    // The file to watch for that is `syntax.css`, not `index.html`. The incremental
    // build leaves an unchanged page alone, so `index.html` holds still even *during* a
    // runaway loop — an assertion on it passes whether or not the filter works, which is
    // exactly what it did before this comment existed. `syntax.css` is rewritten on
    // every build, so its mtime is a direct record of how many builds have run.
    let stylesheet = out.join("syntax.css");
    std::thread::sleep(Duration::from_millis(700));
    let first = std::fs::metadata(&stylesheet).unwrap().modified().unwrap();
    std::thread::sleep(Duration::from_millis(1200));
    let second = std::fs::metadata(&stylesheet).unwrap().modified().unwrap();
    assert_eq!(
        first, second,
        "the build's own writes must not feed back in as changes — watch is rebuilding \
         in a loop"
    );

    drop(handle); // the watcher thread ends with the process
}
