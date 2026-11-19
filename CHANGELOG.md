# Changelog

All notable changes to yapacc are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project
follows [Semantic Versioning](https://semver.org/). While the version is
0.x, a minor release may change behaviour; the changelog says when it does.

## [0.1.0] - 2026-11-19

The first public release.

### What it does

- **Reports that change nothing:** `why`, `timeline` (with `--since-boot`
  and `--html`), `briefing`, `clean`, `restart`, `configs`, `doctor`,
  `platform`, and `list`, `info`, `search`, `check`, `inspect`.
- **Gated changes through the native package manager:** `install repo`,
  `remove repo`, `remove orphans`, `upgrade`, `refresh` and `get`. Each
  shows the plan, the native manager's own simulation and the exact
  command, then asks. `--yes` consents, `--dryrun` only plans, `--script`
  never prompts, and a refusal exits 3.
- **`undo`** of the last transaction on pacman, apt, dnf and zypper, with
  old versions from the cache, the repositories or the distribution's
  archive (Arch Linux Archive, snapshot.debian.org, Launchpad). All or
  nothing, with guards for the running kernel and for packages something
  else needs.
- **Seven package managers:** pacman, apt, dnf (4 and 5), zypper, apk, xbps
  and FreeBSD's pkg. Portage and Nix are detected and changes are refused
  with the reason.
- **Snapshots** through snapper, timeshift or zfs: `snapshot list`,
  `snapshot create`, and optionally a snapshot before every change
  (`[safety] snapshot`).
- **Privileges** through `sudo` or `doas`, never prompting where nobody
  could answer.
- **Three programs:** `yapacc`; `yapacc-tui`, an interactive interface;
  `yapacc-mcp`, an MCP server through which an AI assistant can read and
  propose, and cannot apply unless the configuration allows it.
- **For scripts:** `--json` on the read commands and seven exit codes.
- **Other sources,** as specialist commands: flatpak, AppImages, isolated
  installs of packages made for another distribution, and on Arch the AUR
  with a review step and a sandboxed build.
- **Documentation:** a handbook, built-in topics (`yapacc docs`), a manual
  page, and a website generated from the handbook.

### Verified on

Booted virtual machines of Arch Linux, Debian 13, Ubuntu 24.04, Fedora 45,
Rocky Linux 9.9, openSUSE Leap 16.1 and Tumbleweed, Alpine 3.24, Void Linux,
FreeBSD 14.5 and FreeBSD 15.1. See [STATUS.md](STATUS.md) and
[docs/VERIFICATION.md](docs/VERIFICATION.md) for what was run and what was
not.

### Known limits

- Not run on real hardware, on architectures other than x86_64, or on
  derivative distributions.
- `undo` restores package versions, not configuration or data. It needs
  the old versions to be available: dnf and zypper have no archive to fall
  back on.
- No `undo` on apk, xbps and pkg; no `timeline` on xbps.
- OpenBSD and NetBSD are not supported.

[0.1.0]: https://github.com/ezechiel203/yapacc/releases/tag/v0.1.0
