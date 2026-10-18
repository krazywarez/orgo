//! `watch`: the change filter, and one end-to-end run against real filesystem events.
//!
//! The filter carries the weight here. `orgo watch . -o _site` puts the output inside
//! the source, so a rebuild writes files, writing files raises events, and events trigger
//! a rebuild — a loop that never stops. That it is a pure function is what makes the
//! guarantee testable without waiting on a filesystem.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use camino::{Utf8Path, Utf8PathBuf};

use orgo::site::{build_site, BuildOptions};
use orgo::watch::ChangeFilter;

fn tmpdir(tag: &str) -> Utf8PathBuf {
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    let base = Utf8PathBuf::from_path_buf(std::env::temp_dir())
        .expect("utf-8 temp dir")
        .join(format!("orgo-watch-{}-{tag}-{n}", std::process::id()));
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
    assert!(!filter.is_relevant(Utf8Path::new("_site/.orgo-cache.json")));
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

    assert!(filter.is_relevant(Utf8Path::new("orgo.toml")));
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
// Event kinds
// ---------------------------------------------------------------------------

/// A read is not a change. inotify reports opens, and a build reads every source file, so
/// an access forwarded as a change makes each rebuild trigger the next.
#[test]
fn access_events_do_not_count_as_changes() {
    use notify::event::{AccessKind, AccessMode, CreateKind, DataChange, ModifyKind, RemoveKind};
    use notify::EventKind;
    use orgo::watch::is_change;

    for read in [
        AccessKind::Any,
        AccessKind::Read,
        AccessKind::Open(AccessMode::Any),
        AccessKind::Open(AccessMode::Read),
        AccessKind::Close(AccessMode::Read),
    ] {
        assert!(
            !is_change(&EventKind::Access(read)),
            "{read:?} is a read, not a change"
        );
    }

    assert!(
        is_change(&EventKind::Access(AccessKind::Close(AccessMode::Write))),
        "closing a file that was written to is a change"
    );
    assert!(is_change(&EventKind::Create(CreateKind::File)));
    assert!(is_change(&EventKind::Modify(ModifyKind::Data(DataChange::Any))));
    assert!(is_change(&EventKind::Remove(RemoveKind::File)));
    assert!(
        is_change(&EventKind::Any),
        "FSEvents and the Windows backend report unclassified changes this way"
    );
}

// ---------------------------------------------------------------------------
// End to end
// ---------------------------------------------------------------------------

/// Drive the real watcher against a real edit, then hold still. Timing-dependent by
/// nature, so it polls for the expected result with a generous ceiling rather than
/// sleeping a fixed amount.
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

    // Count rebuilds rather than reading an output file's mtime: a quiet watcher is
    // "no more rebuilds", and that is what the counter says directly.
    let rebuilds = Arc::new(AtomicU32::new(0));
    let (src_t, out_t, counter) = (src.clone(), out.clone(), Arc::clone(&rebuilds));
    let handle = std::thread::spawn(move || {
        let _ = orgo::watch::run_with(&src_t, &out_t, &BuildOptions::default(), |_| {
            counter.fetch_add(1, Ordering::Relaxed);
        });
    });

    // The edit is repeated rather than made once after a fixed head start. The watcher
    // registers its OS watches *after* an initial build, and that build loads syntect's
    // syntax set — on a cold CI runner, comfortably longer than any head start worth
    // hard-coding. An edit made before anything is listening produces no event at all,
    // which looks exactly like a watcher that does not work, and only one of those is a
    // defect worth failing a build over. (Found by the first CI run on Linux, where a
    // 300ms head start was not enough and macOS had never noticed.)
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut wrote_at: Option<Instant> = None;
    while rebuilds.load(Ordering::Relaxed) == 0 && Instant::now() < deadline {
        if wrote_at.is_none_or(|t| t.elapsed() >= Duration::from_millis(400)) {
            std::fs::write(src.join("index.org"), "#+TITLE: Home\n\nSecond version.\n").unwrap();
            wrote_at = Some(Instant::now());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(
        rebuilds.load(Ordering::Relaxed) > 0,
        "an edit should trigger a rebuild within 30s"
    );
    assert!(std::fs::read_to_string(out.join("index.html"))
        .unwrap()
        .contains("Second version."));

    // Nothing is editing the tree now, so nothing more should be built. Two of the
    // rebuild's own side effects would otherwise feed back in as changes:
    //
    // 1. Its writes. The output lives inside the source, so they raise events —
    //    [`ChangeFilter`] drops them.
    // 2. Its reads. A build reads every source file, and inotify reports opens, so a
    //    watcher that treats an access as a change rebuilds at full speed. Reported
    //    against `orgo serve`, where every rebuild reloads the browser.
    //
    // The first sleep lets the rebuild from the last repeated write land, or this
    // measures that instead of a feedback loop.
    std::thread::sleep(Duration::from_millis(1500));
    let settled = rebuilds.load(Ordering::Relaxed);
    std::thread::sleep(Duration::from_millis(1500));
    assert_eq!(
        settled,
        rebuilds.load(Ordering::Relaxed),
        "a quiet tree must produce no rebuilds — watch is rebuilding in a loop"
    );

    drop(handle); // the watcher thread ends with the process
}
