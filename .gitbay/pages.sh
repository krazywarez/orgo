#!/bin/sh
# Publish the documentation site to the `pages` branch, which gitbay serves at
# https://orgo.krz.sh. Queued on every branch push; does nothing off main.
#
# The site is fifteen org files built by the tool that documents them, so this is
# also the most honest smoke test in the repository: if orgo cannot build its own
# documentation, the job fails and says so. --strict makes a broken internal link
# a failure, because docs rot by having their links quietly stop resolving.
set -eu

if [ "${GITBAY_REF:-}" != "main" ]; then
	echo "ref is ${GITBAY_REF:-unset}, not main — nothing to publish"
	exit 0
fi

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
site="$tmp/site"
work="$tmp/work"

cargo run --release --locked -- build docs -o "$site" --strict
rm -f "$site/.orgo-cache.json"

# An orphan history: the pages branch carries the built site and nothing else, so
# each publish replaces it wholesale rather than accumulating every past build.
git init -q "$work"
cp -R "$site/." "$work/"
printf '.orgo-cache.json\n' >"$work/.gitignore"
git -C "$work" add -A
git -C "$work" -c user.name=gitbay-ci -c user.email=ci@orgo.krz.sh commit -q \
	-m "Publish the documentation site

Built from ${GITBAY_SHA} by \`orgo build docs -o _site --strict\`."
git -C "$work" push -q --force "ssh://git@gitbay.org/${GITBAY_REPO}.git" HEAD:refs/heads/pages

echo "published the site from ${GITBAY_SHA} to the pages branch"
