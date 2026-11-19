# yapacc roadmap

Status: 2026-11-19. Milestones are ordered and each has exit criteria that
can be checked. Scope rules are in [STRATEGY.md](STRATEGY.md); the state of
the code is in [STATUS.md](../STATUS.md); the dated record of what was run
where is [VERIFICATION.md](VERIFICATION.md).

A ticked item exists in the code. The date is the day it landed, and the
text after it says where it was run. A tick is not a statement that an
outside user has tried it: nobody but the author has.

## Status, 2026-11-19

Read commands and gated install, remove and upgrade exist for seven package
managers (pacman, apt, dnf, zypper, apk, xbps, FreeBSD pkg); `timeline` for
six of them; `undo` for four (pacman, apt, dnf, zypper).

On 2026-11-17 and 2026-11-18 they were run on booted QEMU/KVM virtual
machines of Arch, Debian 13, Ubuntu 24.04, Fedora 45 (dnf5), Rocky Linux 9.9
(dnf 4), openSUSE Leap 16.1 and Tumbleweed, Alpine 3.24, Void, FreeBSD 14.5
and 15.1 (UFS and ZFS root). Not verified: real hardware, any architecture
other than x86_64, derivatives (Mint, Manjaro, AlmaLinux, SLES), use by
anyone but the author. OpenBSD and NetBSD are unsupported.

`cargo xtask ci` runs the jobs of the CI workflow locally, and they passed
on 2026-11-18. The GitHub runner has not run them yet. The packaging recipes
were built and installed in clean containers on 2026-11-16; nothing is
published.

The order below was not kept in November 2026: most of M1 and much of M2
landed while M0 still had open items. No milestone's exit criterion is met,
because each needs either CI on the runner or use by someone else.

## M0 — Trustworthy (in progress)

The code does what the documents say.

Done:

- [x] 2026-11-15: build sandbox that confines; two-phase network;
      `sandbox = "require"` by default
- [x] 2026-11-15: autorepair asks before installing anything
- [x] 2026-11-15: ledger with a kernel file lock, crash recovery from the
      journal, witnesses, and `verify` wording that states its limits
- [x] 2026-11-15: `yapacc why` as a command (and behind the MCP tool)
- [x] 2026-11-15: no shell interpolation of data anywhere; RPM fallback and
      container creation fixed
- [x] 2026-11-15: MCP cannot approve its own installs; arguments validated
- [x] 2026-11-15: library plus three binaries; components optional; typed
      exit codes
- [x] 2026-11-15: core builds and runs without libalpm (Ubuntu, Fedora,
      Debian and Arch containers)
- [x] 2026-11-15: platform probe for every axis
- [x] 2026-11-15: malformed configuration is an error
- [x] 2026-11-16: an unverified repository is refused in the fetch layer,
      before anything is downloaded
- [x] 2026-11-16: each question to an adapter has a 5 s limit
- [x] 2026-11-18: `cargo xtask ci` runs the CI workflow's jobs locally; a
      full run passed
- [x] 2026-11-19: the default build no longer links libalpm
      (`default = ["tui"]`); `alpm` is an optional feature for local builds,
      because the `alpm` crate is GPL-3.0 and yapacc is GPL-2.0-only
- [x] 2026-11-19: every gate refusal exits 3, including the PKGBUILD review
      gate and the recipe review gate
- [x] 2026-11-19: a failed native simulation refuses the change (exit 3) on
      apt, pacman, zypper and pkg
- [x] 2026-11-19: `undo --dryrun` downloads nothing; downloads happen after
      consent
- [x] 2026-11-19: user handbook in `docs/handbook/`, rendered as the website
      by `cargo xtask site`

Open:

- [ ] CI on the GitHub runner: it has never run a job
- [ ] `list` takes 0.65 s for 1,412 packages on the author's machine
      (2026-11-19); the budget is the native tool's time plus 50 ms
- [ ] Mutating paths exercised by CI, not by hand.
      `contrib/test/families.sh` installs, refuses, applies and undoes in
      eight container images when run locally; the workflow's `families`
      job runs read commands only. An AUR install as root with
      `--build-user` is in neither
- [ ] seccomp filter and resource limits for the build sandbox
- [ ] Remaining `Result<_, String>` call chains (about 180 functions) typed
      where the exit code matters
- [ ] Built-in `docs` topics and the man page checked against the command
      set. The fact-check of 2026-11-19 found removed verb names and
      configuration keys in both

## M1 — Read side

Everything yapacc can *tell* you works the same on apt, dnf and pacman, and
on the tier-2 managers where they have the data.

- [x] 2026-11-16: `NativeBackend` trait in `crates/octo-head`, with pacman,
      apt, dnf, zypper, apk, xbps and pkg behind it: list, info, search,
      owner-of, files, orphans, pending updates. Run on VMs for pacman,
      apt, apk, xbps and pkg; for dnf and zypper the read commands are
      recorded in containers only (the VM passes do not itemise them)
- [x] 2026-11-16: package log parsers: pacman.log, dpkg.log, dnf history,
      zypp history, FreeBSD `/var/log/messages`; 2026-11-17: apk.log
- [x] 2026-11-16: `yapacc timeline` from the native logs and yapacc's own
      ledger, with `--since-boot` and `--html`. It does not read flatpak
      history. No timeline on xbps, which keeps no log
- [x] 2026-11-16: `yapacc doctor`
- [x] 2026-11-16: static musl build with a size check (6 MiB). Built and
      checked locally; the release workflow has never run, so no file is
      published
- [ ] Workspace split. Done: the shared crate `octo-head` holds errors,
      utilities, the platform probe, the native backends and the package
      log parsers (2026-11-15 and 16). Not done: ledger chain, prompts,
      version comparison and taxonomy in the shared crate; one crate per
      binary
- [ ] Fixtures captured from the real tools for every parser. They exist
      for pacman, apt (Debian, Ubuntu) and dnf (Fedora); not for a
      RHEL-like, zypper, apk, xbps or pkg
- [ ] CI matrix on the runner. The workflow has a `families` job over seven
      images; it passes locally and has never run on GitHub
- [ ] `--json` shapes versioned and replayed from fixtures in CI

Exit: the same test script produces equivalent answers on five images.
`families.sh` gives the expected answers locally on eight images
(2026-11-18), none of them a RHEL-like. It has not run on the GitHub runner.

## M2 — Write side

- [x] 2026-11-16: plan, native simulation, gate, apply for install, remove
      and upgrade on seven managers, through the native tool's own
      simulation where it has one. The native log is the record. Run on VMs
      of all eleven systems (2026-11-17 and 18), as a normal user through
      `sudo` (and through `doas` on Alpine, Debian, FreeBSD and Arch) and as
      root
- [x] 2026-11-16: `yapacc undo` on pacman, apt, dnf and zypper, with a
      refusal (exit 3) when an old version is gone. Run on VMs for apt, dnf
      and zypper, including whole upgrades with a kernel; for pacman in a
      container only. There is no undo on apk, xbps or pkg
- [x] 2026-11-16: config-merge report (`configs`: `.pacnew`, `.pacsave`,
      `.dpkg-dist`, `.dpkg-new`, `.dpkg-old`, `.ucf-dist`, `.rpmnew`,
      `.rpmsave`, `.apk-new`)
- [x] 2026-11-16: restart report (`restart`). It reads `/proc/<pid>/maps`
      itself; the reboot question uses `dnf needs-restarting -r` and the
      distributions' flag files, and on FreeBSD `freebsd-version` and
      `checkrestart`. Report only
- [x] 2026-11-16: kegs describe themselves: a plain-text manifest inside
      each prefix (the uninstall test, STRATEGY C4)
- [ ] Resolver (`get`) with every lane of STRATEGY §4 off Arch. Today it
      resolves repository, AUR, flatpak and recipe on Arch, and the
      repository and flatpak elsewhere
- [ ] Flatpak, AppImage and container commands run on a system other than
      Arch
- [ ] `undo` and a real `upgrade` on a booted Arch system. So far: `undo`
      in an Arch container; install and remove on the Arch VM
- [ ] Transactions recorded with before and after package sets
- [ ] Taxonomy moved from Rust source to a data file
- [ ] `yapacc-build` (Arch) and `yapacc-keg` as their own binaries

Exit: a week of daily use on one machine per family with no yapacc-caused
breakage and no drift between yapacc and the native manager. Not started:
every run so far was a test pass on a virtual machine.

## M3 — Beta

- [x] 2026-11-16: snapshot providers (snapper, timeshift, zfs; plain btrfs
      list only; "none" is a valid answer) and a snapshot before a change
      with `[safety] snapshot`. Run on VMs: snapper on openSUSE and Arch,
      timeshift on Debian 13 and Arch, zfs on FreeBSD. Never run before a
      change on dnf, apk or xbps systems. The trait is in `src/snapshot.rs`,
      not yet in the shared crate
- [x] 2026-11-16: packaging recipes for Arch, Debian, RPM, Alpine and Void,
      built and installed in clean containers; install, upgrade and removal
      checked on Debian testing and Fedora 45. None is published
- [x] 2026-11-16: TUI with a home screen, report panes and a plan preview
      before every change
- [ ] A release: the release workflow run, binaries and packages published
- [ ] Handbook pages complete and proofread by someone other than their
      author
- [ ] MCP: `input_required` approval flow for clients on the 2026-07-28
      revision, in addition to the terminal hand-off
- [ ] Adapter SDK: scaffold, conformance fixtures; signed adapter packages
- [ ] A run on real hardware, on a non-x86_64 machine, and by an outside
      user, each recorded in VERIFICATION.md

Exit: a public beta announcement with the tier table and the "does not
protect against" list on the front page.

## Out of scope here

Machine-level features (services, boot, health, recovery) and fleets of
machines are not on this roadmap; they would be separate programs.

## After beta (unordered, each gated)

- Tier-2 managers promoted when someone other than the author maintains
  one.
- Machine interface (`--json`) frozen at 1.0.
- Parked items are revisited only when their entry criterion in STRATEGY §5
  is met.

## Where the risk is

What is missing is mostly evidence, not features. Seven backends exist and
one person has run them, on virtual machines. The items above that matter
most are therefore the ones nobody can tick alone: CI on the runner, real
hardware, and a second user.
