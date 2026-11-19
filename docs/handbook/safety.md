# Safety and privacy

What yapacc protects you from, what it does not, what it sends over the
network and what it writes to disk. The design reasoning behind this page
is in [SECURITY.md](../SECURITY.md).

## What it protects against

**Changing the system without your say.** Every change goes through
[the gate](gate.md). Without `--yes` it asks; with nobody to ask it
refuses. An AI assistant connected through [MCP](mcp.md) cannot apply
anything unless you enabled that in the configuration.

**Surprises in what is run.** The plan, the native manager's own
simulation of it, and the exact command are printed before the question.

**Names that are not names.** Package names are validated against the
character set a package name can have, and passed to the native manager as
separate arguments after `--`. No shell command line is ever built from
package data, log contents or network answers.

**Half-applied undos.** `undo` is all or nothing. If one old version cannot
be found, or one download fails its check, nothing is changed.

**Removing the ground you stand on.** An undo that would remove the running
kernel is refused.

**Hostile build recipes** (the AUR on Arch). The PKGBUILD is shown for
review before anything is built. The build then runs in a bubblewrap
sandbox: your home directory and `/run` are hidden, the environment is
cleared, and the network is available only while sources are downloaded.

**Terminal tricks.** Escape sequences in package names, file names and log
lines are neutralised before they are printed.

## What it does not protect against

**A package you agreed to install.** Once you answer yes, the native
package manager runs as root and package scripts do what they do. yapacc
is not a sandbox for native packages.

**Lost configuration or data.** `undo` restores package versions only. If
an upgrade rewrote a configuration file or migrated a database, undo does
not bring the old one back. A [snapshot](snapshots.md) does.

**A compromised repository.** yapacc relies on your package manager's
signature checks and adds none of its own for native packages.

**Weakly verified archive downloads.** When `undo` has to fetch an old
version from an archive, the verification depends on the archive. A package
from the Arch Linux Archive must carry a signature that verifies, or the
undo stops. A file from snapshot.debian.org is compared with the hash the
service records, which is not a signature. Launchpad downloads are not
verified. The plan says which case applies to each package before you are
asked.

**A determined build.** The build sandbox has no seccomp filter and no
resource limits. It keeps a recipe away from your files; it is not a
virtual machine.

**A local attacker who is already you.** The configuration file and the
ledger of non-native installs live in your home directory. Something
running as you can change them.

## What reaches your terminal

Some of what yapacc prints is other people's text. Answers from the AUR
and from Repology, and the names of systemd units, are printed with
control characters replaced or escaped, so that a crafted name cannot move
the cursor, recolour the screen or hide a line. Package names are
validated before they are used in a plan.

## What leaves the machine

yapacc has no telemetry, no update check of its own and no listening
socket. These are the only cases in which it contacts a server:

| Command | Contacts | What is sent |
|---|---|---|
| `names NAME` | repology.org | the package name |
| `undo` | the distribution's archive: archive.archlinux.org, snapshot.debian.org or launchpad.net | the name and version of each package it needs |
| `briefing` | the distribution's news feed, where there is one (Arch); skipped with `--offline` | nothing but the request |
| `check`, `get`, and the AUR commands, on Arch | aur.archlinux.org | the names of packages that are not in the repositories; the name you ask for |
| `install`, `remove`, `upgrade`, `refresh` | your repositories, through your package manager | whatever your package manager sends |
| anything you give a URL to | that URL | |

Every other read command works offline, from what is on disk. On a machine
with no network at all, `why`, `timeline`, `clean`, `restart`, `configs`,
`doctor`, `platform`, `list`, `info`, `search` and `check` answer as usual.

## What it writes

Read commands write nothing. The complete list of places yapacc writes to:

| Path | When |
|---|---|
| `~/.cache/yapacc/` | old packages while an `undo` runs (deleted when it ends); build caches and the throwaway home of a sandboxed build |
| `~/.local/state/yapacc/ledger/` (or `/var/lib/yapacc/ledger/` as root) | when you install something *outside* the package manager through yapacc |
| `~/.local/share/yapacc/`, `~/Applications/yapacc/` | AppImages, isolated installs and recipes, if you use those commands |
| `./NAME-aur-build` in the current directory | `install aur NAME` (Arch) |
| a file you name | `plan … --out FILE`, `index build --path FILE` and similar |
| systemd timer units | only if you run `monitor daemon-install` |

It never writes its configuration file, never edits your shell startup
files, and never adds a hook to your package manager.

## Reporting a vulnerability

Please report security problems privately, as described in
[SECURITY.md at the top of the repository](../../SECURITY.md), and not in a
public issue.
