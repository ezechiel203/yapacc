# yapacc strategy

Status: rules and scope decided 2026-11-15; text brought up to date
2026-11-19. This file wins on rules and scope. For the state of the code,
[STATUS.md](../STATUS.md) wins, and the dated record of what was run where
is [VERIFICATION.md](VERIFICATION.md).

Companion documents: [ARCHITECTURE.md](ARCHITECTURE.md) (how it is built),
[SECURITY.md](SECURITY.md) (what each protection does and does not do),
[ROADMAP.md](ROADMAP.md) (order of work and exit criteria),
[RESEARCH.md](RESEARCH.md) (the field, checked against sources). How to use
yapacc is in the [handbook](handbook/README.md).

## 1. What yapacc is

yapacc is an **optional companion to the package manager you already have**.
It works beside pacman, apt, dnf, zypper, apk, xbps and FreeBSD pkg, and
answers four questions:

1. **Where should this come from?** One resolver (`get`) over the sources
   present on the machine, with the decision shown before anything happens.
   On Arch: repository, AUR, flatpak, recipe. Elsewhere today: the
   repository and flatpak.
2. **Why is this here, and where did it come from?** `yapacc why`, for any
   package, whether or not yapacc installed it.
3. **What changed?** `yapacc timeline`, read from the native manager's own
   log (six of the seven managers; xbps keeps no log).
4. **Can I take that back?** Gated changes that the native log records, and
   an undo of the last one (pacman, apt, dnf, zypper) from the package
   cache, the repositories or the distribution's archive. Snapshots can be
   taken before a change; yapacc does not restore them.

Used daily it is a front end with a CLI and a TUI. Used once a month it is a
diagnostic tool. Removed, it leaves the system's packages as they were; the
files it may have created under your home directory (downloads for `undo`,
its ledger, kegs) are listed in ARCHITECTURE.md §5.

It is not a package manager replacement, a package format, a distribution,
or a system-configuration tool.

### Scope boundary

yapacc is about packages. Machine-level features (inventories of services,
boot entries, mounts and accounts; health checks; recovery and rescue) and
the management of several machines are out of scope. They would be separate
programs, and none is part of this repository. Features of that kind that
were once proposed for yapacc were cut from it for this reason.

## 2. The constitution

Ten rules. A change that breaks one is a bug regardless of what it adds. The
right-hand column says how each rule is checked today, and where the check
is only a review item it says so.

| # | Rule | How it is enforced today |
|---|---|---|
| C1 | **The native database is the truth.** yapacc never keeps its own copy of what the native manager knows. | `why`, `list`, `info` query the native manager live; the code has no cache of native state |
| C2 | **Works alongside.** Using the native manager or flatpak directly, in any mix, must never confuse yapacc. | follows from C1; the container matrix runs `why` on packages yapacc never touched |
| C3 | **Read-only on first contact.** Every read command works on a machine that has never run yapacc and writes nothing. | test `reads_never_create_a_ledger` (covers the ledger only); a system-call trace of the read commands on Arch on 2026-11-19 showed no file created by yapacc. Known exception, not yapacc's own write: on Arch `briefing` runs `checkupdates` when it is installed, and that tool keeps a copy of the sync databases under `/tmp` |
| C4 | **The uninstall test.** Deleting every yapacc file leaves every installed program working and removable with native tools or `rm`. Launchers never call yapacc; a keg describes itself. | design rule for every lane and a review item; each keg carries `YAPACC-MANIFEST.txt`; the package install, upgrade and removal test left no owned file on Debian testing and Fedora 45 (containers, 2026-11-16) |
| C5 | **Nothing resident, nothing hooked, unless asked.** No daemon, timer, package-manager hook or shell alias is installed by default. Each one is a separate opt-in with a one-command removal. | `monitor daemon-install` is the only installer and is gated; no packaging recipe installs a unit or a hook |
| C6 | **Never edits the native manager's configuration**, never shadows its commands. | no code path writes to `/etc/pacman*`, `/etc/apt`, `/etc/dnf`, `/etc/yum.repos.d`; review item, no mechanical check |
| C7 | **Every change is gated, planned, and recorded by the native manager's own log.** A plain run asks; `--yes` consents; `--dryrun` only plans. Consent given for one thing never covers another. A refusal exits 3 and changes nothing. | gate tests; autorepair asks before installing a dependency; the review gates of AUR and recipe builds exit 3 on "no" |
| C8 | **Each binary is optional.** The core is useful alone; a missing component is a clear message (exit 6), never a crash. | test `missing_component_is_a_clear_error_not_a_crash` |
| C9 | **No axis is inferred from another.** Distribution, package manager, init, boot chain, log system, snapshots and libc are each detected from evidence; an unknown value disables the features that need it and says so. | `platform` probe; see §4 |
| C10 | **Guarantees are stated with their limits.** Every protection states what it does not protect against, in the output where it matters, not only in a document. | `ledger verify` prints "tamper evidence: NONE" when unanchored; `undo` says which downloads are not signature-checked; SECURITY.md |

Being light is part of the contract, so it has a budget:

| Budget | Limit | Measured |
|---|---|---|
| Core binary, static musl build, no optional features | at most 6 MiB (checked by the CI `musl` job) | 5.2 MB (5,212,208 bytes) on 2026-11-19 |
| Core runtime dependencies | none in the static build; libc in a glibc build. libalpm only in a local build with the `alpm` feature | static-pie with no dynamic dependencies (2026-11-18) |
| Read command start-up | at most 50 ms plus the native tool's own time | author's Arch machine, 1,412 packages, one warm run, 2026-11-19: `platform` and `timeline` under 30 ms; `doctor` 0.4 s; `list` 0.65 s against 29 ms for `pacman -Q`. `list` is over budget |
| Files written by a read command | 0 | 0 by yapacc (trace on Arch, see C3) |
| Background processes | 0 | 0 |

## 3. What makes it worth installing

A harsh reading of the field ([RESEARCH.md](RESEARCH.md), checked against
sources on 2026-11-15) says the obvious ideas are taken: unified front ends
exist (pamac, bauh, rhino-pkg, mpm), snapshot rollback on Arch is solved by
snapper, grub-btrfs and limine-snapper-sync and is the default on CachyOS
and Garuda, `dnf` and `nala` already undo transactions, `rua` already builds
AUR packages inside bubblewrap, and a small project (paclens) already
describes itself as "not a package manager" with a `why` inspector.

What that survey did not find in one tool, and where yapacc concentrates:

1. **The same questions answered across package managers.** `why` spans
   native packages, flatpak and yapacc's own installs on seven managers.
   `timeline` and `undo` cover native packages, with per-manager limits
   listed in STATUS.md (undo on four managers, a timeline on six). The tools
   surveyed are per-manager or per-distribution.
2. **An explained source decision.** None of the front ends surveyed shows
   why it picked a source. `get --explain` prints the decision table and
   stops; the distribution's own package wins when it exists.
3. **Confined community builds on Arch by default.** Builds run in a
   bubblewrap sandbox with the network open only while fetching, and yapacc
   asks before it installs any dependency it inferred. Malicious AUR
   packages were reported in July 2025. Of the helpers surveyed, rua builds
   inside bubblewrap (with a seccomp filter that yapacc's sandbox lacks);
   the others confine a build only when asked, or not at all.
4. **An AI interface that cannot approve itself.** The Arch MCP server
   surveyed installs with `--noconfirm` and suggests passwordless sudo.
   yapacc's server only plans, unless the owner of the machine sets
   `[mcp] apply = "allow"` in the configuration file.

Everything else is supporting cast or cut.

## 4. Breadth: every combination, by construction

Requirement: yapacc is agnostic to distribution, package manager, init
system, boot manager, bootloader and log system, and must not special-case
any combination of them.

That is only achievable one way: each axis is an independent provider behind
a small interface, detected on its own, and no provider may assume another
(C9). Then N+M+K providers cover N×M×K machines without anyone enumerating
them. What this costs:

- A feature that needs an axis works only where that axis has a provider.
  Until it does, the feature is unavailable *with a reason*, never guessed.
- Providers are tested one by one, plus a set of representative whole
  machines. Not every combination is tested; the claim is "no combination is
  special-cased", not "every combination was tried".
- Read commands can be tested in containers. Anything that involves a
  reboot, an init system, a snapshot or a password prompt needs a booted
  machine, which is why the evidence in VERIFICATION.md comes from virtual
  machines.

### Support tiers

These are the tiers the code reports (`yapacc platform`,
`crates/octo-head/src/platform.rs`). A tier states intent, not evidence.

| Tier, as `platform` prints it | Native package managers | What the code does there |
|---|---|---|
| 1 — beta target (full support intended) | pacman, apt, dnf | read commands, gated install, remove and upgrade, `timeline`, `undo` |
| 2 — recognised (not a beta target; `yapacc doctor` says what works here) | zypper, apk, xbps, pkg (FreeBSD) | read commands and gated install, remove and upgrade. `undo` on zypper only; no `timeline` on xbps |
| 2, same line | portage | detected; no read backend; changes are refused with the reason |
| 3 — detected only | nix | detected; no read backend; changes are refused with the reason |
| unknown platform | anything else | read commands work where data exists |

An image-based system (ostree) is reported by `platform` as such; nothing
else is promised there. OpenBSD and NetBSD are unsupported.

Evidence, 2026-11-17 and 2026-11-18, on booted QEMU/KVM virtual machines:
Arch, Debian 13, Ubuntu 24.04, Fedora 45 (dnf5), Rocky Linux 9.9 (dnf 4),
openSUSE Leap 16.1 and Tumbleweed, Alpine 3.24, Void, FreeBSD 14.5 and 15.1
(UFS and ZFS root). Not verified: real hardware, any architecture other than
x86_64, derivatives (Mint, Manjaro, AlmaLinux, SLES), and use by anyone but
the author. What ran on each system is in
[handbook/support.md](handbook/support.md) and VERIFICATION.md.

FreeBSD `pkg` was added on 2026-11-16, replacing an earlier "BSD out of
scope". The tiers will be cut again once someone other than the author has
run the tier-2 backends. A web interface stays cut; the static
`timeline --html` page is the alternative.

### Providers today

| Axis | What the probe detects | What uses it |
|---|---|---|
| Native packages | 9 managers: pacman, apt, dnf, zypper, xbps, portage, apk, nix, pkg | 7 have read and gated-change backends (all but portage and nix) |
| Package logs | pacman.log, dpkg.log, apt history.log, dnf logs and history database, zypp history, emerge.log, apk.log, FreeBSD `pkg` lines in `/var/log/messages` | `timeline` (six managers), `undo` (four) |
| Snapshots | snapper, timeshift, btrfs, zfs | `snapshot list`, `snapshot create` and the snapshot before a change: snapper, timeshift, zfs. Plain btrfs can only be listed |
| Init | systemd, OpenRC, runit, s6, dinit, BSD rc.d, a sysvinit-compatible `init`, or none | shown by `platform`; `restart` names the systemd unit a process belongs to; nothing restarts a service |
| Firmware, boot managers, bootloaders | UEFI or BIOS; systemd-boot, rEFInd; GRUB, Limine, syslinux, unified kernel images, the BSD loader | shown by `platform`; no feature acts on it |
| System logs | journald, syslog files, socklog | shown by `platform`; `timeline --since-boot` takes the boot time from the kernel, not from a log |

The per-manager matrix, with how each cell was verified, is in STATUS.md.

### Sources ("lanes") per platform

Target lanes. Today `get` resolves repository, AUR, flatpak and recipe on
Arch, and the repository and flatpak elsewhere. The flatpak, AppImage and
container commands have not been run off Arch (STATUS.md).

| Lane | Arch family | Debian family | Red Hat family |
|---|---|---|---|
| Distribution repository | pacman | apt | dnf |
| Community builds | AUR (sandboxed) | none (a PPA is a repository; parked) | none (COPR is a repository; parked) |
| Flatpak | yes | target | target |
| Snap | not built | not built | not built |
| AppImage / single-file tools | yes | target | target |
| Other distribution's packages | container (distrobox) | target | target |

**Where things are installed** (decided 2026-11-15). Packages made for this
distribution go through the native manager: the official repositories, and
AUR builds on the Arch family. Everything that was *not* packaged for this
distribution (a build from upstream source, another distribution's package,
an AppImage) becomes a **keg**: an isolated prefix with attributed launcher
names, never registered in the native database. See
[ISOLATED_INSTALLS.md](ISOLATED_INSTALLS.md).

## 5. Cut, parked, kept

Kept in yapacc (the whole list): platform probe; `why`; `timeline`;
`briefing`; `undo`; the `clean`, `restart` and `configs` reports; `names`;
`snapshot` (list, create, and before a change); the resolver with its
decision table (`get`); native, flatpak and AppImage lanes; sandboxed AUR
builds; plans as data and gates; the install ledger with witnesses;
`doctor`; plugin adapters; TUI; MCP server.

Out of scope (see §1): inventories of services, boot entries, mounts,
accounts, timers, kernels, devices and network; health checks and scoring;
maintenance windows; boot entries and firmware; key and disk-encryption
management; rescue images; anything that manages more than one machine.

Parked inside yapacc until evidence asks for them (entry criterion in
brackets):

- **Shared dependency store with patchelf.** A sound design, but a second
  product (a small Nix). Foreign-distribution packages go to containers
  instead. [three real packages that cannot run in a container and can with
  the store]
- **Recipe generation, build factories, Homebrew formula lane.** Arch-only
  mechanics dressed as a general feature; AUR already covers the need on
  Arch. The code exists and the Homebrew lane is off by default. If Homebrew
  is ever consumed, through its JSON API, never by parsing Ruby. [a tier-1
  non-Arch user need the other lanes cannot meet]
- **Autorepair beyond diagnosis.** Keep the failure classifier and the
  consented dependency install; park strategy growth and the fix repository.
  [an outside contributor with a real fix to share]
- **deb/rpm extraction into prefixes.** Keep for inspection; do not present
  it as a way to *run* foreign software.
- **A frozen machine interface.** Scripts and other programs need `--json`
  output they can rely on. Version it and contract-test it first; freeze it
  at 1.0, not before. Today's `--json` output is not stable enough to
  promise.

Cut (do not build):

- Wine, Darling, game-store lanes as anything but third-party plugins. (The
  Homebrew lane's code still carries a Darling path from before this
  decision.)
- "Ask mode" natural-language parsing: an MCP client does this better;
  yapacc provides the facts.
- Web UI, D-Bus service, tray icon, UX profiles, task wizard.
- Taxonomy growth as hand-written Rust tables: the tree stays (AppStream
  covers only about 8% of Arch packages, almost all desktop applications, so
  it cannot replace it; RESEARCH.md §6), but it is to move to a data file
  and stops growing in code.
- Any claim of "guaranteed" rollback, and any claim that the ledger is
  tamper-proof.

## 6. Names

**yapacc**: *Yet Another PACkage Companion* (decided 2026-11-15). The
earlier expansion "Yet Another PACman Clone" is retired: the program neither
clones nor replaces pacman and is not Arch-specific.

**octo-head**: the shared library crate; see ARCHITECTURE.md §3.
