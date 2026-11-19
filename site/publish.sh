#!/bin/sh
# Publish the website: build it, then make the built files the only commit
# of the branch `gh-pages` on the remote `origin`. GitHub Pages serves that
# branch (Settings > Pages > Source: "Deploy from a branch", gh-pages, /).
#
# The branch never holds anything but the built site, and each publication
# replaces the one before. The working tree and the current branch are not
# touched.
#
# The commit is made with your git identity. To use another one, set
# GIT_AUTHOR_NAME, GIT_AUTHOR_EMAIL, GIT_COMMITTER_NAME and
# GIT_COMMITTER_EMAIL.
set -eu

cd "$(git rev-parse --show-toplevel)"
cargo xtask site

index=$(mktemp)
trap 'rm -f "$index"' EXIT
rm -f "$index"
tree=$(
    GIT_INDEX_FILE=$index
    export GIT_INDEX_FILE
    git --work-tree=target/site add --all .
    git write-tree
)
commit=$(git commit-tree "$tree" \
    -m "Site built by cargo xtask site from $(git rev-parse --short HEAD)")
git push --force origin "$commit:refs/heads/gh-pages"
echo "published $commit; GitHub needs about a minute to serve it"
