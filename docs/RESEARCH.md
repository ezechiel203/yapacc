# Field research (checked 2026-11-15)

Every claim below about another project was checked against a primary source
(project repositories, man pages, package data) on 2026-11-15. None was
checked again since. The statements about yapacc itself were brought up to
date on 2026-11-19.

Limits of the method: ArchWiki blocked automated fetching, so Arch facts
come from package data, man pages and upstream repositories. "Not found"
means the search found nothing, not proof of absence. Maintenance dates are
from the hosting sites on the day of the check.

## 1. Multi-source front ends

| Tool | Sources | Why / provenance | Undo | Explained source choice | State (Nov 2026) |
|---|---|---|---|---|---|
| pamac | alpm, AUR, flatpak, snap | no | no | source label only | active |
| bauh | AUR, deb, flatpak, snap, AppImage, web | no | downgrade + Timeshift hook | label only | stale (last release Jan 2024) |
| topgrade | upgrades everything | no | no | no | active |
| rhino-pkg | apt, snap, flatpak, Pacstall | no | no | user picks from merged results | active |
| mpm (meta-package-manager) | many managers | no; dump/restore, SBOM | no | no | active |
| pkgx, soar, AM/AppMan | own catalogues / portable binaries | no | AM: per-app snapshots | no | active |
| UPT | alias layer over the native manager | no | no | no | stale (Oct 2024) |
| nala | apt | no | `history undo/redo` | no | active |
| aura | pacman + AUR | no | save / restore package set | no | slow |
| paru, yay | pacman + AUR | no | no | repo-vs-AUR menu | active |
| pacseek, Pacsea | repo + AUR TUIs | Pacsea: preflight modal | Pacsea: via `downgrade` | no | active |
| **paclens** | pacman, AUR (via paru), flatpak | **`why` inspector with confidence labels** | no | no | new (2026), tiny |
| Bedrock `pmm` | several distributions' managers | no | no | configurable priority list, not shown as a table | active |
| metapac, pacdef | declarative multi-backend | no | no | no | active |

Conclusions: unified front ends are common. None of the tools in the table
combines provenance, undo and an explained source decision. No tool was
found with a visible decision table across repository, community builds,
flatpak and containers. paclens is the closest occupant of the "companion,
not a package manager" niche; on the day of the check it had no undo,
sandbox or MCP server, and was a new and small project.

Sources: github.com/manjaro/pamac · github.com/vinifmor/bauh ·
github.com/kdeldycke/meta-package-manager · github.com/sigoden/upt ·
github.com/fosskers/aura · github.com/Firstp1ck/Pacsea ·
github.com/plasmaDestroyer/paclens · github.com/ripytide/metapac ·
bedrocklinux-userland `bedrock.conf`

## 2. Rollback and "generations"

Already solved:

- Bootable btrfs snapshots on Arch: grub-btrfs (active); limine-snapper-sync
  (active, with `limine-snapper-restore`). snap-pac is still the usual hook
  but has had no commit since January 2022.
- Distribution defaults: CachyOS (Limine + automatic snapshots), Garuda
  (snapper + snap-pac + grub-btrfs), openSUSE (boot a snapshot, then
  `snapper rollback`).
- Package-set undo: dnf5 `history undo/rollback/redo`; nala `history
  undo/redo`; aura save/restore; `downgrade`.
- Image-based: ABRoot active; AshOS dead (May 2024).

Not found anywhere (inference from the above):

- one addressable id spanning native packages, AUR, flatpak and containers;
- user-level flatpaks and `/home` included in a rollback;
- bootable rollback on ext4 or xfs;
- any native transaction undo for pacman;
- dnf and nala undo fail once the old versions leave the repositories
  (documented by Red Hat).

yapacc's own undo has the same limit on dnf, where it runs dnf's
`history undo`, and on zypper, where it has no archive to fall back on. On
pacman and apt it falls back to the distribution's archive (Arch Linux
Archive, snapshot.debian.org, Launchpad).

Sources: gitlab.com/Zesko/limine-snapper-sync · github.com/Antynea/grub-btrfs
· github.com/wesbarnett/snap-pac · dnf5 `history(8)` · nala `history(8)` ·
doc.opensuse.org snapper

## 3. Change timelines

pacman-only log viewers exist (paclog, vlp, pacmanlogviewer, logkonsult).
etckeeper commits `/etc` around package transactions through pacman hooks.
`flatpak history` reads the systemd journal. mpm reports changes only for
commands run through it. Nothing was found that merges native package logs,
flatpak history, `/etc` changes and boots or snapshots.

## 4. Sandboxed builds

- paru: chroot is opt-in and requires a local repository.
- aurutils: `aur build -c` (systemd-nspawn) is opt-in.
- devtools `makechrootpkg`: fetches on the host, builds in nspawn; no
  private network flag seen in the script.
- pikaur, yay: none.
- **rua**: bubblewrap with unshared user/ipc/pid/uts/cgroup namespaces,
  read-only root, tmpfs over home, seccomp, read-only public keyring, an
  `--offline` option. Documented pitfalls: a fakeroot bug extracting
  root-owned archives; ccache and X11 break; caches re-download.
- The `makepkg -o` then `makepkg -e` split is documented in `makepkg(8)` and
  used by yay and paru for `pkgver()` handling, not for network isolation.
- Context: malicious AUR packages carrying a remote-access trojan were
  reported in July 2025.

Conclusion: bubblewrap builds are prior art (rua). Helpers still default to
no isolation, so safe-by-default is a real position; rua's seccomp filter is
ahead of yapacc's sandbox.

## 5. MCP

- Tool annotations (`readOnlyHint`, `destructiveHint`, …) since revision
  2025-03-26; clients must treat them as untrusted unless the server is.
- Elicitation since 2025-06-18. The 2026-07-28 revision made the protocol
  stateless: servers return an `input_required` result and the client
  retries with the answers. The spec still places approval on the host.
- Client support for elicitation is partial (roughly half of surveyed
  clients in October 2026).
- Existing package-manager servers: an Arch server that sets annotations but
  installs with `sudo pacman --noconfirm` and suggests passwordless sudo; an
  official `brew mcp-server` whose gating could not be determined.
- A cautionary case: one database vendor's server runs the tool without
  confirmation when the client lacks elicitation.

Conclusion: a server-side gate that does not depend on client features is
the right default; elicitation is an additional path, never the only one.

## 6. Facts used elsewhere in the docs

- Homebrew has a JSON API per formula exposing source URL, checksum
  (`urls.stable.checksum`), dependencies and bottle platforms including
  Linux. No stated terms or stability guarantee.
- AppStream on Arch: package `archlinux-appstream-data`, under
  `/usr/share/swcatalog/`. About 1,300 distinct packages of about 15,700
  (≈8%), nearly all desktop applications; 12 console applications. Debian's
  debtags tool was removed from unstable in April 2025; `Tag:` fields remain
  on about a third of packages.
- distrobox exports work by copying desktop files or writing wrapper
  scripts; listing scans those files. No host-side registry exists.
- Rust `File::lock` / `try_lock` are stable since 1.89.
- Ubuntu 24.04 ships libalpm 13; the `alpm` crate v5 requires 16. The
  libalpm-linked build cannot be compiled there.
- The `alpm` and `alpm-sys` crates declare the licence GPL-3.0 in their
  manifests (alpm 5.0.2, alpm-sys 5.0.1; read on 2026-11-19).
- GNU cpio extracts absolute paths and `..` by default;
  `--no-absolute-filenames` is required.

## 7. What this means for positioning

Not defensible: "nobody has generations", "first AI-native package tool",
"nobody else answers why", "the only unified front end".

Defensible, as far as this survey goes: `why`, a timeline and an undo behind
one command set on several package managers (today `why` on seven, a
timeline on six, an undo on four: apt, dnf, pacman and zypper, each with its
own sources and limits); an explained, policy-driven source decision;
community builds that are confined by default; an AI interface where the
model cannot grant its own approval. None of these is a claim that no other
tool does it: "not found" above means the search found nothing. See
[STRATEGY.md](STRATEGY.md) §3.
