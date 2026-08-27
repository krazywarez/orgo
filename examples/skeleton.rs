//! Render an org file the way orgo would and print either its HTML or its semantic
//! skeleton — the reference outputs the cross-language conformance corpus is built from.
//!
//! Usage:
//!   cargo run --example skeleton -- <file.org>            # print the skeleton
//!   cargo run --example skeleton -- --html <file.org>     # print the rendered HTML
//!
//! The skeleton is one line per element open, element close, or text run (see
//! [`orgo::skeleton`]). Another implementation conforms when its own output, reduced by
//! its own port of the same reduction, equals this.

use camino::Utf8PathBuf;

use orgo::parser::parse;
use orgo::render::{render, Html, SyntectHighlighter};
use orgo::resolve::ResolvedDoc;
use orgo::skeleton::skeleton;

fn main() {
    let mut args = std::env::args().skip(1);
    let mut html_mode = false;
    let mut path: Option<String> = None;
    for arg in args.by_ref() {
        match arg.as_str() {
            "--html" => html_mode = true,
            _ => path = Some(arg),
        }
    }
    let Some(path) = path else {
        eprintln!("usage: skeleton [--html] <file.org>");
        std::process::exit(2);
    };

    let path = Utf8PathBuf::from(path);
    let source = std::fs::read_to_string(&path).expect("read org file");
    let document = parse(path.as_path(), &source).expect("parse org file");
    let Html(html) = render(&ResolvedDoc { document }, &SyntectHighlighter::new());

    if html_mode {
        print!("{html}");
    } else {
        for line in skeleton(&html) {
            println!("{line}");
        }
    }
}
