//! `serve`: URL resolution, reload-script injection, and one live server.
//!
//! `resolve` is the server's security boundary. A URL is attacker-controlled input even
//! on a development server — a page someone is previewing can contain a link, an image
//! or a fetch to anything — so it gets tested as the pure function it deliberately is,
//! rather than only through a running server.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use camino::{Utf8Path, Utf8PathBuf};

use orgo::serve::{inject_reload_script, resolve, since_parameter};
use orgo::site::BuildOptions;

fn tmpdir(tag: &str) -> Utf8PathBuf {
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    let base = Utf8PathBuf::from_path_buf(std::env::temp_dir())
        .expect("utf-8 temp dir")
        .join(format!("orgo-serve-{}-{tag}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    base
}

/// An output tree to serve.
fn write_output(out: &Utf8PathBuf) {
    std::fs::create_dir_all(out.join("blog")).unwrap();
    std::fs::write(out.join("index.html"), "<html><body>home</body></html>").unwrap();
    std::fs::write(out.join("blog/index.html"), "<html><body>blog</body></html>").unwrap();
    std::fs::write(out.join("syntax.css"), "body{}").unwrap();
    std::fs::write(out.join("a file.html"), "<html><body>spaced</body></html>").unwrap();
}

// ---------------------------------------------------------------------------
// URL resolution
// ---------------------------------------------------------------------------

#[test]
fn urls_resolve_to_files_and_directory_indexes() {
    let out = tmpdir("resolve");
    write_output(&out);

    let at = |url: &str| resolve(&out, url).map(|p| p.strip_prefix(&out).unwrap().to_string());
    assert_eq!(at("/"), Some("index.html".into()), "the root serves its index");
    assert_eq!(at("/index.html"), Some("index.html".into()));
    assert_eq!(at("/blog/"), Some("blog/index.html".into()), "a directory serves its index");
    assert_eq!(at("/blog"), Some("blog/index.html".into()), "even without the slash");
    assert_eq!(at("/syntax.css"), Some("syntax.css".into()));
    assert_eq!(at("/index.html?v=1#frag"), Some("index.html".into()), "query and fragment");
    assert_eq!(at("/a%20file.html"), Some("a file.html".into()), "percent-decoded");
    assert_eq!(at("/nope.html"), None, "a file that does not exist");
}

/// The one that matters. A dev server sits on a laptop with a home directory behind it.
#[test]
fn no_url_can_escape_the_output_directory() {
    let root = tmpdir("traversal");
    let out = root.join("out");
    write_output(&out);
    // A file next to the output that must stay unreachable.
    std::fs::write(root.join("secret.txt"), "private").unwrap();

    for attack in [
        "/../secret.txt",
        "/../../etc/passwd",
        "/blog/../../secret.txt",
        "/%2e%2e/secret.txt",
        "/%2E%2E/secret.txt",
        "/..%2fsecret.txt",
        "/....//secret.txt",
        "/\\../secret.txt",
        "//../secret.txt",
        "/./../secret.txt",
        "/blog/%2e%2e/%2e%2e/secret.txt",
    ] {
        assert_eq!(resolve(&out, attack), None, "{attack} must not resolve");
    }
    // And the file really was reachable by its true path, so the test is not vacuous.
    assert!(root.join("secret.txt").is_file());
}

/// A percent-encoded NUL is a classic way to truncate a path in a C-backed API.
#[test]
fn embedded_nul_bytes_are_rejected() {
    let out = tmpdir("nul");
    write_output(&out);
    assert_eq!(resolve(&out, "/index.html%00.txt"), None);
    assert_eq!(resolve(&out, "/%00"), None);
}

/// `+` means a space in a query string, not in a path — decoding it would break the
/// perfectly ordinary filename `c++.html`.
#[test]
fn plus_is_not_decoded_as_a_space() {
    let out = tmpdir("plus");
    write_output(&out);
    std::fs::write(out.join("c++.html"), "<html><body>cpp</body></html>").unwrap();
    assert_eq!(
        resolve(&out, "/c++.html").map(|p| p.strip_prefix(&out).unwrap().to_string()),
        Some("c++.html".into())
    );
}

#[test]
fn the_poll_parameter_is_read_from_the_query() {
    assert_eq!(since_parameter("/__orgo/reload?since=7"), 7);
    assert_eq!(since_parameter("/__orgo/reload?x=1&since=42"), 42);
    assert_eq!(since_parameter("/__orgo/reload"), 0, "absent means start from zero");
    assert_eq!(since_parameter("/__orgo/reload?since=nope"), 0, "unparseable means zero");
}

// ---------------------------------------------------------------------------
// Reload script injection
// ---------------------------------------------------------------------------

/// The built site is what gets deployed. A dev server's JavaScript must never be in it,
/// which is why injection happens on the way out rather than at build time.
#[test]
fn the_reload_script_is_injected_before_the_closing_body_tag() {
    let page = b"<html><body><p>hi</p></body></html>";
    let served = String::from_utf8(inject_reload_script(page, 3)).unwrap();

    assert!(served.contains("<p>hi</p>"), "content is preserved");
    assert!(served.contains("})(3)"), "the generation is baked in: {served}");
    let script_at = served.find("<script>").unwrap();
    let body_at = served.rfind("</body>").unwrap();
    assert!(script_at < body_at, "the script goes inside the body: {served}");
}

/// A fragment with no `</body>` — a partial, or a hand-written page — still gets it.
#[test]
fn injection_falls_back_to_appending() {
    let served = String::from_utf8(inject_reload_script(b"<p>bare</p>", 1)).unwrap();
    assert!(served.starts_with("<p>bare</p>"));
    assert!(served.contains("<script>"));
}

/// Binary content that happens to be served as HTML must not be corrupted into garbage.
#[test]
fn non_utf8_content_is_passed_through_untouched() {
    let bytes = vec![0xff, 0xfe, 0x00, 0x42];
    assert_eq!(inject_reload_script(&bytes, 1), bytes);
}

// ---------------------------------------------------------------------------
// A live server
// ---------------------------------------------------------------------------

/// Minimal HTTP client: send a request, return the whole response.
fn get(port: u16, path: &str, timeout: Duration) -> Option<String> {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).ok()?;
    stream.set_read_timeout(Some(timeout)).ok()?;
    write!(stream, "GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n").ok()?;
    let mut response = Vec::new();
    stream.read_to_end(&mut response).ok()?;
    Some(String::from_utf8_lossy(&response).into_owned())
}

fn start_server(src: &Utf8Path, out: &Utf8Path) -> u16 {
    // Unique per test as well as per process: these tests run in parallel, and two of
    // them sharing a port means one silently queries the other's site.
    static NEXT: AtomicU32 = AtomicU32::new(0);
    let port = 20000 + ((std::process::id() % 10000) as u16) + NEXT.fetch_add(1, Ordering::Relaxed) as u16;
    let (s, o) = (src.to_owned(), out.to_owned());
    std::thread::spawn(move || {
        let _ = orgo::serve::run(&s, &o, &BuildOptions::default(), "127.0.0.1", port);
    });
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline {
        if get(port, "/", Duration::from_millis(500)).is_some() {
            return port;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    panic!("server did not start on port {port}");
}

/// Serve a real site, and confirm an edit both rebuilds and answers a waiting poll —
/// which together are the whole point of the command.
#[test]
fn serving_a_site_reloads_the_browser_when_a_source_changes() {
    let root = tmpdir("live");
    let src = root.join("src");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(src.join("index.org"), "#+TITLE: Home\n\nFirst version.\n").unwrap();
    // Output outside the source, so the test's own writes cannot be mistaken for edits.
    let out = root.join("out");
    let port = start_server(&src, &out);

    let home = get(port, "/", Duration::from_secs(5)).expect("a response");
    assert!(home.contains("200 OK"), "{home}");
    assert!(home.contains("First version."), "the page is served: {home}");
    assert!(home.contains("__orgo/reload"), "with the reload script: {home}");
    assert!(
        !std::fs::read_to_string(out.join("index.html")).unwrap().contains("__orgo"),
        "but the file on disk stays clean"
    );

    // A poll for a generation we already have must block, not answer immediately.
    let poller = std::thread::spawn(move || {
        let started = Instant::now();
        let body = get(port, "/__orgo/reload?since=0", Duration::from_secs(30));
        (started.elapsed(), body)
    });
    std::thread::sleep(Duration::from_millis(400));
    std::fs::write(src.join("index.org"), "#+TITLE: Home\n\nSecond version.\n").unwrap();

    let (waited, body) = poller.join().expect("poller");
    let body = body.expect("poll response");
    assert!(
        waited >= Duration::from_millis(300),
        "the poll should have waited for the edit, not returned at once ({waited:?})"
    );
    assert!(body.trim_end().ends_with('1'), "it reports the new generation: {body}");

    let updated = get(port, "/", Duration::from_secs(5)).expect("a response");
    assert!(updated.contains("Second version."), "and the rebuild is served: {updated}");
}

/// A traversal attempt against the running server, not just the resolver.
#[test]
fn the_running_server_refuses_to_escape_its_root() {
    let root = tmpdir("livetraversal");
    let src = root.join("src");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(src.join("index.org"), "#+TITLE: Home\n\nBody.\n").unwrap();
    std::fs::write(root.join("secret.txt"), "private").unwrap();
    let out = root.join("out");
    let port = start_server(&src, &out);

    for attack in ["/../secret.txt", "/%2e%2e/secret.txt", "/../../etc/passwd"] {
        let response = get(port, attack, Duration::from_secs(5)).expect("a response");
        assert!(response.contains("404"), "{attack} should 404: {response}");
        assert!(!response.contains("private"), "{attack} leaked the file: {response}");
    }
}
