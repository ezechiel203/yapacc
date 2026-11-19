#!/usr/bin/env bash
# yapacc - package lifecycle: install, upgrade, uninstall, in containers.
# Copyright (C) 2026 Alexandre BETRY
# SPDX-License-Identifier: GPL-2.0-only
#
# For each distribution (debian, fedora, alpine), with rootless podman (or
# docker):
#   1. builds the packages from contrib/packaging, twice: the version in
#      Cargo.toml and a bumped patch version, each from its own source
#      tarball (git archive of HEAD), after contrib/packaging/fill-release.sh
#      has filled version and checksums into a COPY of the recipes;
#   2. in a clean image installs the first build, upgrades to the second,
#      removes everything, and asserts that
#        - the package database says the expected version after each step,
#        - files the first build owned and the second no longer owns are
#          gone after the upgrade (the two builds differ by a marker file
#          each, /usr/share/yapacc/build-marker-<version>, added to the copy
#          of the recipes only, so that this can fail),
#        - after removal no file owned by any of the packages exists, no
#          directory named yapacc is left, and the package database is
#          empty of yapacc packages,
#        - a non-root user's ~/.config/yapacc (a sentinel file and a nested
#          one, with their modes and sizes) is byte-identical after every
#          step.
#
# What it does NOT cover: only x86_64 (whatever the host is); the recipes'
# own test steps are skipped (--without check / nocheck / !check) to keep the
# two builds short; the bumped version changes the package version, not the
# Cargo.toml version, so `yapacc --version` prints the same string for both;
# Arch and Void have no lifecycle check here.
#
# Usage: contrib/test/pkg-lifecycle.sh [--keep] [distro...]
#   distro   debian, fedora, alpine (default: all three, run in parallel)
#   --keep   keep the work directory (packages, logs) after success
#
# Environment: YAPACC_LIFECYCLE_REUSE=1 (reuse packages already built in the
# work directory), CONTAINER_ENGINE (podman, else docker), IMAGE_DEBIAN,
# IMAGE_FEDORA, IMAGE_ALPINE, YAPACC_LIFECYCLE_WORKDIR.
#
# Exit status: 0 all asked distributions passed, 1 a check failed,
# 2 usage or missing prerequisite.

set -u
set -o pipefail

PROG=${0##*/}
ROOT=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd)
IMAGE_DEBIAN=${IMAGE_DEBIAN:-docker.io/library/debian:testing}
IMAGE_FEDORA=${IMAGE_FEDORA:-registry.fedoraproject.org/fedora:latest}
IMAGE_ALPINE=${IMAGE_ALPINE:-docker.io/library/alpine:latest}

keep=0
distros=()
while [ $# -gt 0 ]; do
	case $1 in
	--keep) keep=1 ;;
	-h | --help)
		sed -n '2,/^$/p' "$0" | sed 's/^# \{0,1\}//'
		exit 0
		;;
	debian | fedora | alpine) distros+=("$1") ;;
	*)
		echo "$PROG: unknown argument: $1" >&2
		exit 2
		;;
	esac
	shift
done
[ ${#distros[@]} -gt 0 ] || distros=(debian fedora alpine)

ENGINE=${CONTAINER_ENGINE:-}
if [ -z "$ENGINE" ]; then
	if command -v podman >/dev/null 2>&1; then ENGINE=podman
	elif command -v docker >/dev/null 2>&1; then ENGINE=docker
	else
		echo "$PROG: need podman or docker" >&2
		exit 2
	fi
fi
for tool in git gzip sha256sum; do
	command -v "$tool" >/dev/null 2>&1 || {
		echo "$PROG: need $tool" >&2
		exit 2
	}
done

V1=$(sed -n '/^\[package\]/,/^\[/s/^version = "\(.*\)"$/\1/p' "$ROOT/Cargo.toml" | head -n 1)
[ -n "$V1" ] || {
	echo "$PROG: cannot read the version from Cargo.toml" >&2
	exit 2
}
case $V1 in
*[!0-9.]*)
	echo "$PROG: expected a plain x.y.z version, got $V1" >&2
	exit 2
	;;
esac
V2="${V1%.*}.$((${V1##*.} + 1))"

WORK=${YAPACC_LIFECYCLE_WORKDIR:-$(mktemp -d "${TMPDIR:-/tmp}/yapacc-lifecycle.XXXXXX")}
mkdir -p "$WORK"
echo "$PROG: versions $V1 -> $V2, engine $ENGINE, work directory $WORK"

# --- inputs: tarballs and filled copies of the recipes (host side) ---------
IN=$WORK/in
mkdir -p "$IN"
for v in "$V1" "$V2"; do
	git -C "$ROOT" archive --format=tar --prefix="yapacc-$v/" HEAD | gzip -n -9 >"$IN/yapacc-$v.tar.gz" || {
		echo "$PROG: git archive failed" >&2
		exit 2
	}
	rm -rf "$IN/recipes-$v"
	cp -r "$ROOT/contrib/packaging" "$IN/recipes-$v"
	# The recipes in HEAD carry V1; fill-release.sh moves a copy to $v and
	# writes the checksum of the tarball that copy will build from.
	sh "$ROOT/contrib/packaging/fill-release.sh" -d "$IN/recipes-$v" "$v" "$IN/yapacc-$v.tar.gz" >"$WORK/fill-$v.log" 2>&1 || {
		cat "$WORK/fill-$v.log" >&2
		echo "$PROG: fill-release.sh failed for $v" >&2
		exit 2
	}
done
# Alpine: skip abuild's check() to keep the build short (same as the other
# two distributions below).
for v in "$V1" "$V2"; do
	sed -i 's/^options="net"$/options="net !check"/' "$IN/recipes-$v/alpine/APKBUILD"
done
# Make the two builds own different files, so that "no file of the old build
# left after upgrade" can fail: each build of the COPY of the recipes (never
# the checked-in ones) also installs /usr/share/yapacc/build-marker-<version>
# into the core package. After the upgrade the marker of $V1 must be gone and
# the marker of $V2 must exist.
for v in "$V1" "$V2"; do
	m=build-marker-$v
	r=$IN/recipes-$v
	echo "build $v" >"$r/debian/$m"
	echo "debian/$m usr/share/yapacc" >>"$r/debian/yapacc.install"
	sed -i -e "/^%install\$/a\\
install -Dpm 0644 /dev/null %{buildroot}%{_datadir}/yapacc/$m" \
		-e "/^%files\$/a\\
%dir %{_datadir}/yapacc\\
%{_datadir}/yapacc/$m" "$r/rpm/yapacc.spec"
	sed -i "/^package() {\$/a\\
	install -Dm644 /dev/null \"\$pkgdir\"/usr/share/yapacc/$m" "$r/alpine/APKBUILD"
	grep -q "$m" "$r/rpm/yapacc.spec" && grep -q "$m" "$r/alpine/APKBUILD" || {
		echo "$PROG: could not add the marker to the recipes" >&2
		exit 2
	}
done

# --- container-side scripts --------------------------------------------------
cat >"$IN/build.sh" <<'BUILD'
#!/bin/sh
# Runs as root in a clean image: $1 distro, $2 and $3 versions. Inputs in
# /in (read-only), results in /out/<version>/.
set -eu
distro=$1
shift
mkdir -p /b
case $distro in
debian)
	export DEBIAN_FRONTEND=noninteractive
	apt-get update -qq
	apt-get install -y -qq --no-install-recommends build-essential debhelper \
		cargo rustc fakeroot ca-certificates >/dev/null
	for v in "$@"; do
		mkdir -p "/b/$v" "/out/$v"
		tar -xzf "/in/yapacc-$v.tar.gz" -C "/b/$v"
		cd "/b/$v/yapacc-$v"
		cp -r "/in/recipes-$v/debian" debian
		DEB_BUILD_OPTIONS=nocheck dpkg-buildpackage -us -uc -b >"/b/$v.log" 2>&1 ||
			{ tail -n 40 "/b/$v.log"; exit 1; }
		cp /b/"$v"/*.deb "/out/$v/"
	done
	;;
fedora)
	dnf -y -q install rpm-build cargo rust gcc >/dev/null
	for v in "$@"; do
		mkdir -p "/b/$v/SOURCES" "/out/$v"
		cp "/in/yapacc-$v.tar.gz" "/b/$v/SOURCES/"
		rpmbuild -bb --without check --define "_topdir /b/$v" \
			"/in/recipes-$v/rpm/yapacc.spec" >"/b/$v.log" 2>&1 ||
			{ tail -n 40 "/b/$v.log"; exit 1; }
		find "/b/$v/RPMS" -name '*.rpm' ! -name '*debug*' -exec cp {} "/out/$v/" \;
	done
	;;
alpine)
	apk add -q --no-cache alpine-sdk cargo >/dev/null
	abuild-keygen -a -n >/dev/null 2>&1
	cp /root/.abuild/*.rsa.pub /etc/apk/keys/
	mkdir -p /var/cache/distfiles
	for v in "$@"; do
		mkdir -p "/b/$v" "/out/$v"
		cp "/in/yapacc-$v.tar.gz" /var/cache/distfiles/
		cp "/in/recipes-$v/alpine/APKBUILD" "/b/$v/"
		cd "/b/$v"
		REPODEST=/b/repo-$v abuild -F -r >"/b/$v.log" 2>&1 ||
			{ tail -n 40 "/b/$v.log"; exit 1; }
		find "/b/repo-$v" -name '*.apk' -exec cp {} "/out/$v/" \;
	done
	;;
esac
ls -l /out/*/
BUILD

cat >"$IN/life.sh" <<'LIFE'
#!/bin/sh
# Runs as root in a CLEAN image: $1 distro, $2 first version, $3 second.
# Packages are in /out/<version>/ (read-only).
set -u
distro=$1 v1=$2 v2=$3
fails=0
ok() { printf 'PASS  %s\n' "$*"; }
bad() { printf 'FAIL  %s\n' "$*"; fails=$((fails + 1)); }
check() { # description, command...
	d=$1
	shift
	if "$@" >/dev/null 2>&1; then ok "$d"; else bad "$d"; fi
}

# Per-distribution verbs.
case $distro in
debian)
	export DEBIAN_FRONTEND=noninteractive
	apt-get update -qq >/dev/null 2>&1
	useradd -m alice
	pm_install() { apt-get install -y -qq --no-install-recommends "$@" >/dev/null; }
	pm_remove() { apt-get remove -y -qq "$@" >/dev/null; }
	pm_installed() { dpkg-query -W -f '${Package} ${db:Status-Abbrev}\n' 'yapacc*' 2>/dev/null | awk '$2 == "ii" { print $1 }'; }
	pm_version() { dpkg-query -W -f '${Version}' yapacc 2>/dev/null; }
	pm_files() { for p in $(pm_installed); do dpkg -L "$p"; done; }
	pkgfiles() { ls /out/"$1"/*.deb; }
	want_version() { printf '%s-1' "$1"; }
	;;
fedora)
	# The base image has no su; test tooling, installed before the packages.
	dnf -y -q install util-linux >/dev/null 2>&1
	useradd -m alice
	pm_install() { dnf -y -q install "$@" >/dev/null; }
	pm_remove() { dnf -y -q remove "$@" >/dev/null; }
	pm_installed() { rpm -qa --qf '%{NAME}\n' 'yapacc*' | sort; }
	pm_version() { rpm -q --qf '%{VERSION}' yapacc 2>/dev/null; }
	pm_files() { for p in $(pm_installed); do rpm -ql "$p"; done; }
	pkgfiles() { ls /out/"$1"/*.rpm; }
	want_version() { printf '%s' "$1"; }
	;;
alpine)
	adduser -D alice
	pm_install() { apk add -q --allow-untrusted "$@" >/dev/null; }
	pm_remove() { apk del -q "$@" >/dev/null; }
	pm_installed() { apk info | grep '^yapacc' | sort; }
	pm_version() { apk list -I yapacc 2>/dev/null | sed -n 's/^yapacc-\([0-9][^ ]*\)-r[0-9]* .*/\1/p'; }
	pm_files() { for p in $(pm_installed); do apk info -L "$p" | sed -n '/ contains:$/!p'; done; }
	pkgfiles() { ls /out/"$1"/*.apk; }
	want_version() { printf '%s' "$1"; }
	;;
esac

# Absolute, sorted, unique paths owned by the installed yapacc packages.
owned() { pm_files | grep -v '^$' | sed 's|^\([^/]\)|/\1|' | sort -u; }

as_alice() { su -s /bin/sh alice -c "HOME=/home/alice; export HOME; $1"; }

# The sentinel configuration of the user.
cfg=/home/alice/.config/yapacc
as_alice 'mkdir -p ~/.config/yapacc/nested && echo "mine = true" > ~/.config/yapacc/config.toml && echo keep > ~/.config/yapacc/nested/sentinel && chmod 600 ~/.config/yapacc/config.toml'
manifest() { (cd /home/alice && find .config/yapacc -exec stat -c '%n %a %s %U' {} + | sort; find .config/yapacc -type f -exec sha256sum {} + | sort); }
manifest >/tmp/cfg.before
[ "$(wc -l </tmp/cfg.before)" -ge 6 ] && ok "sentinel configuration created for a non-root user" ||
	{ bad "could not create the sentinel configuration"; cat /tmp/cfg.before; }
cfg_same() {
	manifest >/tmp/cfg.now
	# cmp and diff are not in every base image: compare as strings.
	if [ "$(cat /tmp/cfg.before)" = "$(cat /tmp/cfg.now)" ]; then ok "user config untouched $1"; else
		bad "user config changed $1"
		echo "--- before"; cat /tmp/cfg.before; echo "--- now"; cat /tmp/cfg.now
	fi
}
echo "== install $v1"
# shellcheck disable=SC2046
pm_install $(pkgfiles "$v1")
[ "$(pm_version)" = "$(want_version "$v1")" ] && ok "installed version $(pm_version)" ||
	bad "installed version is '$(pm_version)', wanted $(want_version "$v1")"
owned >/tmp/owned.1
n1=$(wc -l </tmp/owned.1)
[ "$n1" -gt 0 ] && ok "first build owns $n1 paths" || bad "no owned paths listed"
[ "$(pm_installed | wc -l)" -eq "$(pkgfiles "$v1" | wc -l)" ] && ok "every built package is installed" ||
	bad "installed: $(pm_installed | tr '\n' ' ')"
check "core binary installed" test -x /usr/bin/yapacc
as_alice 'yapacc --version' && ok "yapacc --version as a non-root user" || bad "yapacc --version as a non-root user"
cfg_same "after install"
# Components are optional but these packages ship them.
echo "packages after install: $(pm_installed | tr '\n' ' ')"

echo "== upgrade to $v2"
# shellcheck disable=SC2046
pm_install $(pkgfiles "$v2")
[ "$(pm_version)" = "$(want_version "$v2")" ] && ok "upgraded version $(pm_version)" ||
	bad "version after upgrade is '$(pm_version)', wanted $(want_version "$v2")"
owned >/tmp/owned.2
# Everything the first build owned and the second does not must be gone. The
# builds differ by a marker file each (see the host side), so the set is
# not empty; if it were, this check would prove nothing and must fail.
marker1=/usr/share/yapacc/build-marker-$v1
marker2=/usr/share/yapacc/build-marker-$v2
grep -qx "$marker1" /tmp/owned.1 && ! grep -qx "$marker1" /tmp/owned.2 && grep -qx "$marker2" /tmp/owned.2 &&
	ok "the two builds own different files (marker of $v1 only in the first)" ||
	bad "the builds do not differ by their markers: the stale-file check below would be vacuous"
[ -f "$marker2" ] && [ ! -e "$marker1" ] && ok "marker of $v2 present, marker of $v1 gone after upgrade" ||
	bad "after upgrade: marker $v1 exists=$([ -e "$marker1" ] && echo yes || echo no), marker $v2 exists=$([ -e "$marker2" ] && echo yes || echo no)"
stale=0
for p in $(comm -23 /tmp/owned.1 /tmp/owned.2); do
	if [ -f "$p" ] || [ -L "$p" ]; then
		echo "  left over after upgrade: $p"
		stale=1
	fi
done
[ $stale -eq 0 ] && ok "no file of the old build left after upgrade" || bad "stale files after upgrade"
check "core binary present after upgrade" test -x /usr/bin/yapacc
as_alice 'yapacc --version' && ok "yapacc --version after upgrade" || bad "yapacc --version after upgrade"
cfg_same "after upgrade"

echo "== remove"
[ -n "$(pm_installed)" ] || bad "nothing to remove: the package database lists no yapacc package"
# shellcheck disable=SC2046
pm_remove $(pm_installed)
cat /tmp/owned.1 /tmp/owned.2 | sort -u >/tmp/owned.all
left=0
while IFS= read -r p; do
	[ -n "$p" ] || continue
	if [ -f "$p" ] || [ -L "$p" ]; then
		echo "  still there: $p"
		left=1
	elif [ -d "$p" ]; then
		case $p in *yapacc*)
			echo "  directory still there: $p"
			left=1
			;;
		esac
	fi
done </tmp/owned.all
[ $left -eq 0 ] && ok "no owned file or yapacc directory left after removal" ||
	bad "files remain after removal"
[ -z "$(pm_installed)" ] && ok "package database lists no yapacc package" ||
	bad "database still lists: $(pm_installed | tr '\n' ' ')"
command -v yapacc >/dev/null 2>&1 && bad "yapacc still on PATH" || ok "yapacc not on PATH"
check "stray yapacc files outside the user's home" sh -c '! find /usr /etc /opt /var/lib -name "*yapacc*" 2>/dev/null | grep -q .'
cfg_same "after removal"

if [ "$distro" = debian ]; then
	echo "== purge"
	apt-get purge -y -qq yapacc yapacc-tui yapacc-mcp >/dev/null 2>&1
	check "dpkg state empty after purge" sh -c '! dpkg -l "yapacc*" 2>/dev/null | grep -q "^[a-z][a-z] "'
	cfg_same "after purge"
fi

echo "RESULT $distro failures=$fails"
[ $fails -eq 0 ]
LIFE

# --- one distribution ----------------------------------------------------------
run_distro() { # distro image
	d=$1
	img=$2
	log=$WORK/$d.log
	out=$WORK/out-$d
	if [ "${YAPACC_LIFECYCLE_REUSE:-0}" != 1 ]; then rm -rf "$out"; fi
	mkdir -p "$out"
	{
		if [ -d "$out/$V1" ] && [ -d "$out/$V2" ]; then
			echo "### reusing packages already built in $out"
		else
			echo "### build $d ($img)"
			"$ENGINE" run --rm -v "$IN:/in:ro" -v "$out:/out" "$img" sh /in/build.sh "$d" "$V1" "$V2" || {
				echo "BUILD FAILED $d"
				exit 1
			}
		fi
		echo "### lifecycle $d (clean $img)"
		"$ENGINE" run --rm -v "$IN:/in:ro" -v "$out:/out:ro" "$img" sh /in/life.sh "$d" "$V1" "$V2"
	} >"$log" 2>&1
	rc=$?
	echo "$rc" >"$WORK/rc-$d"
	return $rc
}

pids=()
for d in "${distros[@]}"; do
	case $d in
	debian) img=$IMAGE_DEBIAN ;;
	fedora) img=$IMAGE_FEDORA ;;
	alpine) img=$IMAGE_ALPINE ;;
	esac
	run_distro "$d" "$img" &
	pids+=($!)
done
status=0
for p in "${pids[@]}"; do wait "$p" || status=1; done

echo
for d in "${distros[@]}"; do
	echo "=== $d (log: $WORK/$d.log)"
	grep -E '^(PASS|FAIL|RESULT|BUILD FAILED|==|packages after)' "$WORK/$d.log" || tail -n 20 "$WORK/$d.log"
done
echo
if [ $status -eq 0 ]; then
	echo "$PROG: all passed (${distros[*]})"
	[ $keep -eq 1 ] || rm -rf "$WORK"
else
	echo "$PROG: FAILED, work directory kept: $WORK"
fi
exit $status
