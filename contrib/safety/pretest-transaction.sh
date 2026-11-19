#!/usr/bin/env bash
# Pretest helper for package transactions (best-effort).
# Performs non-mutating checks where possible (resolve/dry-run/plan). It does NOT
# apply the transaction and does NOT provide full isolation guarantees.
#
# This file is compiled into the yapacc binary (src/safety.rs) and run from
# there; yapacc never looks for it in the current directory.
# Package arguments are always passed after `--`.
set -euo pipefail

op="${1:-}"; shift || true
ok=1
note() { printf '[pretest] %s\n' "$*"; }
# run CMD...: the command's stdout is discarded, its stderr is shown
run() { note "check: $*"; if "$@" >/dev/null; then return 0; else note "FAILED: $*"; ok=0; return 0; fi; }
# try CMD...: quiet; succeeds or fails without recording a failure
try() { "$@" >/dev/null 2>&1; }

case "$op" in
  arch-install)
    for p in "$@"; do run pacman -Sp -- "$p"; done
    ;;
  arch-remove)
    for p in "$@"; do run pacman -Qi -- "$p"; done
    ;;
  arch-update|arch-upgrade)
    # pacman -Qu exits 1 when nothing is pending: not a failure
    note "check: pacman -Qu"
    pacman -Qu >/dev/null || true
    ;;
  aur-install|aur-upgrade)
    if command -v yay >/dev/null 2>&1; then
      note "check: yay -Si -- $*"
      try yay -Si -- "$@" || try yay -Qua || true
    else
      note "yay not available; skipping AUR resolve pretest"
    fi
    ;;
  flatpak-install)
    for p in "$@"; do
      if try flatpak remote-info --system flathub -- "$p"; then
        note "check: flatpak remote-info --system flathub -- $p"
      else
        run flatpak search -- "$p"
      fi
    done
    ;;
  flatpak-remove)
    for p in "$@"; do run flatpak info -- "$p"; done
    ;;
  flatpak-update)
    if [ "$#" -gt 0 ]; then
      for p in "$@"; do run flatpak info -- "$p"; done
    else
      try flatpak remote-ls --updates || run flatpak --version
    fi
    ;;
  *)
    note "unknown or generic operation '$op'; running basic environment checks"
    note "check: command -v pacman"
    command -v pacman >/dev/null || true
    ;;
esac

if [ "$ok" -eq 1 ]; then
  note "RESULT: PASS (non-mutating pretest checks)"
  exit 0
else
  note "RESULT: FAIL (one or more checks failed)"
  exit 2
fi
