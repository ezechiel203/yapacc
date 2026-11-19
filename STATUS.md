# yapacc status

Version 0.1.0, the first public release. Last updated: 2026-11-20.

This file says what is true of the code today: what was checked and how,
what is known not to work, and what was never checked. The run-by-run
evidence is in [docs/VERIFICATION.md](docs/VERIFICATION.md). The user's view
of the same facts is the handbook page
[What works where](docs/handbook/support.md).

The short version: yapacc was run for real on booted virtual machines of
eleven systems across seven package managers, by its author and nobody
else. It has not run on real hardware.

## Checks that pass today

| Check | Result |
|---|---|
| `cargo test --workspace --all-targets` | 680 tests pass, 1 ignored |
| `cargo clippy --workspace --all-targets -- -D warnings` | clean on stable 1.98.1 and on nightly, with the default features, `--no-default-features` and `--features alpm` |
| Static core (`--no-default-features`, musl) | 5,929,072 bytes, no shared library; the build fails above 6 MiB |
| `cargo xtask ci` | the four jobs of the CI workflow pass locally: `portable`, `musl`, `arch` (in an Arch container) and `families` (the static binary in containers of seven distributions) |
| `cargo xtask site` | 21 handbook pages; every internal link and anchor is checked |
| `cargo xtask dist` | builds the release archives for x86_64 and aarch64 with the pinned toolchain, checks that they are static and hold no path of the build machine, starts each binary (aarch64 under `qemu-aarch64`) and writes `SHA256SUMS` |

The CI workflow (`ci.yml`) runs on GitHub on every push to `main`, and
passes there. `cargo xtask ci` runs the same commands locally, with one
difference that matters: its `portable` job runs on the developer's
machine, which has pacman when that machine is Arch, while the hosted job
runs on Ubuntu without it. The first hosted runs found three defects the
local run could not see; [docs/VERIFICATION.md](docs/VERIFICATION.md) has
them. The release workflow (`release.yml`) has run once, for the tag
`v0.1.0`: it built the archives, attested them and created the release.
The six binaries it built, for x86_64 and aarch64, are byte for byte the
ones `cargo +1.98.1 xtask dist` builds on a developer's machine. `site.yml`
has never run: the website is published by `site/publish.sh`, which pushes
the built files to the branch `gh-pages`.

## Support, by package manager

"VM" means a booted QEMU/KVM virtual machine built from the distribution's
public cloud image, with real installs, upgrades, removals and undos, an
ordinary user going through `sudo` or `doas`, and prompts answered on a
terminal.

| Manager | Tier | Ran on (VM) | Read commands | Gated changes | `undo` |
|---|---|---|---|---|---|
| pacman | 1 | Arch | yes | yes | yes: cache, then the Arch Linux Archive (signature checked) |
| apt | 1 | Debian 13, Ubuntu 24.04 | yes | yes | yes: repositories, cache, then snapshot.debian.org (hash checked) or Launchpad (not verified) |
| dnf | 1 | Fedora 45 (dnf5), Rocky Linux 9.9 (dnf 4) | yes | yes | yes: `dnf history undo`, after checking the old versions are still offered |
| zypper | 2 | openSUSE Leap 16.1, Tumbleweed | yes; `timeline` needs root | yes | yes: repositories only |
| apk | 2 | Alpine 3.24 | yes; `timeline` with apk-tools 3 | yes | no |
| xbps | 2 | Void (image built from the official container) | yes; no `timeline`, xbps keeps no log | yes | no |
| pkg | 2 | FreeBSD 14.5 and 15.1, UFS and ZFS root | yes | yes | no |
| portage, nix | 3 | | detected only | refused, with the reason | no |

Tier 1 is a main target: everything yapacc does, including `undo`. Tier 2
is supported with some features missing; `yapacc doctor` says which on the
machine at hand. OpenBSD and NetBSD are not supported.

Also run on VMs: `undo` end to end on Arch, Debian 13, Ubuntu 24.04,
Fedora 45, Rocky Linux 9.9, Leap 16.1 and Tumbleweed, including the undo
of a whole system upgrade and kernel upgrades followed by a real reboot;
snapshots before a change through snapper (Leap, Tumbleweed,
Arch), timeshift (Debian 13, Arch) and zfs (FreeBSD); `doas` instead of
`sudo` (Alpine, Debian 13, FreeBSD, Arch); the MCP server over standard
input and output; the interactive interface on a pseudo-terminal.

### The last round, and what came after it

The release candidate (commit `2ea00fa`) was run on nine booted systems on
2026-11-19. That round found defects; they were fixed afterwards, and a
static review of the read side found and fixed more. The fixes were
checked by unit tests and in containers, and then on booted machines
again:

- **Debian 13**, the session the handbook's examples are taken from: the
  read commands, the gate answered yes and no, a refusal under `--script`,
  an upgrade of two packages and its undo, a full upgrade of 24 packages
  with a new kernel, the undo of that whole upgrade (25 of 25 packages
  verified at their old versions), the upgrade again, and a reboot into
  the new kernel.
- **FreeBSD 15.1, Fedora 45, Tumbleweed and Arch**: the read commands with
  no repository metadata on disk (no file created, no packet sent on
  Fedora and Tumbleweed), an install answered at the prompt, its undo with
  the verification line (not on FreeBSD, which has no undo), refusals and
  their exit codes.

Ubuntu 24.04, Rocky Linux 9.9, Leap 16.1, Alpine and Void were not booted
again after the fixes. That last pass found defects of its own, in states
a new machine is often in: an unknown package name on dnf, Arch before its
first `pacman -Syu`, FreeBSD with no catalogue on disk. They were fixed and
checked in containers of Fedora, Rocky Linux, Arch and Tumbleweed, not on a
booted machine. The two FreeBSD fixes were checked by unit tests and with a
stand-in for `pkg`, not on FreeBSD.
[docs/VERIFICATION.md](docs/VERIFICATION.md) lists which fix was checked
how.

## Known limits

- **`undo` restores package versions**, not configuration or data, and only
  the last transaction. It is all or nothing: when one old version cannot
  be found, the whole undo is refused.
- **dnf and zypper have no archive to fall back on.** Once a repository
  drops an old version, the upgrade that replaced it cannot be undone. On
  Tumbleweed that is a day or two.
- **Old packages from snapshot.debian.org** are checked against the hash
  the service records and are not signature-verified. **Launchpad
  downloads are not verified.** The output says which applies.
- **Snapshots** are taken through snapper, timeshift or zfs. Plain btrfs
  is listed only. yapacc never restores a snapshot.
- **`restart`** cannot see interpreters holding replaced scripts,
  statically linked programs or kernel live patches, and says so every
  time. Its "newer kernel" verdict is judged by file date, so an older
  kernel installed later shows up as newer; the line says that too.
- **apt multiarch**: `why NAME:ARCH` is refused as a name, `list` shows a
  package installed for two architectures as two rows that look alike,
  and `timeline` drops the architecture suffix. `undo` handles
  architectures correctly.
- **`timeline`** shows `(time unknown)` for entries of a pacman.log written
  before pacman 5.2, whose stamps have no zone.
- **`configs`** does not know xbps's `FILE.new-VERSION` files.
- **`platform`** does not mark openSUSE MicroOS as image-based, nor LXC,
  systemd-nspawn or WSL as containers. No command depends on those two
  fields.
- **`briefing`** counts a date-like version change (`20250419` to
  `20260101`) as a major version change.
- **`check` on xbps** shows the new version only; xbps does not give the
  old one in the same answer.
- **The build sandbox** (third-party recipes on Arch) has no seccomp
  filter and no resource limits. Recipes that download during `build()`
  need `--build-net`.
- **The ledger** records what yapacc installed outside the package manager.
  It detects tampering only with a configured witness or a kept anchor.
- **Autorepair** of a failed build classifies English tool output only,
  and has one strategy that changes the system (installing missing build
  dependencies), which asks first.
- **The MCP server** speaks protocol revision 2024-11-05. It proposes by
  default; applying needs a root-owned configuration file and an explicit
  confirmation on each call.
- **The `alpm` feature** links libalpm (GPL-3.0-or-later). No release
  artifact enables it; it exists for local builds on Arch.
- **dnf5 creates `~/.cache/libdnf5`** (84 MiB on a fresh Fedora 45) when an
  ordinary user runs `check`, `search` or `briefing`. That is dnf's doing
  and yapacc cannot prevent it; yapacc itself writes nothing on a read
  command, and `clean` lists the directory.
- **An unknown package name** given to `install repo` is a refusal (exit
  code 3) on apt, dnf, zypper and pacman, where the simulation says so,
  and "not found" (exit code 4) on FreeBSD, where the catalogue is asked.
  On FreeBSD with no catalogue on disk, an ordinary user's `--yes` for such
  a name exits 1.
- **A dry run on FreeBSD as root contacts the mirrors**: `pkg install -n`
  run as root updates its catalogue first. As an ordinary user it contacts
  nothing.
- **The Arch lane looks different.** Its plan headings
  (`== Install plan … ==`), the sections of `check` and the table of
  `search` are older than the other lanes and were not brought in line
  with them.
- **zypper prints its own `Continue?` question**, answered for it, inside
  the simulation and again in the real run. yapacc asks once.
- **Errors.** About 250 internal functions still return plain-text errors.
  A failure that has no more specific code exits 1.

## Never verified

- **Real hardware.** Every run was a virtual machine or a container.
- **Other architectures.** Everything ran on x86_64. The aarch64 archive
  is built, and its three binaries were started under `qemu-aarch64`;
  nothing more was run on that architecture.
- **Other users.** Nobody but the author has used it.
- **Derivatives.** Linux Mint, Pop!_OS, Manjaro, EndeavourOS, AlmaLinux,
  RHEL and SLES were not run. `undo`'s archive fallback knows Debian,
  Ubuntu and Arch only.
- **The published release on other machines.** The x86_64 archive was
  installed from the release page in one Debian container, with the
  handbook's commands. Nobody else has downloaded and run it.
- **Restoring a snapshot.** yapacc does not do it, so there is nothing of
  its own to test.
- **Ubuntu 26.04.** The image boots in the harness; yapacc was never run
  on it.
- From the last fixes: the "versions were restored, but the manager
  reported an error" message of `undo` is unit-tested and was not produced
  by a real failing package script; a named snapshot provider that is
  unusable was tested with a stand-in for snapper, not with snapper; an
  archive that does not answer was not met during a lookup (launchpad.net
  did answer with errors during the round, and each time the undo was
  refused with nothing changed); a `sudo` that asks for a password was not
  met after the fixes to the pacman and dnf lanes.

## Where things are

| For | Read |
|---|---|
| using yapacc | the [handbook](docs/handbook/README.md), also at <https://ezechiel203.github.io/yapacc/> |
| the evidence behind this page | [docs/VERIFICATION.md](docs/VERIFICATION.md) |
| what changed in each release | [CHANGELOG.md](CHANGELOG.md) |
| how the code is laid out | [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) |
| the threat model | [docs/SECURITY.md](docs/SECURITY.md); reporting a vulnerability: [SECURITY.md](SECURITY.md) |
| scope and what is planned | [docs/STRATEGY.md](docs/STRATEGY.md), [docs/ROADMAP.md](docs/ROADMAP.md) |
