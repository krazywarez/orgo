//! `serve`: a development server over the built site, with browser live reload.
//!
//! `watch` rebuilds but leaves you to serve the output and press reload yourself. This
//! closes that loop: build, watch, serve, and push a reload to the browser when a
//! rebuild lands.
//!
//! Three decisions shape it:
//!
//! 1. **Loopback by default.** A development server binds `127.0.0.1`, not `0.0.0.0`.
//!    It serves unreviewed drafts off someone's laptop, and exposing that to the local
//!    network should be a thing you ask for (`--host`), never a thing you get.
//! 2. **The reload script is injected at serve time**, never written to disk. The built
//!    site is what you deploy, and it must not carry a dev server's JavaScript.
//! 3. **Long-polling, not WebSockets or SSE.** The browser asks "has anything changed
//!    since generation N?" and the server holds the request open until something has.
//!    That is instant like a push, needs no protocol beyond ordinary HTTP, and — unlike
//!    a streamed response — completes, which is what makes it work at all: tiny_http
//!    buffers a response until its body ends, so a body that never ends never reaches
//!    the client. Long-polling was the version of this that worked.

use std::io;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use anyhow::Result;
use camino::{Utf8Path, Utf8PathBuf};
use tiny_http::{Header, Request, Response, Server, StatusCode};

use crate::site::{build_site, BuildOptions};

/// Where the browser subscribes for reload events. Namespaced so it cannot collide with
/// a real page.
pub const RELOAD_PATH: &str = "/__orgo/reload";

/// How long a poll waits before answering "nothing yet". Long enough that an idle tab is
/// nearly silent, short enough to stay under any proxy or browser idle timeout.
const POLL_TIMEOUT: Duration = Duration::from_secs(25);

/// The script injected into served HTML, carrying the generation the page was built
/// from.
///
/// Baking the generation in is what makes this race-free: if a rebuild lands between the
/// page being served and the first poll going out, the server answers immediately rather
/// than the tab sitting on stale content until the *next* edit.
fn reload_script(generation: u64) -> String {
    format!(
        "\n<script>(function p(n){{fetch(\"{RELOAD_PATH}?since=\"+n)\
         .then(function(r){{return r.json()}})\
         .then(function(g){{g>n?location.reload():p(g)}})\
         .catch(function(){{setTimeout(function(){{p(n)}},1000)}})}})({generation})</script>\n"
    )
}

/// A build counter that event streams wait on.
#[derive(Default)]
struct BuildSignal {
    generation: Mutex<u64>,
    changed: Condvar,
}

impl BuildSignal {
    fn bump(&self) {
        *self.generation.lock().expect("build signal") += 1;
        self.changed.notify_all();
    }
}

/// Run the development server until interrupted.
pub fn run(
    src: &Utf8Path,
    out: &Utf8Path,
    opts: &BuildOptions,
    host: &str,
    port: u16,
) -> Result<()> {
    if !src.is_dir() {
        anyhow::bail!("serve requires a source directory: serve <src-dir> -o <out-dir>");
    }
    let report = build_site(src, out, opts)?;

    let address = format!("{host}:{port}");
    let server = Server::http(&address).map_err(|e| {
        anyhow::anyhow!("cannot listen on {address}: {e}. Is something already using port {port}?")
    })?;
    let server = Arc::new(server);
    let signal = Arc::new(BuildSignal::default());
    let root: Utf8PathBuf = out.to_owned();

    println!(
        "serving {} page(s) from {out} at http://{address}/ — Ctrl-C to stop.",
        report.pages.len()
    );

    // Rebuild in the background; the main thread serves.
    {
        let (src, out, opts, signal) = (
            src.to_owned(),
            out.to_owned(),
            opts.clone(),
            Arc::clone(&signal),
        );
        std::thread::spawn(move || {
            let result = crate::watch::run_with(&src, &out, &opts, |built| {
                // Reload on a *successful* rebuild only. Reloading onto a stale page
                // because the build just failed tells the author nothing; the error is
                // already on their terminal.
                if built.is_ok() {
                    signal.bump();
                }
            });
            if let Err(e) = result {
                eprintln!("watch stopped: {e:#}");
            }
        });
    }

    // A thread per request. The volume is one developer's browser, and an event stream
    // occupies its thread for as long as the tab is open — which a fixed pool would let
    // starve everything else.
    for request in server.incoming_requests() {
        let root = root.clone();
        let signal = Arc::clone(&signal);
        std::thread::spawn(move || {
            if let Err(e) = handle(request, &root, &signal) {
                // A browser closing a tab mid-response is routine, not a problem.
                if e.kind() != io::ErrorKind::BrokenPipe {
                    eprintln!("serve: {e}");
                }
            }
        });
    }
    Ok(())
}

fn handle(request: Request, root: &Utf8Path, signal: &Arc<BuildSignal>) -> io::Result<()> {
    let url = request.url().to_string();
    if url.split(['?', '#']).next() == Some(RELOAD_PATH) {
        let since = since_parameter(&url);
        return serve_poll(request, signal, since);
    }
    match resolve(root, &url) {
        Some(path) => serve_file(request, &path, signal),
        None => request.respond(
            Response::from_string("404 not found")
                .with_status_code(StatusCode(404))
                .with_header(header("Content-Type", "text/plain; charset=utf-8")),
        ),
    }
}

fn serve_file(request: Request, path: &Utf8Path, signal: &Arc<BuildSignal>) -> io::Result<()> {
    let Ok(bytes) = std::fs::read(path) else {
        return request.respond(
            Response::from_string("404 not found").with_status_code(StatusCode(404)),
        );
    };
    let mime = mime_type(path);
    let bytes = if mime.starts_with("text/html") {
        let generation = *signal.generation.lock().expect("build signal");
        inject_reload_script(&bytes, generation)
    } else {
        bytes
    };
    request.respond(
        Response::from_data(bytes)
            .with_header(header("Content-Type", mime))
            // A dev server must never be cached, or an edit appears not to have landed.
            .with_header(header("Cache-Control", "no-store")),
    )
}

/// Put the reload script just before `</body>`, or at the end if there is none.
pub fn inject_reload_script(bytes: &[u8], generation: u64) -> Vec<u8> {
    let Ok(text) = std::str::from_utf8(bytes) else {
        return bytes.to_vec();
    };
    let script = reload_script(generation);
    match text.rfind("</body>") {
        Some(at) => format!("{}{script}{}", &text[..at], &text[at..]).into_bytes(),
        None => format!("{text}{script}").into_bytes(),
    }
}

/// Answer a poll: block until the build generation passes `since`, then report it.
///
/// A timeout answers with the *current* generation, which the client compares itself —
/// so a slow answer is indistinguishable from a fast one and no event can be missed.
fn serve_poll(request: Request, signal: &Arc<BuildSignal>, since: u64) -> io::Result<()> {
    let guard = signal.generation.lock().expect("build signal");
    let (guard, _) = signal
        .changed
        .wait_timeout_while(guard, POLL_TIMEOUT, |generation| *generation <= since)
        .expect("build signal");
    let generation = *guard;
    drop(guard);

    request.respond(
        Response::from_string(generation.to_string())
            .with_header(header("Content-Type", "application/json"))
            .with_header(header("Cache-Control", "no-store")),
    )
}

/// The `since=N` parameter of a poll request.
pub fn since_parameter(url: &str) -> u64 {
    url.split_once('?')
        .map(|(_, query)| query)
        .into_iter()
        .flat_map(|query| query.split('&'))
        .find_map(|pair| pair.strip_prefix("since="))
        .and_then(|value| value.parse().ok())
        .unwrap_or(0)
}

/// Map a request URL onto a file inside `root`, or `None` if it does not name one.
///
/// This is the server's security boundary, so it is a pure function with its own tests.
/// A URL is attacker-controlled input even on a development server: `..` segments,
/// percent-encoded `..`, absolute paths and backslashes all have to resolve to nothing
/// rather than to somewhere outside the output directory.
pub fn resolve(root: &Utf8Path, url: &str) -> Option<Utf8PathBuf> {
    let path = url.split(['?', '#']).next().unwrap_or("");
    let decoded = percent_decode(path);

    // Build the path from scratch out of accepted segments. Normalizing a joined path
    // afterwards is the version of this that has bugs: it is far easier to reason about
    // a list that never contained a `..` than about removing one correctly.
    let mut segments: Vec<&str> = Vec::new();
    for segment in decoded.split(['/', '\\']) {
        match segment {
            "" | "." => {}
            ".." => return None,
            // A NUL or a path separator that survived decoding is not a filename.
            s if s.contains('\0') => return None,
            s => segments.push(s),
        }
    }

    let mut candidate = root.to_owned();
    for segment in &segments {
        candidate.push(segment);
    }
    // Directories, and the bare root, serve their index.
    if decoded.ends_with('/') || segments.is_empty() || candidate.is_dir() {
        candidate.push("index.html");
    }
    // Defence in depth: whatever the segment logic did, the result must be inside root.
    if !candidate.starts_with(root) {
        return None;
    }
    candidate.is_file().then_some(candidate)
}

/// Decode `%XX` escapes. `+` is left alone: it means a space in a query string, not in a
/// path, and turning `a+b.html` into `a b.html` would break a real filename.
fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok();
            if let Some(byte) = hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
                out.push(byte);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn mime_type(path: &Utf8Path) -> &'static str {
    match path.extension().unwrap_or("").to_ascii_lowercase().as_str() {
        "html" | "htm" => "text/html; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "js" => "text/javascript; charset=utf-8",
        "json" => "application/json",
        "xml" | "rss" | "atom" => "application/xml; charset=utf-8",
        "txt" => "text/plain; charset=utf-8",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "avif" => "image/avif",
        "ico" => "image/x-icon",
        "woff2" => "font/woff2",
        "woff" => "font/woff",
        "ttf" => "font/ttf",
        "pdf" => "application/pdf",
        _ => "application/octet-stream",
    }
}

fn header(name: &str, value: &str) -> Header {
    Header::from_bytes(name.as_bytes(), value.as_bytes()).expect("static header is valid")
}
