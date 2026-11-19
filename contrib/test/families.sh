#!/usr/bin/env bash
# yapacc — one portable binary, every supported family.
# Copyright (C) 2026 Alexandre BETRY
# SPDX-License-Identifier: GPL-2.0-only
#
# Mounts ONE yapacc binary read-only into a container of each supported
# distribution family and checks, as root inside the container, that the
# read side answers correctly and that the write side plans, refuses,
# applies and takes back a change through the image's own package manager.
#
# Usage:
#   contrib/test/families.sh [options] [path/to/yapacc] [image[=manager]...]
#
# Options:
#   --keep-going     run every check on every image (default)
#   --fail-fast      stop at the first failed check
#   --timeout SECS   limit for one command inside a container (default 300)
#   --list           print the default images and exit
#   -h, --help       this text
#
# Environment:
#   CONTAINER_ENGINE   podman (default when present) or docker
#   YAPACC_TEST_TIMEOUT, YAPACC_TEST_PULL_TIMEOUT   seconds (300, 900)
#   YAPACC_TEST_LOGDIR  where captured output is kept (default: a temp dir,
#                       removed when every check passed)
#
# The binary defaults to target/x86_64-unknown-linux-musl/release/yapacc and
# falls back to target/release/yapacc. A glibc-linked binary cannot run on
# Alpine or Void-musl; those images are skipped (not failed) for it unless
# they are named explicitly.
#
# Exit status: 0 every check passed, 1 at least one failed, 2 usage or
# missing prerequisite.

set -u
set -o pipefail

PROG=${0##*/}
ROOT=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd)

DEFAULT_IMAGES=(
    "debian:stable=apt"
    "ubuntu:latest=apt"
    "fedora:latest=dnf"
    "archlinux:latest=pacman"
    "opensuse/tumbleweed:latest=zypper"
    "alpine:latest=apk"
    "ghcr.io/void-linux/void-glibc:latest=xbps"
)
# Added to the defaults only when the binary is statically linked.
STATIC_ONLY_IMAGES=(
    "ghcr.io/void-linux/void-musl:latest=xbps"
)

FAIL_FAST=0
TIMEOUT=${YAPACC_TEST_TIMEOUT:-300}
PULL_TIMEOUT=${YAPACC_TEST_PULL_TIMEOUT:-900}
# The package the write side installs and takes back. Present under this
# name in every default image's repositories, and installed in none.
PROBE_PKG=tree
IN_CONTAINER=/usr/local/bin/yapacc

usage() {
    sed -n '2,/^$/p' "$0" | sed -e 's/^# \{0,1\}//'
}

die() {
    printf '%s: %s\n' "$PROG" "$*" >&2
    exit 2
}

warn() {
    printf '%s: warning: %s\n' "$PROG" "$*" >&2
}

# ---------------------------------------------------------------- arguments

POSITIONAL=()
while [ "$#" -gt 0 ]; do
    case "$1" in
        --keep-going) FAIL_FAST=0 ;;
        --fail-fast) FAIL_FAST=1 ;;
        --timeout)
            [ "$#" -ge 2 ] || die "--timeout needs a number of seconds"
            TIMEOUT=$2
            shift
            ;;
        --timeout=*) TIMEOUT=${1#--timeout=} ;;
        --list)
            printf '%s\n' "${DEFAULT_IMAGES[@]}" "${STATIC_ONLY_IMAGES[@]}"
            exit 0
            ;;
        -h | --help)
            usage
            exit 0
            ;;
        --)
            shift
            while [ "$#" -gt 0 ]; do
                POSITIONAL+=("$1")
                shift
            done
            break
            ;;
        -*) die "unknown option '$1' (see --help)" ;;
        *) POSITIONAL+=("$1") ;;
    esac
    shift
done

case "$TIMEOUT" in
    '' | *[!0-9]*) die "--timeout '$TIMEOUT' is not a whole number of seconds" ;;
esac
case "$PULL_TIMEOUT" in
    '' | *[!0-9]*) die "YAPACC_TEST_PULL_TIMEOUT '$PULL_TIMEOUT' is not a whole number of seconds" ;;
esac

BIN=""
IMAGES=()
EXPLICIT_IMAGES=0
if [ "${#POSITIONAL[@]}" -gt 0 ]; then
    first=${POSITIONAL[0]}
    # The first argument is the binary when it names a file; an image
    # reference never does.
    if [ -f "$first" ]; then
        BIN=$first
        POSITIONAL=("${POSITIONAL[@]:1}")
    elif [ -e "$first" ] || [ "${first#/}" != "$first" ] || [ "${first#./}" != "$first" ]; then
        die "'$first' is not a file"
    fi
fi
if [ "${#POSITIONAL[@]}" -gt 0 ]; then
    IMAGES=("${POSITIONAL[@]}")
    EXPLICIT_IMAGES=1
fi

if [ -z "$BIN" ]; then
    for candidate in \
        "$ROOT/target/x86_64-unknown-linux-musl/release/yapacc" \
        "$ROOT/target/release/yapacc"; do
        if [ -f "$candidate" ]; then
            BIN=$candidate
            break
        fi
    done
    [ -n "$BIN" ] || die "no binary found; build one first:
  cargo build --release --no-default-features --target x86_64-unknown-linux-musl --bin yapacc
or pass its path as the first argument"
fi
[ -x "$BIN" ] || die "'$BIN' is not executable"
BIN=$(cd -- "$(dirname -- "$BIN")" && pwd)/$(basename -- "$BIN")

# ------------------------------------------------------------ prerequisites

ENGINE=${CONTAINER_ENGINE:-}
if [ -z "$ENGINE" ]; then
    if command -v podman >/dev/null 2>&1; then
        ENGINE=podman
    elif command -v docker >/dev/null 2>&1; then
        ENGINE=docker
    else
        die "neither podman nor docker was found; install one or set CONTAINER_ENGINE"
    fi
fi
command -v "$ENGINE" >/dev/null 2>&1 || die "container engine '$ENGINE' was not found"

HAVE_TIMEOUT=0
if command -v timeout >/dev/null 2>&1; then
    HAVE_TIMEOUT=1
else
    warn "no 'timeout' command on this host: a hung container command will hang this script"
fi

JSON_TOOL=""
if command -v jq >/dev/null 2>&1; then
    JSON_TOOL=jq
elif command -v python3 >/dev/null 2>&1; then
    JSON_TOOL=python3
else
    die "the JSON checks need jq or python3 on this host"
fi

# Is the binary statically linked? yes / no / unknown.
binary_is_static() {
    local out
    if command -v file >/dev/null 2>&1; then
        out=$(file -L -- "$BIN" 2>/dev/null)
        case "$out" in
            *"statically linked"* | *"static-pie linked"*)
                echo yes
                return
                ;;
            *"dynamically linked"*)
                echo no
                return
                ;;
        esac
    fi
    if command -v ldd >/dev/null 2>&1; then
        out=$(ldd "$BIN" 2>&1)
        case "$out" in
            *"not a dynamic executable"* | *"statically linked"*)
                echo yes
                return
                ;;
            *"=>"* | *"ld-linux"* | *"ld-musl"*)
                echo no
                return
                ;;
        esac
    fi
    echo unknown
}

STATIC=$(binary_is_static)

if [ "$EXPLICIT_IMAGES" -eq 0 ]; then
    IMAGES=("${DEFAULT_IMAGES[@]}")
    if [ "$STATIC" = yes ]; then
        IMAGES+=("${STATIC_ONLY_IMAGES[@]}")
    fi
fi

case "$STATIC" in
    no)
        warn "$BIN is dynamically linked (glibc): it cannot run on Alpine or Void-musl."
        warn "build the static one: cargo build --release --no-default-features --target x86_64-unknown-linux-musl --bin yapacc"
        ;;
    unknown)
        warn "could not tell whether $BIN is statically linked (no 'file' or 'ldd'); assuming it is"
        ;;
esac

LOGDIR=${YAPACC_TEST_LOGDIR:-}
LOGDIR_IS_TEMP=0
if [ -z "$LOGDIR" ]; then
    LOGDIR=$(mktemp -d "${TMPDIR:-/tmp}/yapacc-families.XXXXXX") || die "could not create a temporary directory"
    LOGDIR_IS_TEMP=1
else
    mkdir -p -- "$LOGDIR" || die "could not create '$LOGDIR'"
fi

# ---------------------------------------------------------------- bookkeeping

CID=""          # the running container of the current image
IMAGE=""        # the current image reference, as given
REF=""          # the same, as handed to the container engine
MANAGER=""      # its expected native package manager
SLUG=""         # the image reference as a file name
SEQ=0           # numbers the captured output files
RC=0            # exit status of the last container command
OUT=""          # file holding its standard output
ERR=""          # file holding its standard error

TOTAL_PASS=0
TOTAL_FAIL=0
TOTAL_SKIP=0
IMG_PASS=0
IMG_FAIL=0
IMG_SKIP=0
IMG_FAILED_NAMES=""
SUMMARY_ROWS=()
STOPPED_EARLY=0

cleanup_container() {
    if [ -n "$CID" ]; then
        "$ENGINE" rm -f "$CID" >/dev/null 2>&1
        CID=""
    fi
}

trap cleanup_container EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

pass() {
    printf 'PASS  %-42s %s\n' "$IMAGE" "$1"
    IMG_PASS=$((IMG_PASS + 1))
    TOTAL_PASS=$((TOTAL_PASS + 1))
}

skip() {
    printf 'SKIP  %-42s %s — %s\n' "$IMAGE" "$1" "$2"
    IMG_SKIP=$((IMG_SKIP + 1))
    TOTAL_SKIP=$((TOTAL_SKIP + 1))
}

# fail NAME REASON — also shows the end of what the command printed.
fail() {
    printf 'FAIL  %-42s %s — %s\n' "$IMAGE" "$1" "$2"
    if [ -n "$OUT" ] && [ -s "$OUT" ]; then
        tail -n 8 -- "$OUT" | sed 's/^/        stdout| /'
    fi
    if [ -n "$ERR" ] && [ -s "$ERR" ]; then
        tail -n 8 -- "$ERR" | sed 's/^/        stderr| /'
    fi
    IMG_FAIL=$((IMG_FAIL + 1))
    TOTAL_FAIL=$((TOTAL_FAIL + 1))
    IMG_FAILED_NAMES="${IMG_FAILED_NAMES:+$IMG_FAILED_NAMES, }$1"
    if [ "$FAIL_FAST" -eq 1 ]; then
        STOPPED_EARLY=1
    fi
}

stopped() {
    [ "$STOPPED_EARLY" -eq 1 ]
}

# with_timeout SECS COMMAND... — run on the host under a time limit.
with_timeout() {
    local secs=$1
    shift
    if [ "$HAVE_TIMEOUT" -eq 1 ]; then
        timeout -k 15 "$secs" "$@"
    else
        "$@"
    fi
}

# in_container COMMAND... — run inside the current container, as root, with
# no terminal and no standard input. Sets RC, OUT and ERR.
in_container() {
    SEQ=$((SEQ + 1))
    OUT=$(printf '%s/%s.%03d.out' "$LOGDIR" "$SLUG" "$SEQ")
    ERR=$(printf '%s/%s.%03d.err' "$LOGDIR" "$SLUG" "$SEQ")
    printf '%s\n' "$*" >"$OUT.cmd"
    with_timeout "$TIMEOUT" "$ENGINE" exec --user 0 "$CID" "$@" >"$OUT" 2>"$ERR" </dev/null
    RC=$?
}

describe_rc() {
    case "$1" in
        124 | 137) printf 'timed out after %ss' "$TIMEOUT" ;;
        125) printf 'the container engine failed (status 125)' ;;
        126) printf 'the command could not be executed (status 126) — wrong architecture or libc?' ;;
        127) printf 'the command was not found (status 127)' ;;
        *) printf 'exit status %s' "$1" ;;
    esac
}

# rc_is "0 4" — is RC one of the listed statuses?
rc_is() {
    local want
    for want in $1; do
        if [ "$RC" -eq "$want" ]; then
            return 0
        fi
    done
    return 1
}

# expect NAME "ALLOWED STATUSES" COMMAND... — one check on the exit status.
# Returns 0 when it passed, so a caller can chain a check on the output.
expect() {
    local name=$1 allowed=$2
    shift 2
    if stopped; then
        return 1
    fi
    in_container "$@"
    if rc_is "$allowed"; then
        pass "$name"
        return 0
    fi
    fail "$name" "$(describe_rc "$RC"), expected ${allowed// / or }"
    return 1
}

# run_quiet NAME "ALLOWED STATUSES" COMMAND... — like expect, but reports
# only a failure; the caller prints the PASS after checking the output.
run_quiet() {
    local name=$1 allowed=$2
    shift 2
    if stopped; then
        return 1
    fi
    in_container "$@"
    if rc_is "$allowed"; then
        return 0
    fi
    fail "$name" "$(describe_rc "$RC"), expected ${allowed// / or }"
    return 1
}

# json_check FILE MODE [VALUE] — on the host, over captured output.
#   valid            parses as JSON and is not empty
#   nonempty-array   is an array with at least one element
#   manager VALUE    top-level "native_pkg_manager" equals VALUE
json_check() {
    local file=$1 mode=$2 value=${3:-}
    if [ "$JSON_TOOL" = jq ]; then
        case "$mode" in
            valid) jq -e 'true' <"$file" >/dev/null 2>&1 ;;
            nonempty-array) jq -e 'type == "array" and length > 0' <"$file" >/dev/null 2>&1 ;;
            manager) jq -e --arg m "$value" '.native_pkg_manager == $m' <"$file" >/dev/null 2>&1 ;;
            *) return 2 ;;
        esac
    else
        python3 -I -c '
import json, sys
path, mode, value = sys.argv[1], sys.argv[2], sys.argv[3]
try:
    with open(path, encoding="utf-8") as handle:
        data = json.load(handle)
except Exception:
    sys.exit(1)
if mode == "valid":
    ok = True
elif mode == "nonempty-array":
    ok = isinstance(data, list) and len(data) > 0
elif mode == "manager":
    ok = isinstance(data, dict) and data.get("native_pkg_manager") == value
else:
    sys.exit(2)
sys.exit(0 if ok else 1)
' "$file" "$mode" "$value"
    fi
}

# The manager expected for an image given without "=manager".
manager_for() {
    case "$1" in
        *debian* | *ubuntu* | *mint* | *kali*) echo apt ;;
        *fedora* | *centos* | *rocky* | *alma* | *rhel* | *ubi*) echo dnf ;;
        *arch* | *manjaro* | *cachyos* | *endeavour*) echo pacman ;;
        *suse*) echo zypper ;;
        *alpine*) echo apk ;;
        *void*) echo xbps ;;
        *) echo "" ;;
    esac
}

# Short names ("debian:stable") are ambiguous to podman, which then asks
# which registry was meant — impossible without a terminal. They mean Docker
# Hub here, so say so. Names that already carry a registry are left alone.
qualify() {
    local ref=$1 head
    head=${ref%%/*}
    if [ "$head" = "$ref" ]; then
        printf 'docker.io/library/%s' "$ref"
        return
    fi
    case "$head" in
        *.* | *:* | localhost) printf '%s' "$ref" ;;
        *) printf 'docker.io/%s' "$ref" ;;
    esac
}

# Does this image use musl as its C library (so a glibc binary cannot run)?
image_is_musl() {
    case "$1" in
        *alpine* | *musl*) return 0 ;;
        *) return 1 ;;
    esac
}

# A package certain to be installed in the family's base image.
installed_pkg_for() {
    case "$1" in
        apk) echo busybox ;;
        xbps) echo base-files ;;
        *) echo bash ;;
    esac
}

# The image's own way to fetch its package index (fresh images ship none).
refresh_cmd_for() {
    case "$1" in
        apt) echo 'apt-get update' ;;
        dnf) echo 'dnf -y makecache' ;;
        pacman) echo 'pacman -Sy --noconfirm' ;;
        zypper) echo 'zypper --non-interactive refresh' ;;
        apk) echo 'apk update' ;;
        xbps) echo 'xbps-install -S' ;;
        *) echo '' ;;
    esac
}

# Managers whose last transaction `yapacc undo` is expected to take back.
undo_expected() {
    case "$1" in
        apt | pacman | dnf | zypper) return 0 ;;
        *) return 1 ;;
    esac
}

# Managers whose log `yapacc timeline` is expected to show the install in.
timeline_expected() {
    case "$1" in
        apt | pacman | dnf | zypper) return 0 ;;
        *) return 1 ;;
    esac
}

probe_present() {
    in_container sh -c "command -v $PROBE_PKG"
    [ "$RC" -eq 0 ]
}

finish_image() {
    cleanup_container
    SUMMARY_ROWS+=("$(printf '%s\t%s\t%s\t%s\t%s\t%s' \
        "$IMAGE" "${MANAGER:-?}" "$IMG_PASS" "$IMG_FAIL" "$IMG_SKIP" "${IMG_FAILED_NAMES:--}")")
}

# ------------------------------------------------------------------ one image

test_image() {
    local spec=$1 pkg refresh name
    IMAGE=${spec%%=*}
    if [ "$spec" != "$IMAGE" ]; then
        MANAGER=${spec#*=}
    else
        MANAGER=$(manager_for "$IMAGE")
    fi
    REF=$(qualify "$IMAGE")
    # busybox on Alpine already provides a `tree` applet: probe with a
    # package whose command exists nowhere else.
    if [ "$MANAGER" = apk ]; then PROBE_PKG=pv; else PROBE_PKG=tree; fi
    SLUG=$(printf '%s' "$IMAGE" | tr -c 'A-Za-z0-9._-' '_')
    IMG_PASS=0
    IMG_FAIL=0
    IMG_SKIP=0
    IMG_FAILED_NAMES=""
    OUT=""
    ERR=""

    printf '\n== %s (expecting %s) ==\n' "$IMAGE" "${MANAGER:-unknown}"

    if [ -z "$MANAGER" ]; then
        fail "image" "no expected package manager is known for this image; write it as IMAGE=MANAGER"
        finish_image
        return
    fi

    if [ "$STATIC" = no ] && image_is_musl "$IMAGE" && [ "$EXPLICIT_IMAGES" -eq 0 ]; then
        skip "image" "a glibc-linked binary cannot run on a musl image"
        finish_image
        return
    fi

    OUT="$LOGDIR/$SLUG.pull.out"
    ERR="$LOGDIR/$SLUG.pull.err"
    with_timeout "$PULL_TIMEOUT" "$ENGINE" pull "$REF" >"$OUT" 2>"$ERR" </dev/null
    RC=$?
    if [ "$RC" -ne 0 ]; then
        fail "pull" "could not pull the image ($(describe_rc "$RC"))"
        finish_image
        return
    fi

    # One container per image, kept alive by a sleep; each check is an
    # `exec` with its own time limit. SELinux labelling is turned off for
    # the container instead of relabelling the binary on the host.
    OUT="$LOGDIR/$SLUG.start.out"
    ERR="$LOGDIR/$SLUG.start.err"
    CID=$(with_timeout 120 "$ENGINE" run -d --rm \
        --user 0 \
        --security-opt label=disable \
        -v "$BIN:$IN_CONTAINER:ro" \
        "$REF" sleep 7200 2>"$ERR" </dev/null)
    RC=$?
    CID=${CID##*$'\n'}
    if [ "$RC" -ne 0 ] || [ -z "$CID" ]; then
        CID=""
        fail "start" "could not start a container ($(describe_rc "$RC"))"
        finish_image
        return
    fi

    pkg=$(installed_pkg_for "$MANAGER")
    refresh=$(refresh_cmd_for "$MANAGER")

    # ---- read side ------------------------------------------------------

    expect "platform" "0" yapacc platform

    name="--json platform names $MANAGER"
    if run_quiet "$name" "0" yapacc --json platform; then
        if json_check "$OUT" manager "$MANAGER"; then
            pass "$name"
        else
            fail "$name" "\"native_pkg_manager\": \"$MANAGER\" is not in the output"
        fi
    fi

    name="--json why $pkg"
    if run_quiet "$name" "0" yapacc --json why "$pkg"; then
        if json_check "$OUT" valid; then
            pass "$name"
        else
            fail "$name" "the output is not JSON"
        fi
    fi

    expect "why <absent> exits 4" "4" yapacc why definitely-not-installed-xyz

    name="--json list --limit 5"
    if run_quiet "$name" "0" yapacc --json list --limit 5; then
        if json_check "$OUT" nonempty-array; then
            pass "$name"
        else
            fail "$name" "the output is not a non-empty JSON array"
        fi
    fi

    # 0: an owner was found. 4: no package owns it (a merged-/usr symlink,
    # or busybox's /bin/sh). Anything else is a defect.
    expect "inspect owns /bin/sh" "0 4" yapacc inspect owns /bin/sh

    # In a pacman image the older pacman code path answers, and it exits 1
    # when no package is an orphan.
    if [ "$MANAGER" = pacman ]; then
        expect "inspect orphans" "0 1" yapacc inspect orphans
    else
        expect "inspect orphans" "0" yapacc inspect orphans
    fi

    expect "check" "0" yapacc check
    expect "doctor" "0 1" yapacc doctor
    expect "clean" "0" yapacc clean

    name="--json briefing --offline"
    if run_quiet "$name" "0" yapacc --json briefing --offline; then
        if json_check "$OUT" valid; then
            pass "$name"
        else
            fail "$name" "the output is not JSON"
        fi
    fi

    # 6: a fresh image may have no package log at all yet.
    expect "timeline" "0 6" yapacc timeline

    # ---- resolver, after fetching the index the image ships without -----

    if ! stopped; then
        if [ -n "$refresh" ]; then
            in_container sh -c "$refresh"
            if [ "$RC" -eq 0 ]; then
                pass "native index refresh ($refresh)"
            else
                fail "native index refresh ($refresh)" "$(describe_rc "$RC") — is the network reachable?"
            fi
        else
            skip "native index refresh" "no refresh command is known for $MANAGER"
        fi
    fi

    expect "get $PROBE_PKG --explain" "0" yapacc get "$PROBE_PKG" --explain

    # ---- write side ------------------------------------------------------

    if ! stopped && probe_present; then
        fail "precondition" "$PROBE_PKG is already installed in this image; the write checks would prove nothing"
        finish_image
        return
    fi

    name="install repo $PROBE_PKG --dryrun"
    if run_quiet "$name" "0" yapacc install repo "$PROBE_PKG" --dryrun; then
        if grep -q 'DRY-RUN' -- "$OUT" "$ERR"; then
            if probe_present; then
                fail "$name" "the dry run installed $PROBE_PKG"
            else
                pass "$name"
            fi
        else
            fail "$name" "the output does not say DRY-RUN"
        fi
    fi

    name="--script install without consent exits 3"
    if run_quiet "$name" "3" yapacc --script install repo "$PROBE_PKG"; then
        if probe_present; then
            fail "$name" "$PROBE_PKG was installed without consent"
        else
            pass "$name"
        fi
    fi

    name="install repo $PROBE_PKG --yes"
    if run_quiet "$name" "0" yapacc install repo "$PROBE_PKG" --yes; then
        if probe_present; then
            pass "$name"
        else
            fail "$name" "exit status 0 but '$PROBE_PKG' is not on PATH"
        fi
    fi

    if timeline_expected "$MANAGER"; then
        name="timeline mentions $PROBE_PKG"
        if run_quiet "$name" "0" yapacc timeline; then
            if grep -qw -- "$PROBE_PKG" "$OUT"; then
                pass "$name"
            else
                fail "$name" "the install is not in the timeline"
            fi
        fi
    elif ! stopped; then
        skip "timeline mentions $PROBE_PKG" "not expected with $MANAGER yet"
    fi

    local use_remove=1
    if undo_expected "$MANAGER" && ! stopped; then
        in_container yapacc undo --dryrun
        if [ "$RC" -eq 0 ]; then
            if probe_present; then
                pass "undo --dryrun"
                use_remove=0
                name="undo --yes"
                if run_quiet "$name" "0" yapacc undo --yes; then
                    if probe_present; then
                        fail "$name" "exit status 0 but $PROBE_PKG is still installed"
                    else
                        pass "$name"
                    fi
                fi
            else
                fail "undo --dryrun" "the dry run removed $PROBE_PKG"
            fi
        elif [ "$RC" -eq 6 ] && [ "$MANAGER" = zypper ]; then
            # Exit 6 is yapacc saying "undo is not available with zypper
            # yet". Reported, not failed; the package is removed below.
            skip "undo --dryrun" "yapacc reports undo as unavailable with zypper (exit 6)"
            skip "undo --yes" "yapacc reports undo as unavailable with zypper (exit 6)"
        else
            fail "undo --dryrun" "$(describe_rc "$RC"), expected 0"
        fi
    fi

    if [ "$use_remove" -eq 1 ] && ! stopped; then
        name="remove repo $PROBE_PKG --yes"
        if run_quiet "$name" "0" yapacc remove repo "$PROBE_PKG" --yes; then
            if probe_present; then
                fail "$name" "exit status 0 but $PROBE_PKG is still installed"
            else
                pass "$name"
            fi
        fi
    fi

    finish_image
}

# ------------------------------------------------------------------------ run

printf 'binary:  %s (static: %s)\n' "$BIN" "$STATIC"
printf 'engine:  %s\n' "$ENGINE"
printf 'json:    checked on the host with %s\n' "$JSON_TOOL"
printf 'timeout: %ss per command, %ss per pull\n' "$TIMEOUT" "$PULL_TIMEOUT"
printf 'output:  %s\n' "$LOGDIR"

for spec in "${IMAGES[@]}"; do
    test_image "$spec"
    if stopped; then
        break
    fi
done

printf '\n== summary ==\n'
{
    printf 'IMAGE\tMANAGER\tPASS\tFAIL\tSKIP\tFAILED CHECKS\n'
    if [ "${#SUMMARY_ROWS[@]}" -gt 0 ]; then
        printf '%s\n' "${SUMMARY_ROWS[@]}"
    fi
} | {
    if command -v column >/dev/null 2>&1; then
        column -t -s "$(printf '\t')"
    else
        cat
    fi
}
printf '\n%s passed, %s failed, %s skipped over %s image(s)\n' \
    "$TOTAL_PASS" "$TOTAL_FAIL" "$TOTAL_SKIP" "${#SUMMARY_ROWS[@]}"

if [ "$STOPPED_EARLY" -eq 1 ]; then
    printf 'stopped at the first failure (--fail-fast); later checks and images were not run\n'
fi

if [ "$TOTAL_FAIL" -gt 0 ]; then
    printf 'captured output kept in %s\n' "$LOGDIR"
    exit 1
fi
if [ "$TOTAL_PASS" -eq 0 ]; then
    printf '%s: nothing was checked\n' "$PROG" >&2
    exit 1
fi
if [ "$LOGDIR_IS_TEMP" -eq 1 ]; then
    rm -rf -- "$LOGDIR"
fi
exit 0
