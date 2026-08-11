//! Proves the test harness works end to end while the pipeline is still stubbed.
//!
//! Two layers, matching the spec's testing philosophy (§5):
//! - a plain unit test on the one real function (`content_hash`);
//! - an `insta` JSON snapshot of a hand-built element-tree value, standing in for the
//!   element-tree snapshots the parser will produce from Phase 1 onward.

use org_ssg::model::{Link, LinkTarget, Object};
use org_ssg::parser::content_hash;

#[test]
fn content_hash_is_deterministic_blake3() {
    let a = content_hash(b"hello\n");
    let b = content_hash(b"hello\n");
    let c = content_hash(b"goodbye\n");
    assert_eq!(a, b, "same bytes must hash identically");
    assert_ne!(a, c, "different bytes must hash differently");
    // blake3 is 32 bytes.
    assert_eq!(a.0.len(), 32);
}

#[test]
fn element_tree_json_snapshot() {
    // A small inline-object tree built by hand — the shape the parser will emit.
    let tree = vec![
        Object::Text("see ".into()),
        Object::Bold(vec![Object::Text("bold".into())]),
        Object::Link(Link {
            target: LinkTarget::CustomId("first".into()),
            description: Some(vec![Object::Text("the first heading".into())]),
        }),
    ];
    insta::assert_json_snapshot!(tree);
}
