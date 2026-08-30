#!/bin/sh
# The tag push is the release. Builds the Linux binaries, creates the gitbay
# release with the tag's own annotation as its notes, attaches the tarballs, and
# publishes to crates.io.
#
# The two darwin tarballs are not built here: this runner is Linux, and gitbay's
# `runner next` claims the oldest pending build with no platform targeting, so a
# Mac runner could not be aimed at them. .githooks/pre-push builds them on a Mac
# before the tag is pushed and uploads them to this release once it exists.
#
# crates.io is the one step that cannot be undone — a version can be yanked but
# never replaced — so it runs last, after the release exists and the binaries are
# attached. CARGO_REGISTRY_TOKEN is a repository secret (`gitbay repo secret set`).
set -eu

tag="${GITBAY_REF:?no tag in GITBAY_REF}"
repo="${GITBAY_REPO:?no repository in GITBAY_REPO}"
: "${CARGO_REGISTRY_TOKEN:?CARGO_REGISTRY_TOKEN secret is not set}"

# The tag is the source of truth for the version, checked rather than trusted: a
# release tagged v0.18.0 whose binary reports 0.17.0 is the kind of thing nobody
# notices for months.
version=$(cargo metadata --no-deps --format-version 1 |
	python3 -c 'import json,sys; print(json.load(sys.stdin)["packages"][0]["version"])')
if [ "$tag" != "v$version" ]; then
	echo "tag $tag does not match Cargo.toml version $version" >&2
	exit 1
fi

dist=dist
mkdir -p "$dist"

# glibc for ordinary distributions, musl for containers and anything older than
# the runner's glibc — a dynamically linked binary is the usual reason a download
# does not run.
for target in x86_64-unknown-linux-gnu x86_64-unknown-linux-musl; do
	cargo build --release --locked --target "$target"
	# A tarball rather than a bare binary: it keeps the executable bit through the
	# download path, and carries the licence with the thing it licenses.
	staging="orgo-$tag-$target"
	rm -rf "$staging"
	mkdir "$staging"
	cp "target/$target/release/orgo" README.md LICENSE "$staging/"
	tar czf "$dist/$staging.tar.gz" "$staging"
	rm -rf "$staging"
	(cd "$dist" && sha256sum "$staging.tar.gz" >"$staging.tar.gz.sha256")
done

# Notes come from the annotated tag, so the person cutting the release writes
# them at the moment they decide to cut it (`git tag -a "$tag" -F notes.md`).
git tag -l --format='%(contents)' "$tag" |
	ssh git@gitbay.org release create "$repo" "$tag" --title "${tag#v}" --file -

for f in "$dist"/*; do
	ssh git@gitbay.org release asset add "$repo" "$tag" "$(basename "$f")" <"$f"
	echo "attached $(basename "$f")"
done

# --locked publishes exactly the dependency versions the tests ran against,
# rather than whatever resolves at publish time.
cargo publish --locked
echo "published $tag to crates.io"
