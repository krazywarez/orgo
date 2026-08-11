# Releasing

A release is three things that must agree: a version in `Cargo.toml`, a git tag, and a
changelog entry. The release workflow checks the first two against each other and refuses
to build if they differ, because a release tagged `v0.18.0` containing a binary that
reports `0.17.0` is the kind of mistake nobody notices for months.

## Before the first publish

`repository` in `Cargo.toml` is commented out, because a wrong URL on a crates.io page is
worse than none. Set it, then:

```bash
cargo login          # a crates.io token, once per machine
cargo publish --dry-run
```

## Every release

1. **Write the changelog entry first.** [CHANGELOG.md](CHANGELOG.md) names behaviour, not
   commits — someone reading it wants to know what their next build will do differently.
   Anything that changes rendered HTML gets said out loud.

2. **Bump the version** in `Cargo.toml`, and build once so `Cargo.lock` follows.

   Patch for fixes that change nothing about the stable surface. Minor for new config
   keys, new template variables, an MSRV bump, or output that changes to track Emacs more
   closely. Major for anything that breaks the promises in the README's Compatibility
   section — config keys, template context, CLI, or URLs.

3. **Check it.**

   ```bash
   cargo test
   cargo clippy --all-targets -- -D warnings
   cargo run -- build docs -o docs/_site --strict
   cargo package
   ```

   `cargo package` is the one people forget: it builds the crate exactly as crates.io will
   receive it, and catches a file the `exclude` list should not have removed.

4. **Verify against a real corpus.** The test suite says the code does what it did; a
   corpus says the *site* does. Build a site you know with `--no-cache` and diff the
   output against the previous version's. A release that quietly changes 200 pages should
   do so on purpose.

5. **Commit, tag, push.**

   ```bash
   git commit -am "0.18: <what changed>"
   git tag -a v0.18.0 -m "0.18.0"
   git push && git push --tags
   ```

6. **Publish the crate.**

   ```bash
   cargo publish
   ```

   This is irreversible: a published version can be yanked but never replaced.

7. **Finish the GitHub release.** Pushing the tag builds binaries for macOS (arm64 and
   x86_64) and Linux (gnu and musl) and opens a *draft* release with them attached. Paste
   the changelog entry in and publish it. The draft is deliberate — a release that
   publishes itself before anyone has read it cannot be edited quietly.

## If a release goes wrong

Yank rather than delete, and ship a fix as a new version:

```bash
cargo yank --version 0.18.0
```

Yanking stops new dependents from selecting it; anyone who already has it keeps working.
Then release `0.18.1` with the fix and a changelog entry that says what happened.
