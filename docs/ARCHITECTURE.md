# yapacc architecture

Status: describes the tree as of 2026-11-19 and the target it is moving
toward. "Today" means implemented; where each part was run is in
[STATUS.md](../STATUS.md) and [VERIFICATION.md](VERIFICATION.md). "Target"
means decided and not built. Rules come from [STRATEGY.md](STRATEGY.md);
protections are specified in [SECURITY.md](SECURITY.md). How to use the
program is in the [handbook](handbook/README.md).

## 1. Shape

```
                 you            scripts            AI assistant
                  |                |                    |
            yapacc-tui      yapacc --json          yapacc-mcp
                  \________________|____________________/
                                   |
                              yapacc  (core)
        resolver · plans · gates · why · ledger · platform probe
                                   |
        ---------------------------------------------------------
        native lane           flatpak          build lane     adapters
        pacman, apt, dnf,     (its own CLI)    (Arch: AUR)    yapacc-source-*
        zypper, apk, xbps,
        pkg
                                   |
                  the machine's own tools and databases
```

Three things are true of every arrow: it crosses a process boundary, the data
on it is plain text or JSON, and the thing below never trusts the thing above
with more than it asked for.

yapacc is about packages. Machine-level features (services, boot, health,
recovery) and fleets of machines are out of scope; they would be separate
programs and are not part of this repository.

## 2. Binaries

| Binary | Purpose | Needs | State |
|---|---|---|---|
| `yapacc` | core CLI: reads, resolver, plans, gates, native and flatpak lanes, ledger | the native package manager's own tools; nothing else in the static build | today |
| `yapacc-tui` | interactive interface; every action runs the core | core; ratatui is linked only here | today |
| `yapacc-mcp` | MCP stdio server for AI assistants | core | today |
| `yapacc-build` | everything that executes third-party build code: AUR, recipes, autorepair | bubblewrap, base-devel, git | target (the code is in the core today) |
| `yapacc-keg` | foreign artifacts in isolated prefixes: AppImage, deb/rpm/snap extraction, repository parsing, GPG | bsdtar, ar, gpgv | target (in the core today) |

Why separate binaries rather than cargo features of one binary:

- **Optional by packaging.** A distribution ships `yapacc` alone, or with
  `yapacc-tui`; nobody pays for ratatui or bubblewrap they do not use.
- **Different dependency worlds.** The build lane only makes sense on the
  Arch family and needs a compiler toolchain; the core must run on a minimal
  Debian container.
- **Privilege separation.** The process that runs untrusted build code is not
  the process that holds the ledger lock or talks to sudo. This is the
  target; today the build code runs inside the core, in a sandbox.

Dispatch: `yapacc i` and `yapacc mcp` replace the core process with the
component. Components are searched **only** in `$YAPACC_LIBEXEC`, next to
the running binary, in `../lib/yapacc` and `../libexec/yapacc` relative to
it, and in `/usr/lib/yapacc`, in that order. They are never searched on the
whole `PATH`, so a stray `yapacc-tui` earlier on `PATH` cannot run under
yapacc's name. A missing component is exit code 6 with the package to
install. Components find the core through `$YAPACC_BIN`, then a sibling
named `yapacc`, then `PATH`.

Cargo features:

- `tui` (default): ratatui and crossterm, required only by `yapacc-tui`.
- `alpm` (off by default): links libalpm, used on Arch for plan analysis
  and by the `install alpm` command. It is for local builds only. The `alpm`
  Rust crate is GPL-3.0 and yapacc is GPL-2.0-only, so no released artifact
  enables it.

Without `alpm`, which is every released build, the pacman lane uses the
pacman command line. A normal user sees pacman's own preview
(`pacman -S --print`, `pacman -Rs --print`) before the gate.
`cargo build --release --no-default-features --bin yapacc` produces the
portable core. The static musl build of it is what
`contrib/test/families.sh` runs on eight container images. FreeBSD needs its
own build (done natively on FreeBSD 15.1 with the same command); no FreeBSD
binary is published.

## 3. Source layout (today)

A cargo workspace: the root package `yapacc` (a library and three binaries)
and two member crates. The shared crate is called `octo-head` after an early
sketch of the project as an octopus, with one head of shared code and one
arm per program.

```
Cargo.toml                  workspace and the yapacc package
src/main.rs                 core entry point
src/bin/yapacc-tui.rs       component entry points
src/bin/yapacc-mcp.rs
src/lib.rs                  library behind all three binaries
src/cli/                    command grammar and dispatch
    args.rs                   clap types (verbs, subverbs, shared gate flags)
    mod.rs                    run(), component dispatch, top-level dispatch
    read.rs                   read verbs on Arch
    portable.rs               read verbs through NativeBackend (other managers)
    portable_write.rs         change verbs and `get` on the other managers
    get.rs install.rs change.rs planning.rs flows.rs pkg.rs
                              Arch flows: resolver lanes, install verbs,
                              remove/mark, plans, AUR and safety flows, monitor
src/ops.rs                  portable write path: plan, native simulation, gate, apply
src/undo.rs                 take back the last native transaction
src/why.rs timeline.rs briefing.rs clean.rs restart.rs configs.rs
src/names.rs doctor.rs      read-only reports
src/snapshot.rs             snapshot providers; the snapshot before a change
src/ledger.rs               journal, replay, locking, witnesses
src/sandbox.rs              bubblewrap confinement for builds
src/repair.rs recipe.rs     build runner, failure classifier, recipe lane
src/resolve.rs plugin.rs    resolver and adapter protocol
src/backend.rs aur_rpc.rs   older Arch-only pacman code path; AUR RPC client
src/alpm_backend.rs alpm_flows.rs             libalpm (feature `alpm` only)
src/alpm_stub.rs alpm_flows_stub.rs           the same surface without it
src/foreign_extract.rs appimage.rs source_repos.rs cross_format.rs   keg lane
src/safety.rs update_service.rs   `safety apply` pipeline, update monitor
src/mcp.rs tui.rs           the two component bodies
src/taxonomy.rs docs.rs prompt.rs plan.rs config.rs vercmp.rs brew.rs

crates/octo-head/           shared library
    src/error.rs              typed errors and the exit-code contract
    src/util.rs               PATH lookup, name validation, component lookup
    src/platform.rs           the platform probe (every axis, independently)
    src/native/               native package backends behind one trait:
                              pacman.rs apt.rs dnf.rs zypper.rs apk.rs xbps.rs
                              pkgng.rs (FreeBSD pkg), mod.rs (trait, dispatch)
    src/pkglog.rs             native package log parsers
crates/xtask/               developer tooling, never shipped
    src/main.rs               `cargo xtask ci`: the CI workflow's jobs, locally
    src/site.rs               `cargo xtask site`: the website from docs/handbook
                              (`cargo xtask dist` builds the release files)

tests/                      cli_smoke.rs timeline_smoke.rs mcp_config.rs build_sandbox.rs
fixtures/                   captured tool output (native/) and the taxonomy corpus
docs/                       these design documents; handbook/ is the user handbook
site/                       templates, assets and demo transcripts of the website
contrib/
    packaging/                recipes: arch, debian, rpm, alpine, void; fill-release.sh
    test/families.sh          the container matrix
    test/pkg-lifecycle.sh     package install, upgrade, removal in containers
    test/vm/                  the QEMU/KVM harness (vm.sh, void-image.sh, void-vm.sh)
    fixtures/capture-native.sh
    man/yapacc.1  completions/  systemd/  safety/
.github/workflows/          ci.yml, release.yml
yapacc.example.toml         every configuration key, commented
```

Target: one crate per binary, with the ledger chain, prompts, version
comparison and taxonomy moved into the shared crate. Today the shared crate
holds errors, utilities, the platform probe, the native backends and the
package log parsers; everything else is in the root package.

## 4. The provider model

One small interface per axis; one implementation per tool; detection in
`platform.rs`. Behaviour is keyed on the detected tools, not on the
distribution name. The os-release `ID` is read only where the answer depends
on the distribution and not on a tool: which public archive `undo` can use
(`src/undo.rs`, `src/doctor.rs`), whether an openSUSE system is a rolling
one and upgrades with `zypper dist-upgrade` (`src/ops.rs`), whether
`briefing` has a news feed to read, and which repository `names` calls
"this one".

| Axis | Interface today | Providers today |
|---|---|---|
| Native packages, reads | `trait NativeBackend { manager, installed, explicit, fact, candidate, search, owner_of, files, orphans, updates }` in `crates/octo-head/src/native/mod.rs` | pacman, apt, dnf, zypper, apk, xbps, pkg. portage and nix are detected and have no backend |
| Native packages, changes | not in the trait: `src/ops.rs` builds, per manager, the commands and the manager's own simulation | the same seven; portage and nix are refused with the reason |
| Package history | parser functions in `crates/octo-head/src/pkglog.rs`; no trait yet (target: `PackageLog { transactions(since) }`) | pacman.log, dpkg.log, `dnf history`, zypp history, apk.log, FreeBSD `/var/log/messages`. xbps keeps no log. apt's `history.log` is read by `undo` only |
| Snapshots | `trait SnapshotProvider { name, list, create_command, guarantees }` in `src/snapshot.rs` | snapper, timeshift, zfs; plain btrfs can be listed and not created; "none" is a valid answer |
| Init | detection only | systemd, OpenRC, runit, s6, dinit, BSD rc.d. `restart` reads `/proc/<pid>/maps` itself and names the systemd unit of a process from its cgroup |
| System log | detection only | journald, syslog files, socklog |
| Boot | detection only (firmware, boot managers, bootloaders), shown by `platform` | features that act on the boot chain are out of scope |

Rules for providers:

1. Text in, struct out. Each parser is a pure function; commands run with
   `LC_ALL=C`. Fixtures captured from the real tool exist for pacman, apt
   (Debian and Ubuntu) and dnf (Fedora) under `fixtures/native/`. The
   zypper, apk, xbps and pkg parsers are tested on text written in the test
   itself.
2. A provider reports what it cannot know as *unknown*, never as a default.
   (`why` prints "unknown — no repository index on disk yet" rather than
   calling a package foreign; that distinction came from running it in
   fresh containers.)
3. The native manager is chosen by asking each candidate which one owns
   `/usr/lib/os-release`, not by the distribution name, so pacman installed
   on Ubuntu is correctly not "native".
4. A missing provider turns a feature off with a message naming the axis.

## 5. State

| What | Where | Notes |
|---|---|---|
| Configuration | `~/.config/yapacc/config.toml` | read, never written by yapacc; a file that does not parse is an error (exit 2), never a silent fallback |
| Install ledger (user) | `~/.local/state/yapacc/ledger/` | `journal.jsonl` is the truth; `state.json` is a rebuildable cache; `ledger.lock` |
| Install ledger (system) | `/var/lib/yapacc/ledger/` | same format |
| Kegs | `/var/lib/yapacc/isolated/foreign/` as root, `~/.local/share/yapacc/isolated/foreign/` otherwise | everything not packaged for this distribution lands here; each keg has `YAPACC-MANIFEST.txt` and is removable with `rm -r` |
| AppImages | `~/Applications/yapacc/appimages` | written by `install appimage` |
| Recipes, recorded fixes, adapters | `~/.local/share/yapacc/recipes/`, `fixes/`, `plugins/` | `plugins/` is read, not written |
| Old packages for `undo` | `$XDG_CACHE_HOME/yapacc/undo` or `~/.cache/yapacc/undo` | written after consent; never cleaned by yapacc; under `sudo yapacc undo` this is root's home |
| AUR build directory | `./NAME-aur-build` in the current directory | written by `install aur` |
| Monitor units | `~/.config/systemd/user/` or `/etc/systemd/system/` | only by `monitor daemon-install`, gated |
| Native packages | the native manager's database | never copied |

Read commands write to none of these.

The install ledger records only what no native database knows: flatpak,
AppImage and extracted-package installs made through yapacc, and installs
into yapacc's own prefixes. Native changes are recorded by the native log
only. The ledger is not a package database.

## 6. Contracts

**Exit codes** (`crates/octo-head/src/error.rs`, tested): 0 ok · 1 failed ·
2 usage · 3 refused by a gate or policy · 4 not found · 5 integrity check
failed · 6 something the command needs is unavailable (a tool or component,
a file this user may not read, a feature this build lacks). Older code paths still
return plain strings and therefore exit 1; they are moved to typed errors as
they are touched.

**Machine output**: `--json` on read commands. Target: a schema version field
and recorded fixtures in CI, so a change to a documented shape fails the
build; frozen at 1.0.

**Gate**: a mutating command shows the native manager's own command in full
and, where the manager has one that can run, its own simulation. It then
asks on a terminal. `--yes`/`--noconfirm` consents to *that command's plan*;
`--script` and non-terminal input never prompt; `--dryrun` only plans. A
gate that does not get consent (you answer no, or there is nobody to ask)
ends with exit code 3 and changes nothing. That includes the PKGBUILD review
gate and the recipe review gate. Anything discovered after the plan (an
inferred build dependency, for instance) needs its own consent.

A simulation that fails refuses the change with exit code 3, before the
gate, and `--yes` does not override it. This holds where the simulation's
exit status tells a failure from "there is something to do": apt, pacman
(install and remove), zypper, apk, xbps and `pkg delete`. dnf's
`--assumeno` exits non-zero by design, so its messages are read instead
(`dnf_simulation_failed` in `src/ops.rs`): an install or a removal dnf
could not resolve is refused the same way. `pkg install -n` is judged by
the catalogue on disk and by pkg's "no packages available" line. A failed
upgrade simulation on dnf, and the pacman upgrade preview, which is the
pending list (`pacman -Qu`), do not refuse. dnf and zypper simulate only
as root; when no simulation was run, the output says so.

**Undo**: `undo` reverses the last native transaction on pacman, apt, dnf
and zypper, and refuses whole (exit 3) when one old version cannot be found.
Sources: pacman takes old versions from its cache, then the Arch Linux
Archive (Arch family only); apt from the repositories, its cache, then
snapshot.debian.org on Debian or Launchpad on Ubuntu; zypper from versions
still installed or listed in the repository metadata; dnf uses its own
`history undo`, named by transaction ID.
`undo --dryrun` downloads nothing: it asks the archive whether each file
exists, and the download happens after consent. On apt the plan keeps the
architecture of each package (`name:arch`).

**Adapter protocol v1** (community sources): `yapacc-source-NAME --protocol`
prints `1`; `--probe PKG` prints one JSON candidate. Adapters answer
questions; they never install. Each question has a 5 s limit, after which
the adapter is killed. Adapters are found in the configured directories,
then `~/.local/share/yapacc/plugins`, then on `PATH` (lowest precedence).
They are not sandboxed and not signed.

**MCP**: protocol revision 2024-11-05 with tool annotations, 21 tools, stdio
only. A tool that changes the system only plans, unless the owner of the
machine set `[mcp] apply = "allow"`. Target: the 2026-07-28 revision's
`input_required` flow as an additional approval path for clients that
support it; never the only gate.

## 7. Known structural debt

- `src/cli/install.rs` is mostly one dispatcher of about 600 lines; split by
  lane when the build and keg lanes move out.
- About 180 functions return `Result<_, String>`, which exits 1 whatever the
  cause.
- Arch has two pacman code paths: the older one in `src/backend.rs` and the
  `NativeBackend` implementation in `crates/octo-head`. They should be one.
- `src/taxonomy.rs` is about 3,000 lines of data written as Rust; move it to
  a data file.
- The build lane, autorepair, recipes and the Homebrew code are Arch-only
  and live in the portable core.
- Fixtures from the real tools are missing for zypper, apk, xbps and pkg.
