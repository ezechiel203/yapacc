#!/bin/sh
# Capture real package-manager output as parser fixtures.
#
# Runs INSIDE a throwaway container of one distribution and writes one file
# per command into /out. Every command runs with LC_ALL=C so the text is
# what the parsers will see. Nothing here touches the host.
#
# Usage (from the repository root, rootless podman):
#   podman run --rm -v ./contrib/fixtures/capture-native.sh:/capture.sh:ro,z \
#       -v ./fixtures/native/<family>:/out:z <image> sh /capture.sh
#
# Each output file starts with a header line:  # cmd: <the exact command>
# followed by the raw output, then a trailer:   # exit: <status>
set -u
export LC_ALL=C DEBIAN_FRONTEND=noninteractive
PKG=bash           # a package that exists everywhere
LEAF=tree          # a small package with no reverse dependencies
QUERY=tree         # search term
FILE=/usr/bin/env  # a path owned by some package everywhere

cap() { # cap <file> <command...>
  f="/out/$1"; shift
  { printf '# cmd: %s\n' "$*"; "$@" 2>&1; printf '# exit: %s\n' "$?"; } > "$f"
}
capsh() { # capsh <file> <shell string>
  f="/out/$1"; shift
  { printf '# cmd: %s\n' "$1"; sh -c "$1" 2>&1; printf '# exit: %s\n' "$?"; } > "$f"
}

cp /etc/os-release /out/os-release.txt

if command -v dpkg-query >/dev/null 2>&1; then
  apt-get update >/dev/null 2>&1
  apt-get install -y "$LEAF" >/dev/null 2>&1
  apt-mark auto "$LEAF" >/dev/null 2>&1
  cap list.txt dpkg-query -W -f '${Package}\t${Version}\t${db:Status-Abbrev}\t${Installed-Size}\t${Essential}\t${Priority}\t${binary:Summary}\n'
  cap explicit.txt apt-mark showmanual
  cap info.txt apt-cache show --no-all-versions "$PKG"
  cap search.txt apt-cache search "$QUERY"
  cap owner.txt dpkg-query -S "$FILE"
  cap owner-missing.txt dpkg-query -S /nonexistent/yapacc-fixture
  cap files.txt dpkg-query -L "$LEAF"
  cap orphans.txt apt-get -s autoremove
  cap updates.txt apt-get -s upgrade
  capsh log-dpkg.txt 'tail -n 40 /var/log/dpkg.log'
  capsh log-apt-history.txt 'tail -n 40 /var/log/apt/history.log'
elif command -v rpm >/dev/null 2>&1 && command -v dnf >/dev/null 2>&1; then
  dnf -q -y install "$LEAF" >/dev/null 2>&1
  dnf -q -y mark dependency "$LEAF" >/dev/null 2>&1 || dnf -q -y mark remove "$LEAF" >/dev/null 2>&1
  # dnf5 does not expand "\t" in --qf; pass real TAB and newline characters,
  # exactly as the Rust code does.
  T=$(printf '\t'); N='
'
  cap dnf-version.txt dnf --version
  cap list.txt rpm -qa --qf '%{NAME}\t%{EVR}\t%{ARCH}\t%{SIZE}\t%{INSTALLTIME}\t%{SUMMARY}\n'
  cap explicit.txt dnf -q repoquery --userinstalled --qf "%{name}$N"
  cap info.txt dnf -q repoquery --qf "%{name}$T%{evr}$T%{arch}$T%{repoid}$T%{installsize}$T%{summary}$N" "$PKG"
  cap search.txt dnf -q repoquery --qf "%{name}$T%{evr}$T%{repoid}$T%{summary}$N" "*$QUERY*"
  cap owner.txt rpm -qf --qf '%{NAME}\n' "$FILE"
  cap owner-missing.txt rpm -qf --qf '%{NAME}\n' /nonexistent/yapacc-fixture
  cap files.txt rpm -ql "$LEAF"
  cap orphans.txt dnf -q repoquery --unneeded --qf "%{name}$N"
  cap updates.txt dnf -q repoquery --upgrades --qf "%{name}$T%{evr}$T%{repoid}$N"
  cap log-history-list.txt dnf history list
  capsh log-dnf5.txt 'tail -n 40 /var/log/dnf5.log 2>/dev/null || tail -n 40 /var/log/dnf.rpm.log'
elif command -v pacman >/dev/null 2>&1; then
  pacman -Sy --noconfirm >/dev/null 2>&1
  pacman -S --noconfirm --asdeps "$LEAF" >/dev/null 2>&1
  cap list.txt pacman -Qi
  cap explicit.txt pacman -Qeq
  cap info.txt pacman -Si "$PKG"
  cap search.txt pacman -Ss "$QUERY"
  cap owner.txt pacman -Qo "$FILE"
  cap owner-missing.txt pacman -Qo /nonexistent/yapacc-fixture
  cap files.txt pacman -Qlq "$LEAF"
  cap orphans.txt pacman -Qdtq
  cap updates.txt pacman -Qu
  capsh log-pacman.txt 'tail -n 40 /var/log/pacman.log'
fi
ls -la /out
