# Every command

`yapacc --help` lists the everyday commands. This page lists all of them,
including the specialist ones it leaves out. `yapacc help COMMAND` prints
the options of any command, and `yapacc docs` carries longer built-in
topics.

Commands marked *read-only* change nothing. Commands marked *gated* show a
plan and stop at [the gate](gate.md).

## Everyday, on every system

### Reports (read-only)

| Command | What it answers |
|---|---|
| [`why NAME`](why.md) | Why is this package here? |
| [`timeline`](timeline.md) | What changed, and when? |
| [`briefing`](briefing.md) | What will the next upgrade do? |
| [`clean`](housekeeping.md#clean) | What can be reclaimed? |
| [`restart`](housekeeping.md#restart) | What must be restarted after an upgrade? |
| [`configs`](housekeeping.md#configs) | Which configuration files are left unmerged? |
| [`doctor`](doctor.md) | What works on this machine? |
| [`platform`](doctor.md#platform) | What kind of machine is this? |

### Looking things up (read-only)

| Command | What it does |
|---|---|
| `list` | installed packages; `--explicit` shows only what you asked for |
| `info PKG` | details of one installed package |
| `search QUERY` | search the repositories |
| `check` | pending updates, as of the last refresh |
| `inspect owns PATH` | which package owns this file |
| `inspect files PKG` | the files of a package |
| `inspect orphans` | dependencies nothing needs any more |
| `tree` | browse software by category, offline |

### names

`yapacc names NAME` says what a package is called on other distributions.
It asks [repology.org](https://repology.org), which means **it sends the
name you give to that site**. It needs `curl`, and exits with code 6
without it.

### Changes (gated)

| Command | What it does |
|---|---|
| [`install repo PKG…`](changes.md) | install from the distribution's repositories |
| [`remove repo PKG…`](changes.md) | remove packages |
| [`remove orphans`](changes.md) | remove dependencies nothing needs |
| [`upgrade`](changes.md) | full system upgrade |
| `refresh` | refresh the repository indexes only |
| [`get NAME`](changes.md#get-choosing-a-source) | choose a source and install; `--explain` only shows the choice |
| [`undo`](undo.md) | take the last transaction back |
| [`snapshot create`](snapshots.md) | take a filesystem snapshot now |

`snapshot list` is read-only.

### The program itself

| Command | What it does |
|---|---|
| `docs [TOPIC]` | built-in documentation; `docs --man` prints the manual page |
| `config` | the effective configuration and where it came from |
| `completions SHELL` | shell completions for bash, zsh, fish, elvish or powershell |
| `capabilities` | what this host supports, per source and safety feature |
| [`i`](tui.md) | the interactive interface |
| [`mcp`](mcp.md) | the server for AI assistants |

## Other sources

These commands install software that does not come from the distribution's
repositories. Each one records what it installed in yapacc's **ledger**, so
that it can be listed, audited and removed later. Native packages are never
recorded there.

| Command | What it does |
|---|---|
| `flatpak list\|search\|info` | read flatpak |
| `install flatpak`, `remove flatpak`, `update flatpak` | manage flatpaks (gated) |
| `install appimage`, `remove appimage`, `update appimage` | manage AppImages under your home directory (gated) |
| `install foreign`, `remove foreign`, `update foreign` | unpack a package made for another distribution into an isolated directory, without touching the system (gated) |
| `foreign info\|list` | inspect such a package, list the isolated installs |
| `external list\|audit`, `remove external` | everything the ledger tracks |
| `ledger status\|list\|head\|verify\|audit\|conflicts` | read the ledger and check its hash chain |
| `sources apt\|rpm` | read APT or RPM source definitions |
| `plugins` | list adapter programs that add sources |
| `repair LOG` | classify a failed build or install log and suggest repairs |
| `fixes list\|push\|pull` | keep recorded repairs, and share them through a git remote you configure |

Isolated installs are described in
[ISOLATED_INSTALLS.md](../ISOLATED_INSTALLS.md).

## Arch only

| Command | What it does |
|---|---|
| `search --aur\|--all` | include the AUR in a search |
| `query NAME` | repository and AUR metadata for one package |
| `aur deps\|review\|pkgbuild NAME` | read an AUR package: its dependency closure, its PKGBUILD |
| `install aur NAME` | clone, show the PKGBUILD for review, build in a sandbox, install (gated twice: review, then install) |
| `news` | recent Arch Linux news |
| `mark explicit\|dependency PKG` | change a package's install reason (gated) |
| `download PKG` | download a package to the cache without installing |
| `cache clean` | clean the package cache (gated) |
| `inspect leaves\|deps\|rdepends\|file-search\|groups\|facets\|pacdiff` | closer looks at the package database |
| `install local FILE…` | install package files with `pacman -U` (gated) |
| `index build\|search` | a local index of the repositories for offline search |
| `tree CATEGORY.SUBCATEGORY` | list the packages of a category (the category tree itself works everywhere) |
| `plan install\|remove\|upgrade … --out FILE`, `plan apply FILE` | write a plan to a file, apply it later; the plan is validated again when applied, and the command is rebuilt from it, never read from the file |
| `safety apply` | a staged change: plan, pre-check, gate, then an optional snapshot (`--snapshot-tag`), the change, and an optional health check (`--health-check`) |
| `package scaffold\|validate\|srcinfo` | help with writing a PKGBUILD |
| `install alpm`, `remove alpm`, `inspect alpm` | the same through libalpm, only in a build with the optional `alpm` feature |
| `monitor check`, `monitor daemon-install` | see below |

## Scheduled checks

On Arch, `yapacc monitor check` is a one-shot check for pending updates,
meant for a timer, and `yapacc monitor daemon-install` installs a systemd
timer that runs it (gated). Nothing of the kind is installed unless you run
that command: see rule 2 in the
[introduction](README.md#the-rules-yapacc-keeps). On other systems both
exit with code 6; use `yapacc check` from your own timer there.

## Global options

| Option | Effect |
|---|---|
| `--json` | machine-readable output on read commands |
| `--script` | never prompt; fail fast |
| `--noconfirm` | consent in advance, like `--yes` on a gated command |
| `--config PATH` | use this configuration file |
| `-V`, `--version` | print the version |

Global options go before the command: `yapacc --json why bash`.

Gated commands take `--dryrun` (plan only) and `--yes` (apply without
asking) after the command: `yapacc upgrade --dryrun`.
