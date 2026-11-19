#!/bin/sh
# yapacc - fill version and checksums of a release into the packaging recipes.
# Copyright (C) 2026 Alexandre BETRY
# SPDX-License-Identifier: GPL-2.0-only
#
# Usage: fill-release.sh [-d DIR] VERSION SOURCE_TARBALL
#
#   VERSION         release version without the leading "v" (0.2.0)
#   SOURCE_TARBALL  the source tarball the recipes will build from; it must
#                   unpack to yapacc-VERSION/ (README, "The recipes")
#   -d DIR          recipe directory to edit (default: the directory of this
#                   script). Pass a copy to leave the checkout untouched.
#
# What is edited, in DIR:
#   arch/PKGBUILD, arch/.SRCINFO   pkgver, pkgrel, source line, sha256sums
#   alpine/APKBUILD                pkgver, pkgrel, sha512sums (abuild format)
#   void/template                  version, revision, checksum (sha256)
#   rpm/yapacc.spec                Version, Release, a %changelog entry
#   debian/changelog               a new top entry for a new version; the
#                                  distribution of the entry of VERSION
#                                  (UNRELEASED in the checkout) becomes
#                                  $DEB_DISTRIBUTION, "unstable" by default
#
# Environment: DEB_DISTRIBUTION (see above); SOURCE_DATE_EPOCH, when set, is
# the date of the changelog entries this run adds (otherwise: now), so that
# the same release filled twice gives the same files.
#
# The checksum is that of the tarball given, so the URL the recipes point at
# must serve exactly that file (for example the release asset, not GitHub's
# generated archive, whose bytes are not guaranteed to stay the same).
# Running it twice with the same arguments changes nothing the second time.
# The version of a recipe that already has VERSION keeps its release number.

set -eu

die() {
	printf 'fill-release: %s\n' "$*" >&2
	exit 1
}

dir=$(cd -- "$(dirname -- "$0")" && pwd)
while getopts d: opt; do
	case $opt in
	d) dir=$(cd -- "$OPTARG" && pwd) || exit 1 ;;
	*) die "usage: fill-release.sh [-d DIR] VERSION SOURCE_TARBALL" ;;
	esac
done
shift $((OPTIND - 1))
[ $# -eq 2 ] || die "usage: fill-release.sh [-d DIR] VERSION SOURCE_TARBALL"
version=$1
tarball=$2

case $version in
v*) die "give the version without the leading v: ${version#v}" ;;
'' | *[!0-9A-Za-z.+~_-]*) die "unusable version: $version" ;;
esac
[ -f "$tarball" ] || die "no such file: $tarball"
for f in arch/PKGBUILD arch/.SRCINFO alpine/APKBUILD void/template \
	rpm/yapacc.spec debian/changelog; do
	[ -f "$dir/$f" ] || die "missing $dir/$f"
done

hash_of() { # algorithm file -> hex digest
	"$1" "$2" | cut -d ' ' -f 1
}
if command -v sha256sum >/dev/null 2>&1; then
	sha256=$(hash_of sha256sum "$tarball")
	sha512=$(hash_of sha512sum "$tarball")
elif command -v shasum >/dev/null 2>&1; then
	sha256=$(shasum -a 256 "$tarball" | cut -d ' ' -f 1)
	sha512=$(shasum -a 512 "$tarball" | cut -d ' ' -f 1)
else
	die "need sha256sum or shasum"
fi
[ ${#sha256} -eq 64 ] && [ ${#sha512} -eq 128 ] || die "could not compute checksums"

# Replace a file's content in place, keeping its mode (sed -i is not POSIX).
# $1 file, rest: sed arguments.
edit() {
	_f=$1
	shift
	sed "$@" "$_f" >"$_f.new" || {
		rm -f "$_f.new"
		die "sed failed on $_f"
	}
	cat "$_f.new" >"$_f"
	rm -f "$_f.new"
}

# First capture group of a sed -n expression, empty when absent.
field() { # file sed-expression
	sed -n "$2" "$1" | head -n 1
}

# Escape a literal for the pattern side of a sed expression.
esc() { printf '%s' "$1" | sed 's/[][\.*^$/]/\\&/g'; }

# Print "1" (or the given reset value) when the recipe's current version is
# not the new one, else the current release number.
release_for() { # current-version current-release reset
	if [ "$1" = "$version" ] && [ -n "$2" ]; then printf '%s' "$2"; else printf '%s' "$3"; fi
}

# The date of new changelog entries, in the given format: SOURCE_DATE_EPOCH
# when it is set (GNU and busybox date take -d @N, BSD date takes -r N).
stamp() {
	if [ -n "${SOURCE_DATE_EPOCH:-}" ]; then
		LC_ALL=C date -u -d "@$SOURCE_DATE_EPOCH" "$1" 2>/dev/null ||
			LC_ALL=C date -u -r "$SOURCE_DATE_EPOCH" "$1"
	else
		LC_ALL=C date -u "$1"
	fi
}
now_rpm=$(stamp '+%a %b %d %Y') || die "cannot format a date"
now_deb=$(stamp '+%a, %d %b %Y %H:%M:%S +0000') || die "cannot format a date"

# --- Arch ------------------------------------------------------------------
f=$dir/arch/PKGBUILD
old=$(field "$f" 's/^pkgver=//p')
rel=$(release_for "$old" "$(field "$f" 's/^pkgrel=//p')" 1)
[ -n "$old" ] || die "no pkgver in $f"
# shellcheck disable=SC2016
edit "$f" -e "s/^pkgver=.*/pkgver=$version/" -e "s/^pkgrel=.*/pkgrel=$rel/" \
	-e "s/^sha256sums=.*/sha256sums=('$sha256')/"

f=$dir/arch/.SRCINFO
oldsrc=$(field "$f" 's/^[[:space:]]*pkgver = //p')
[ -n "$oldsrc" ] || die "no pkgver in $f"
edit "$f" -e "s/^\\([[:space:]]*pkgver = \\).*/\\1$version/" \
	-e "s/^\\([[:space:]]*pkgrel = \\).*/\\1$rel/" \
	-e "/^[[:space:]]*source = /s/$(esc "$oldsrc")/$version/g" \
	-e "s/^\\([[:space:]]*sha256sums = \\).*/\\1$sha256/"

# --- Alpine ----------------------------------------------------------------
f=$dir/alpine/APKBUILD
old=$(field "$f" 's/^pkgver=//p')
[ -n "$old" ] || die "no pkgver in $f"
rel=$(release_for "$old" "$(field "$f" 's/^pkgrel=//p')" 0)
# shellcheck disable=SC2016
edit "$f" -e "s/^pkgver=.*/pkgver=$version/" -e "s/^pkgrel=.*/pkgrel=$rel/" \
	-e '/^sha512sums="/,$d'
# abuild's format: the digest, two spaces, the name the source is saved as.
printf 'sha512sums="\n%s  yapacc-%s.tar.gz\n"\n' "$sha512" "$version" >>"$f"

# --- Void ------------------------------------------------------------------
f=$dir/void/template
old=$(field "$f" 's/^version=//p')
[ -n "$old" ] || die "no version in $f"
rel=$(release_for "$old" "$(field "$f" 's/^revision=//p')" 1)
edit "$f" -e "s/^version=.*/version=$version/" -e "s/^revision=.*/revision=$rel/" \
	-e "s/^checksum=.*/checksum=$sha256/"

# --- RPM -------------------------------------------------------------------
f=$dir/rpm/yapacc.spec
old=$(field "$f" 's/^Version:[[:space:]]*//p')
[ -n "$old" ] || die "no Version in $f"
oldrel=$(field "$f" 's/^Release:[[:space:]]*\([0-9][0-9]*\).*/\1/p')
rel=$(release_for "$old" "$oldrel" 1)
edit "$f" -e "s/^\\(Version:[[:space:]]*\\).*/\\1$version/" \
	-e "s/^\\(Release:[[:space:]]*\\)[0-9][0-9]*/\\1$rel/"
if ! grep -q -- "- $(esc "$version")-$rel\$" "$f"; then
	who=$(field "$f" 's/^\* [A-Za-z]* [A-Za-z]* [0-9]* [0-9]* \(.*\) - [^ ]*$/\1/p')
	[ -n "$who" ] || die "no earlier %changelog entry in $f to take the packager from"
	edit "$f" "/^%changelog/a\\
* $now_rpm $who - $version-$rel\\
- Release $version"
fi

# --- Debian ----------------------------------------------------------------
# The checkout says UNRELEASED, as Debian's tools expect of work in
# progress; a filled recipe names the distribution it is built for.
f=$dir/debian/changelog
dist=${DEB_DISTRIBUTION:-unstable}
case $dist in
'' | *[!0-9A-Za-z.-]*) die "unusable DEB_DISTRIBUTION: $dist" ;;
esac
if head -n 1 "$f" | grep -q "^yapacc ($(esc "$version")-[0-9][0-9]*)"; then
	edit "$f" -e "1s/^\\(yapacc ([^)]*)\\) [^;]*;/\\1 $dist;/"
else
	who=$(field "$f" 's/^ -- \(.*[^ ]\)  .*/\1/p')
	[ -n "$who" ] || die "no earlier entry in $f to take the maintainer from"
	{
		printf 'yapacc (%s-1) %s; urgency=medium\n\n  * Release %s.\n\n -- %s  %s\n\n' \
			"$version" "$dist" "$version" "$who" "$now_deb"
		cat "$f"
	} >"$f.new"
	cat "$f.new" >"$f"
	rm -f "$f.new"
fi

printf 'filled %s for %s\n  sha256 %s\n  sha512 %s\n' "$dir" "$version" "$sha256" "$sha512"
