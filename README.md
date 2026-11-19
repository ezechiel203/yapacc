# yapacc

**Know what changed. Take it back.**

yapacc is an optional companion to the package manager you already have:
`pacman`, `apt`, `dnf`, `zypper`, `apk`, `xbps` or FreeBSD's `pkg`. It tells
you why a package is installed, what changed since the machine last worked,
what the next upgrade will do, and it can undo the last transaction. It
replaces nothing and leaves nothing running.

```console
$ yapacc why libcurl4t64
libcurl4t64 8.14.1-2+deb13u5  — native package (apt)
  what         easy-to-use client-side URL transfer library (OpenSSL flavour)
  why          pulled in as a dependency
  because      libcurl4t64 ← curl  (you installed curl)
  needed by    curl
  from         mirror+file:/etc/apt/mirrors/debian.list trixie/main amd64 Packages
  installed    2026-09-14 04:34 UTC (last install or upgrade)
  verified     repository signature (apt verifies the signed Release index)
  …
```

```console
$ yapacc timeline --since-boot     # what changed since the machine last started
$ yapacc briefing                  # what the next upgrade will do, before you run it
$ yapacc undo                      # take the last transaction back: plan, simulation, then it asks
```

| The question | The command |
|---|---|
| Why is this package here? | `yapacc why NAME` |
| What changed since it last worked? | `yapacc timeline --since-boot` |
| What will the next upgrade do? | `yapacc briefing` |
| Can I take the last change back? | `yapacc undo` |
| What can I clean up? What must be restarted? Which config files are unmerged? | `yapacc clean`, `restart`, `configs` |
| What works on this machine? | `yapacc doctor` |

**Website and handbook: <https://ezechiel203.github.io/yapacc/>**
(the same handbook is in [`docs/handbook`](docs/handbook/README.md), and
inside the program: `yapacc docs`).

## The rules it keeps

1. **Your package manager stays yours.** yapacc keeps no copy of what the
   native manager knows. Install or remove anything directly and yapacc is
   never confused by it.
2. **Nothing runs unless you ask.** No daemon, timer, hook or alias is
   installed by default.
3. **Read commands change nothing**, and work on a machine that has never
   used yapacc.
4. **Removable without trace.** Delete yapacc and everything it installed
   keeps working.
5. **Every change is gated.** A plain command asks; `--yes` consents;
   `--dryrun` only plans. A refusal exits 3 and changes nothing.
6. **Changes go through your package manager**, as the command it would run
   itself, shown in full, after its own simulation. The native log is the
   record.
7. **A protection says what it does not protect against**, in the output.

It is not a package manager, not a backup (`undo` restores package
versions, not configuration or data), not a service and not a
configuration system.

## Install

One static binary of under 6 MB, with no shared library. There is deliberately no
`curl | sh` installer.

```console
$ V=0.1.0; A=$(uname -m)
$ curl -fsSLO https://github.com/ezechiel203/yapacc/releases/download/v$V/yapacc-$V-$A-linux-musl.tar.gz
$ curl -fsSLO https://github.com/ezechiel203/yapacc/releases/download/v$V/SHA256SUMS
$ sha256sum -c --ignore-missing SHA256SUMS
$ sudo tar -xzf yapacc-$V-$A-linux-musl.tar.gz --strip-components=1 -C /usr/local
$ yapacc doctor
```

Or build it (Rust 1.89 or newer):

```console
$ cargo build --release --no-default-features --bin yapacc
$ ./target/release/yapacc doctor
```

Without root, on FreeBSD, or as a distribution package:
[the install page](docs/handbook/install.md).

## What works where

| | pacman | apt | dnf | zypper | apk | xbps | pkg |
|---|---|---|---|---|---|---|---|
| Read commands (`why`, `briefing`, `clean`, `restart`, `doctor`, …) | yes | yes | yes | yes | yes | yes | yes |
| Gated `install`, `remove`, `upgrade` | yes | yes | yes | yes | yes | yes | yes |
| `timeline` | yes | yes | yes | yes | with apk-tools 3 | no log | yes |
| `undo` | yes | yes | yes | yes | no | no | no |

Run on booted virtual machines of Arch, Debian 13, Ubuntu 24.04, Fedora 45,
Rocky Linux 9.9, openSUSE Leap 16.1 and Tumbleweed, Alpine 3.24, Void, and
FreeBSD 14.5 and 15.1. Not run on real hardware, on architectures other
than x86_64, or on derivative distributions. OpenBSD and NetBSD are not
supported.

The details and the limits: [what works where](docs/handbook/support.md).
The current state, honestly: [STATUS.md](STATUS.md). The evidence, run by
run: [docs/VERIFICATION.md](docs/VERIFICATION.md).

## Three programs

| Program | What | Required |
|---|---|---|
| `yapacc` | the command line | yes |
| `yapacc-tui` | an interactive interface, `yapacc i` | no |
| `yapacc-mcp` | an MCP server for AI assistants, `yapacc mcp`: it can read and propose, and cannot apply unless you enable that | no |

For scripts: `--json` on the read commands, and seven exit codes
(0 ok, 1 failed, 2 usage, 3 refused, 4 not found, 5 integrity check failed,
6 something the command needs is unavailable).

## Documentation

- [The handbook](docs/handbook/README.md): install, a ten-minute tour, each
  command, the gate, snapshots, notes per distribution, scripting, safety
  and privacy, troubleshooting.
- [STATUS.md](STATUS.md): what works today and what was never verified.
- [docs/STRATEGY.md](docs/STRATEGY.md): what yapacc is, its rules, its
  scope. [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md): how it is built.
  [docs/SECURITY.md](docs/SECURITY.md): each protection and its limits.
  [docs/ROADMAP.md](docs/ROADMAP.md): what comes next.
- [CONTRIBUTING.md](CONTRIBUTING.md), [CHANGELOG.md](CHANGELOG.md),
  [SECURITY.md](SECURITY.md) (how to report a vulnerability).

## Development

```console
$ cargo test --workspace --all-targets
$ cargo xtask ci        # the CI jobs, locally: lints, tests, static build, seven distributions in containers
$ cargo xtask site      # the website, into target/site
$ cargo xtask dist      # release archives, into target/dist
```

## Licence

Copyright (C) 2026 Alexandre BETRY. yapacc is free software under the GNU
General Public License, version 2 only. See [LICENSE](LICENSE).
