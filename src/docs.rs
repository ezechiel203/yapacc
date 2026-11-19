// yapacc — built-in documentation
// Copyright (C) 2026 Alexandre BETRY
// SPDX-License-Identifier: GPL-2.0-only
//! Built-in documentation: the topics served by `yapacc docs [TOPIC]` and
//! shown in the interactive interface, and the manual page
//! (contrib/man/yapacc.1, printed by `yapacc docs --man`).
//!
//! Every statement below is about the program as it is. The tests at the
//! end keep it that way where a machine can: every topic is listed, no line
//! is wider than 80 columns, every `yapacc …` command line parses, the
//! `commands` topic names every command, and the JSON table printed here is
//! the one the dispatcher enforces.

pub const MAN_PAGE: &str = include_str!("../contrib/man/yapacc.1");

/// (slug, title, one-line about)
pub fn topics() -> &'static [(&'static str, &'static str, &'static str)] {
    &[
        (
            "getting-started",
            "Getting started",
            "the first commands to run; all of them are read-only",
        ),
        (
            "daily",
            "Everyday commands",
            "reports and gated changes: what each does, and does not do",
        ),
        (
            "commands",
            "Every command",
            "the everyday commands, and the ones `--help` leaves out",
        ),
        (
            "workflow",
            "The gate",
            "plan, simulation, question: how a change is confirmed",
        ),
        (
            "why",
            "Why is this here",
            "why a package is installed, and what still needs it",
        ),
        (
            "undo",
            "Undo",
            "taking the last transaction back: old versions, guards",
        ),
        (
            "snapshots",
            "Snapshots",
            "snapper, timeshift, zfs: list, create, one before a change",
        ),
        (
            "platform",
            "Platform and doctor",
            "what kind of machine this is, what works on it, the tiers",
        ),
        (
            "safety",
            "Safety and privacy",
            "what it protects against, what it contacts, what it writes",
        ),
        (
            "exit-codes",
            "Exit codes",
            "what the exit codes 0 to 6 mean",
        ),
        (
            "json",
            "JSON output",
            "--json: the commands that have a JSON form, and their shapes",
        ),
        (
            "config",
            "Configuration",
            "the configuration file: where it is, each key and default",
        ),
        (
            "get",
            "Choosing a source",
            "where a package can come from, and which source wins",
        ),
        (
            "inspection",
            "Inspecting packages",
            "list, info, inspect: files, owners, orphans, dependency trees",
        ),
        (
            "search",
            "Search and the software tree",
            "search, query, tree, index: finding a package",
        ),
        (
            "ledger",
            "The ledger",
            "what yapacc installed outside the package manager",
        ),
        (
            "foreign",
            "Isolated installs",
            "unpacking a deb, rpm or snap into a directory of its own",
        ),
        (
            "repo-trust",
            "Repository trust",
            "signature and hash checks of installs from a repository",
        ),
        (
            "aur",
            "The AUR (Arch only)",
            "reading and building AUR packages: review, sandbox, install",
        ),
        (
            "autorepair",
            "Build repair",
            "classifying a failed build, the repairs, the record of fixes",
        ),
        (
            "plans",
            "Saved plans and staged changes (Arch only)",
            "plan … --out, plan apply, safety apply",
        ),
        (
            "daemon",
            "Scheduled checks",
            "monitor check, and the optional systemd timer that runs it",
        ),
        (
            "plugins",
            "Plugin adapters",
            "separate programs that answer for a source: protocol, limits",
        ),
        (
            "tui",
            "The interactive interface",
            "`yapacc i`: the home screen, the tabs, the keys",
        ),
        (
            "ai",
            "AI assistants (MCP)",
            "`yapacc mcp`: the tools, and what an assistant may do",
        ),
    ]
}

/// Other names a topic answers to. Not listed by `yapacc docs`.
const ALIASES: &[(&str, &str)] = &[
    ("mcp", "ai"),
    ("gate", "workflow"),
    ("resolution", "get"),
    ("isolated", "foreign"),
    ("doctor", "platform"),
];

pub fn render(topic: &str) -> Option<String> {
    let topic = ALIASES
        .iter()
        .find(|(alias, _)| *alias == topic)
        .map_or(topic, |(_, slug)| slug);
    let t = match topic {
        "getting-started" => GETTING_STARTED,
        "daily" => DAILY,
        "commands" => COMMANDS,
        "workflow" => WORKFLOW,
        "why" => WHY,
        "undo" => UNDO,
        "snapshots" => SNAPSHOTS,
        "platform" => PLATFORM,
        "safety" => SAFETY,
        "exit-codes" => EXIT_CODES,
        "json" => return Some(json_topic()),
        "config" => CONFIG,
        "get" => GET,
        "inspection" => INSPECTION,
        "search" => SEARCH,
        "ledger" => LEDGER,
        "foreign" => FOREIGN,
        "repo-trust" => REPO_TRUST,
        "aur" => AUR,
        "autorepair" => AUTOREPAIR,
        "plans" => PLANS,
        "daemon" => DAEMON,
        "plugins" => PLUGINS,
        "tui" => TUI,
        "ai" => AI,
        _ => return None,
    };
    Some(t.to_string())
}

pub fn index() -> String {
    let mut s = String::from(
        "yapacc: an optional companion to the package manager you already have\nBuilt-in documentation\n\nTopics (yapacc docs TOPIC):\n",
    );
    for (slug, title, about) in topics() {
        s.push_str(&format!(
            "  {slug:<16} {title}\n                   {about}\n"
        ));
    }
    s.push_str(
        "\n`yapacc docs --man` prints the manual page (pipe it to `man -l -`).\n`yapacc help COMMAND` prints the options of one command.\nThe handbook: https://ezechiel203.github.io/yapacc/\n",
    );
    s.push_str(
        "\nyapacc is free software under the GNU General Public License, version 2\nonly. Copyright (C) 2026 Alexandre BETRY.\n",
    );
    s
}

/// The `json` topic. The list of commands is printed from the table the
/// dispatcher checks, so the two cannot differ.
fn json_topic() -> String {
    let mut s = String::from(JSON_HEAD);
    for (path, condition) in crate::cli::JSON_COMMANDS {
        if condition.is_empty() {
            s.push_str(&format!("  {path}\n"));
        } else {
            s.push_str(&format!("  {path:<18}{condition}\n"));
        }
    }
    s.push_str(JSON_TAIL);
    s
}

const GETTING_STARTED: &str = r#"GETTING STARTED

yapacc is an optional companion to the package manager you already have:
pacman, apt, dnf, zypper, apk, xbps or FreeBSD's pkg. You keep using it.
yapacc reads what it already records and, when you ask for a change, runs
the command it would run itself, after showing it to you.

Nothing has to be configured. None of these commands changes anything:

  yapacc doctor              # what works on this machine, check by check
  yapacc platform            # what kind of machine this is
  yapacc why bash            # why a package is installed
  yapacc timeline --limit 5  # the last package changes
  yapacc briefing --offline  # what the next upgrade will do
  yapacc clean               # what can be reclaimed; it removes nothing
  yapacc undo --dryrun       # what taking the last change back would do

They read what is on disk. Two of them can contact a server: `briefing`
without --offline fetches the distribution's news where there is a feed,
and `undo --dryrun` asks the distribution's archive whether an old version
it needs is still there. Neither downloads a package.

Before you let yapacc change something, read `yapacc docs workflow`. It
describes the plan, the question and the refusal that every change shares.

Shell completions are installed by the release archive and by the
distribution packages. To write one yourself:
  yapacc completions bash > ~/.local/share/bash-completion/completions/yapacc
  yapacc completions zsh > ~/.zsh/completions/_yapacc
  yapacc completions fish > ~/.config/fish/completions/yapacc.fish

The manual page is `man yapacc` where it is installed. Anywhere:
  yapacc docs --man | man -l -

Next: `yapacc docs daily` for the everyday commands, `yapacc docs commands`
for every command, `yapacc help COMMAND` for the options of one.
"#;

const DAILY: &str = r#"EVERYDAY COMMANDS

Reports. They are read-only: each prints what it found and the command you
would run yourself, and runs nothing.

  why NAME    why a package is installed, what needs it, where it came
              from, how it was verified. See `yapacc docs why`.
  timeline    installs, upgrades and removals from the package manager's
              own log, newest first, 50 by default (--limit N).
              --since YYYY-MM-DD starts at a day. --since-boot starts at
              the boot before the current one: "it worked yesterday".
              --html writes one self-contained page (not with --json).
              xbps keeps no log, and timeline says so. A run of
              `apk --quiet` is logged without its packages: timeline
              counts such runs in a note.
  briefing    what the next upgrade will do: kernel, C library, init
              system, the package manager itself, remote access; security
              fixes where the system has a source for them; distribution
              news (Arch, fetched with curl); unmerged configuration; free
              disk space; the snapshot tool found. The pending list is as
              of your last repository refresh: briefing never refreshes.
              On Arch it also runs `checkupdates` when that is installed,
              which asks the mirrors. --offline contacts nothing.
  clean       what can be reclaimed: orphans, the package cache, yapacc's
              own cache, unused flatpak runtimes. Each item is labelled
              safe or review and comes with its command. A size that
              could not be read in full is a lower bound and says so.
              On dnf the downloaded packages and the repository metadata
              are two items, each with its own command.
  restart     whether a reboot is due, and which services and programs
              still use files an upgrade replaced. An ordinary user sees
              only their own processes: the output says how many it could
              not examine. Run it as root for the whole picture.
  configs     configuration files an upgrade left unmerged under /etc:
              .pacnew .pacsave, .dpkg-dist .dpkg-new .dpkg-old .ucf-dist,
              .rpmnew .rpmsave, .apk-new; with the command to compare
              each. Nothing is known for xbps and pkg.
  doctor      what works on this machine, one line per check.
  platform    what kind of machine this is. See `yapacc docs platform`.
  names NAME  what a package is called on other distributions. This one
              sends the name to repology.org, and needs curl.

Looking things up, read-only, from what is on disk:
  list, info PKG, search QUERY, check, tree,
  inspect owns PATH, inspect files PKG, inspect orphans
`check` lists pending updates as of the last refresh; on Arch it also asks
aur.archlinux.org about the packages that are in no repository.
No read command refreshes or downloads repository metadata. Where none is
on disk (a fresh image, an emptied cache), `check`, `search`, `briefing`
and `clean` say "no repository metadata on disk" with the command that
downloads it, instead of an empty answer.

Changes. Each one shows its plan, the package manager's own simulation and
the exact command, then asks. --dryrun only plans; --yes applies without
asking; a refusal exits 3. See `yapacc docs workflow`.
  install repo PKG…  install from the distribution's repositories
  remove repo PKG…   remove packages
  remove orphans     remove dependencies that nothing needs any more
  upgrade            full system upgrade
  refresh            refresh the repository indexes only
  get NAME           choose a source, then install: `yapacc docs get`
  undo               take the last transaction back: `yapacc docs undo`
  snapshot create    take a filesystem snapshot: `yapacc docs snapshots`

You never have to use them. A change you make directly with your package
manager is just as visible to `timeline` and `undo`.

Not supported: OpenBSD, NetBSD. Portage and Nix are detected only: changes
are refused there, with the reason.
"#;

const COMMANDS: &str = r#"EVERY COMMAND

`yapacc --help` lists the everyday commands. This page lists all of them.
`yapacc help COMMAND` prints the options of any command. Commands marked
(gated) show a plan and stop at the gate: `yapacc docs workflow`.

EVERYDAY, ON EVERY SYSTEM

Reports (read-only)
  why NAME         why is this package here?
  timeline         what changed, and when?
  briefing         what will the next upgrade do?
  clean            what can be reclaimed?
  restart          what must be restarted after an upgrade?
  configs          which configuration files are left unmerged?
  doctor           what works on this machine?
  platform         what kind of machine is this?

Looking things up (read-only)
  list             installed packages; --explicit: only what you asked for
  info PKG         details of one installed package
  search QUERY     search the repositories
  check            pending updates, as of the last refresh
  inspect owns PATH    which package owns this file
  inspect files PKG    the files of a package
  inspect orphans      dependencies nothing needs any more
  tree             browse software by category, offline
  names NAME       the name on other distributions (asks repology.org)

Changes (gated)
  install repo PKG…    install from the distribution's repositories
  remove repo PKG…     remove packages
  remove orphans       remove dependencies nothing needs
  upgrade              full system upgrade
  refresh              refresh the repository indexes only
  get NAME             choose a source and install; --explain only shows
  undo                 take the last transaction back
  snapshot create      take a filesystem snapshot (snapshot list: read-only)

The program itself
  docs [TOPIC]         this documentation; docs --man: the manual page
  config               the effective configuration and where it came from
  completions SHELL    bash, zsh, fish, elvish or powershell
  capabilities         what this host supports, per format (not in --help)
  i                    the interactive interface (optional yapacc-tui)
  mcp                  the server for AI assistants (optional yapacc-mcp)

OTHER SOURCES (not in --help, except ledger)
What these install is recorded in the ledger, so that it can be listed,
audited and removed. Native packages are never recorded there.
  flatpak list|search|info                     read flatpak
  install|remove|update flatpak                manage flatpaks (gated)
  install|remove|update appimage               AppImages under your home
  install|remove|update foreign                a deb, rpm or snap unpacked
                                               into a directory of its own
  install tx, tx-apt, tx-apt-repo, tx-apt-sources, tx-rpm, tx-rpm-repo
                                               several such packages at once
  install container                            a distrobox container
  foreign info|list                            inspect one, list them
  external list|audit, remove|update external  everything the ledger tracks
  ledger status|list|head|verify|audit|conflicts   read and check the ledger
  sources apt|rpm                              read repository definitions
  plugins                                      list adapter programs

ARCH ONLY (not in --help)
On another system each of these says so and exits 6.
  search --aur|--all   include the AUR in a search
  query NAME           repository and AUR metadata for one package
  aur deps|review|pkgbuild NAME    read an AUR package
  install aur NAME     review, build in a sandbox, install (gated twice)
  install local FILE…  install package files you have (gated)
  install alpm, remove alpm        through libalpm (builds with `alpm`;
                                   other builds exit 6 and name the
                                   command that does the same)
  news                 recent Arch Linux news
  mark explicit|dependency PKG…    change the install reason (gated)
  download PKG…        fetch into the cache without installing (gated)
  cache clean          clean the package cache (gated)
  inspect leaves|facets|deps|rdepends|file-search|groups|pacdiff|alpm
  index build|search   a file index of the repositories
  plan install|remove|upgrade, plan apply FILE     saved plans
  plan flatpak, flatpak-remove, flatpak-update, appimage, foreign, tx,
       tx-apt, tx-apt-repo, tx-apt-sources, tx-rpm, tx-rpm-repo,
       container, cross, safety                    print a plan only
  safety apply         a staged change: plan, pre-check, gated apply
  package scaffold|validate|srcinfo    help with writing a PKGBUILD
  tree CATEGORY.SUBCATEGORY            the packages of a subcategory
  monitor check        one report of pending updates, for a timer
  monitor daemon-install   install a systemd timer for `monitor check`
                           (gated). Nothing of the kind is installed
                           unless you run this.

BUILD REPAIRS (not in --help)
They serve the builds above, but run on any system.
  repair LOG           classify the failures in a build log
  fixes list|push|pull     the repairs recorded here; your git remote

GLOBAL OPTIONS (before the command)
  --json           JSON instead of text, on the commands that have a JSON
                   form; exit 2 on the others (`yapacc docs json`)
  --script         never prompt; a command that would ask is refused
  --noconfirm      consent in advance, like --yes on a gated command
  --config PATH    use this configuration file
  -V, --version    print the version

A command that needs something this machine lacks names it and exits 6.
"#;

const WORKFLOW: &str = r#"THE GATE

Every command that changes the system goes through the same gate: install,
remove, upgrade, refresh, undo, snapshot create, and the specialist ones.

What happens, in order
  1. Plan        what will be installed, removed, upgraded or restored.
  2. Simulation  the package manager's own dry run of that plan, printed
                 as the manager says it. If the simulation fails, the
                 change is refused (exit 3) before any question: an
                 unknown package or a broken dependency stops here.
                 dnf's --assumeno preview and `pkg install -n` end with
                 an error status by design: they are judged by what they
                 print (dnf's "Error:" or "Failed to resolve the
                 transaction" is a refusal, exit 3; pkg's "No packages
                 available to install matching" is "not found", exit 4).
                 Exempt: `pkg upgrade`, and the upgrade preview on pacman
                 and dnf (it runs without the refresh).
                 Before it, these are refused as "not found" (exit 4):
                 removing a package that is not installed, and, on
                 FreeBSD, installing a package the catalogue on disk does
                 not have. On Arch, with no repository database on disk,
                 an install is refused (exit 6) with the command that
                 downloads them.
  3. Command     the exact command that is about to run.
  4. Gate        the question, unless a flag already answered it.
  5. Apply       the command, through sudo or doas when you are not root.
                 It is run so that the package manager does not ask a
                 second time (-y, --noconfirm, --non-interactive): your
                 answer at the gate is the consent. The last line says
                 "Nothing was changed" (after an upgrade: "No package
                 was changed") when the installed packages are the same
                 as before.
  6. Record      the package manager writes its log as usual. yapacc keeps
                 no second record of native changes.

How you answer
  the plain command   the plan is shown and you are asked [y/N]; Enter is no
  --yes               the plan is shown and applied without a question
  --dryrun            the plan and the simulation are shown; nothing is
                      applied. Not together with --yes.
  --script            never prompts: needs --yes or --dryrun, otherwise it
                      is a refusal
  input from a pipe   cannot answer: a refusal unless --yes or --dryrun
  --noconfirm         another spelling of --yes, placed before the command

The question follows your locale ([o/N] in French, [j/N] in German);
English y and yes are always accepted.

A refusal exits 3 and has changed nothing, whether you answered no or
nobody could be asked. A change that was attempted and failed exits 1.

What --yes does not answer
  --yes consents to the plan you could have read, and to nothing else:
  * undoing a whole upgrade on zypper, packages with no earlier record
    are asked about at the terminal;
  * the review of a PKGBUILD or a generated recipe needs --no-review;
  * a non-free licence of an isolated install needs --non-free.
  Without a terminal each of these is a refusal (exit 3).

What is run, by package manager
          install             remove            upgrade
  pacman  pacman -S           pacman -Rs        pacman -Syu
          --noconfirm         --noconfirm       --noconfirm
  apt     apt-get install -y  apt-get remove -y apt-get update, then
                                                apt-get upgrade
                                                --with-new-pkgs -y
  dnf     dnf install -y      dnf remove -y     dnf upgrade --refresh -y
  zypper  zypper install      zypper remove     zypper dist-upgrade on
                                                rolling releases, else
                                                zypper update
  apk     apk add             apk del           apk update, apk upgrade
  xbps    xbps-install -y     xbps-remove -y    xbps-install -Syu
  pkg     pkg install -y      pkg delete -y     pkg upgrade -y
Package names are validated and passed as separate arguments after `--`;
no shell command line is built. On pacman a removal is -Rs: configuration
files you modified are kept as .pacsave. On Arch, `refresh` warns that
refreshing and then installing a package is a partial upgrade.

Privileges
  Run yapacc as yourself. For the one command that needs root it calls
  sudo, or doas when there is no sudo; the password prompt is theirs.
  With --script, or without a terminal, the tool is called with -n and
  fails at once instead of waiting for a password. As root no tool is
  used. With neither tool the plan is still shown, then exit 6.
  dnf and zypper can only simulate as root: their simulation runs through
  the same tool, with -n when the run may not ask for a password. If the
  tool then refuses, there is no simulation and the plan says so.

A snapshot first
  With `[safety] snapshot = "auto"` or "auto-best-effort" a filesystem
  snapshot is taken after you consent and before the command runs. The
  plan says so, and says so too when none will be taken because the
  machine has no snapshot tool. A dry run takes none. See
  `yapacc docs snapshots`.

The gate is not a sandbox: once you answer yes, the package manager runs
with the privileges it always has. The gate makes sure you saw the plan.
"#;

const WHY: &str = r#"WHY IS THIS HERE (yapacc why NAME)

Read-only. It answers for any package, whether or not yapacc had anything
to do with installing it.

  yapacc why bash

One block is printed for each place that knows the name: the native
package, a flatpak with a matching name, and anything yapacc installed
itself outside the package manager.

  what        the package's own one-line description
  why         you asked for it, or it was pulled in as a dependency, or it
              is a dependency that nothing needs any more. "orphan" is
              said only of a package `inspect orphans` lists too; a
              dependency nothing depends on that the manager still keeps
              (apt: another package recommends or suggests it) is
              described as that
  because     the chain back to a package you installed yourself
  needed by   the installed packages that depend on it directly; a long
              list is cut on the terminal, and --json lists all of it
  from        the repository it came from, or that this is not known
              and why. On zypper it is the repository that offers the
              package now. On apt a stock package whose installed version
              has left the index is described as that, not as foreign
  installed   when it was last installed or upgraded
  verified    how the package manager checked it
  ledger      whether yapacc itself has a record of installing it

What could not be checked is printed as a line starting with `note:`.
yapacc does not guess.

Exit codes: 0 found; 4 not installed anywhere yapacc can see; 2 the name
is not a valid package name.

  yapacc --json why bash

Related: `yapacc inspect owns /usr/bin/env` finds the package that owns a
file; `yapacc inspect files bash` lists the files of a package.
"#;

const UNDO: &str = r#"UNDO (yapacc undo)

Takes the last package transaction back. It reads the last transaction
from the package manager's own log, works out its reverse, shows the plan
and the manager's simulation, and asks. It works on pacman, apt, dnf and
zypper. On apk, xbps and pkg it says that undo is not available (exit 6).

  yapacc undo --dryrun

  installed a package   -> it is removed
  removed a package     -> that version is installed again
  upgraded a package    -> the previous version is restored
  downgraded a package  -> the newer version is restored

It restores package versions. It does NOT restore configuration files,
databases or anything a package script changed: a filesystem snapshot
does. The output repeats this every time.

Where old versions come from, in order
  pacman          the pacman cache, then the Arch Linux Archive. An
                  archive package without a signature that verifies stops
                  the undo (exit 5).
  apt on Debian   the configured repositories, the apt cache, then
                  snapshot.debian.org, checked against the hash that
                  service records (not a signature).
  apt on Ubuntu   the configured repositories, the apt cache, then
                  Launchpad; those downloads are NOT verified, and the
                  plan says so.
  dnf             the enabled repositories, through `dnf history undo N`
                  for the transaction ID shown in the plan. No archive.
  zypper          the configured repositories only. No archive: on a
                  rolling release an upgrade more than a day or two old
                  usually cannot be undone.

All or nothing: if one old version cannot be found, the whole undo is
refused (exit 3) and nothing is changed. An archive that does not answer
(20 s to connect, 60 s in all, asked twice) is said apart from an archive
that does not have the version: "did not answer … Try again later."

Afterwards the installed packages are compared with the plan. A success
ends with "Verified: N of N package(s) are at the planned state." When
the package manager reports an error, the message says whether the
versions were restored all the same (dnf exits 1 when a package script
fails after every version is back) or which packages are not at the
planned version. Commands that report no error and leave a package at
another version are a failure (exit 1).

A dry run writes nothing and downloads nothing: it asks the archive
whether each file exists and reports where it would come from, with its
size when the archive gives one. A real run downloads after you agreed,
into a private directory under ~/.cache/yapacc/undo that is deleted when
the undo ends, with its parents when they are left empty. On apt, when
files must be downloaded, you are asked twice: before the download, then
with apt's simulation of the restore in front of you. --yes answers both.

Guards
  * An undo that would remove the running kernel is refused.
  * On apt, an undo whose simulation would remove a package that was not
    part of the transaction is refused (exit 3).
  * A file that fails its signature or hash check is deleted and the undo
    stops (exit 5).
  * On zypper, packages with no earlier record cannot be told apart from
    upgraded ones. They are named and asked about at the terminal; --yes,
    --script and a pipe do not answer (exit 3 without a terminal).
  * Names and versions taken from the log are validated before any
    command is built.

Limits
  * One transaction: the last one in the log.
  * A restored package is upgraded again by the next full upgrade unless
    you hold it yourself (IgnorePkg, apt-mark hold, a dnf or zypper lock).
  * openSUSE keeps /var/log/zypp/history readable by root only: run
    `sudo yapacc undo` there.
  * With `[safety] snapshot` set, a snapshot is taken first, as for any
    other change.
  * It ran end to end on virtual machines of Arch, Debian 13, Ubuntu
    24.04, Fedora 45, Rocky Linux 9.9, openSUSE Leap 16.1 and Tumbleweed.
    It has not been run on real hardware.
"#;

const SNAPSHOTS: &str = r#"SNAPSHOTS

`undo` restores package versions. A filesystem snapshot restores the rest:
configuration files and data that package scripts rewrote. yapacc does not
implement snapshots; it drives the tool the machine already has.

  tool          list   create
  snapper       yes    yes
  timeshift     yes    yes   (rsync mode and btrfs mode)
  zfs           yes    yes   (the dataset mounted at /)
  plain btrfs   yes    no    (subvolumes are listed; none is created)

  yapacc snapshot list
  yapacc snapshot create --description "before the upgrade" --dryrun

`snapshot create` is a change: it goes through the gate, and yapacc calls
sudo or doas for it. `snapshot list` is read-only and never calls them:
where listing needs root (btrfs, snapper) it says so and exits 6, and you
run it as root. `yapacc platform` shows the tools it detected.

A snapshot before every change, in the configuration file:
  [safety]
  snapshot = "auto"            # off (default) | auto | auto-best-effort
  snapshot_provider = "auto"   # auto | snapper | timeshift | zfs | none

  off               nothing is taken
  auto              taken after you consent, before the native command;
                    if it fails, the change is refused (exit 3)
  auto-best-effort  the same; if it fails, a warning is printed and the
                    change goes on
What the plan says, before the question, also in a dry run:
  * snapshot_provider = "auto" and a tool is found: the tool and the
    command.
  * snapshot_provider = "auto" and the machine has no snapshot tool: "No
    snapshot will be taken … the change will run without a snapshot."
    Your answer covers that.
  * snapshot_provider names a tool (snapper, timeshift, zfs) that is not
    usable here: with "auto" the change is refused (exit 3), also under
    --dryrun; with "auto-best-effort" a warning says no snapshot will be
    taken and the change goes on.
A dry run never takes a snapshot.

If timeshift was never set up, its first run picks a device and writes
its own configuration. Set it up yourself first; the plan warns about it.

yapacc never restores a snapshot. Rolling a root filesystem back is done
with the tool itself (snapper rollback, timeshift --restore, zfs
rollback), often from a rescue system.
"#;

const PLATFORM: &str = r#"PLATFORM AND DOCTOR

Two read-only commands that describe the machine. Attach both to a bug
report.

  yapacc platform
  yapacc doctor

platform prints one line per axis, each detected on its own: distribution,
architecture and C library, native package manager, extra sources, init
system, firmware, boot managers, bootloaders, log systems, package logs,
root filesystem, snapshot tools, support tier. Where an axis could only be
read in part, the line says so. When several package managers are
installed, the native one is the one whose database owns
/usr/lib/os-release. dnf and zypper read the same rpm database: between
them it is the one that has repositories configured (/etc/zypp/repos.d,
/etc/yum.repos.d), and zypper on SUSE, dnf elsewhere, when both have.

Support tiers, of the native package manager
  1  pacman, apt, dnf         the main targets
  2  zypper, apk, xbps, pkg   supported; some features are missing
                              (no undo on apk, xbps and pkg; no timeline
                              on xbps)
  3  portage, nix             detected only: changes are refused with the
                              reason, and read commands have no backend

doctor prints one line per check:
  ok    it works
  warn  something a feature of this system uses is missing or weaker;
        the line says what you lose
  fail  a command will not work until this is fixed; the line says why
  n/a   the check does not apply to this system, or an optional part
        (yapacc-tui, yapacc-mcp, flatpak, gpgv) is not installed; the
        line says what it would be for
The lines for undo, timeline and briefing say what each can do on this
machine: where old versions would come from, which log is read, whether
there is a news feed. doctor creates no file.

  yapacc --json platform
  yapacc --json doctor

It ran on virtual machines of Arch, Debian 13, Ubuntu 24.04, Fedora 45,
Rocky Linux 9.9, openSUSE Leap 16.1 and Tumbleweed, Alpine 3.24, Void, and
FreeBSD 14.5 and 15.1. Not on real hardware, not on another architecture
than x86_64, not on derivatives. OpenBSD and NetBSD are not supported.
"#;

const SAFETY: &str = r#"SAFETY AND PRIVACY

What it protects against
  * Changing the system without your say. Every change goes through the
    gate: without --yes it asks, with nobody to ask it refuses (exit 3).
    An AI assistant connected through MCP cannot apply anything unless
    the machine's administrator enabled that: `yapacc docs ai`.
  * Surprises in what is run. The plan, the package manager's own
    simulation and the exact command are printed before the question.
  * Names that are not names. Package names are validated and passed to
    the package manager as separate arguments after `--`. No shell
    command line is built from package data, log contents or network
    answers.
  * Half-applied undos. `undo` is all or nothing.
  * Removing the ground you stand on. An undo that would remove the
    running kernel is refused.
  * Hostile build recipes (the AUR on Arch). The PKGBUILD is shown for
    review before anything is built; the build runs in a bubblewrap
    sandbox with a throwaway home directory, an empty /run, a cleared
    environment, and the network only while sources are fetched.
  * Terminal tricks. Escape sequences in package names, file names and
    log lines are neutralised before they are printed.

What it does NOT protect against
  * A package you agreed to install. The package manager then runs as
    root and package scripts do what they do. yapacc is not a sandbox
    for native packages.
  * Lost configuration or data. `undo` restores package versions only.
  * A compromised repository. yapacc relies on your package manager's
    signature checks and adds none for native packages.
  * Weakly verified archive downloads during `undo`: snapshot.debian.org
    files are checked against a recorded hash, not a signature; Launchpad
    downloads are not verified. The plan says which applies.
  * A determined build. The build sandbox has no seccomp filter and no
    resource limits; it is not a virtual machine.
  * Something already running as you. The configuration file and the
    ledger live in your home directory.

What leaves the machine
  No telemetry, no update check of its own, no listening socket. These
  are the only cases in which a server is contacted:
  names NAME                 repology.org: the package name
  undo                       the distribution's archive
                             (archive.archlinux.org, snapshot.debian.org,
                             launchpad.net): the name and version of each
                             package it needs
  briefing                   on Arch: the news feed, and the mirrors
                             through `checkupdates`; not with --offline
  check, get, search --aur, query, news, the aur commands, on Arch
                             aur.archlinux.org or archlinux.org: the
                             names of packages that are in no repository;
                             the name or query you give
  install, remove, upgrade, refresh
                             your repositories, through your package
                             manager
  fixes push, fixes pull     the git remote you configured
  anything you give a URL    that URL
  Every other read command answers from what is on disk.

What it writes
  Read commands write nothing. The places yapacc writes to:
  ~/.cache/yapacc/           old packages while an undo runs (deleted
                             when it ends), the throwaway home of a
                             sandboxed build, the file of `index build`
  ~/.local/state/yapacc/ledger/   (as root: /var/lib/yapacc/ledger/)
                             when you install something outside the
                             package manager through yapacc
  ~/.local/share/yapacc/     isolated installs, recipes and recorded
                             build fixes, if you use those commands
  ~/Applications/yapacc/     AppImages, if you install one
  ./NAME-aur-build           the build directory of `install aur`
  a file you name            `plan … --out FILE` and the like
  systemd timer units        only if you run `monitor daemon-install`
  It never writes its configuration file, never edits your shell startup
  files, and never adds a hook to your package manager.
"#;

const EXIT_CODES: &str = r#"EXIT CODES

  0  ok           the command did what was asked
  1  failed       the operation ran and failed
  2  usage        bad arguments, a name that is not a package name, --json
                  on a command without a JSON form, or a configuration
                  file that does not parse
  3  refused      a gate or a guard said no; nothing was changed
  4  not found    the package, file owner, record, topic or plan does not
                  exist; a package to remove is not installed; `get
                  --explain` chose no source
  5  integrity    a hash, signature or chain check failed
  6  unavailable  something this command needs is not available: a tool
                  or optional component that is not installed, a file
                  this user may not read (run as root), a feature this
                  build lacks, or a command that does not exist for this
                  package manager

The message goes to standard error as `error: …`.

The two that matter most in a script are 3 and 6. A 3 means the system is
exactly as it was, by decision. A 6 means the machine lacks something, and
the message names it.

Some specialist commands on Arch still end with 1 for every failure.
"#;

const JSON_HEAD: &str = r#"JSON OUTPUT (--json)

--json is a global option and goes before the command:

  yapacc --json platform
  yapacc --json why bash
  yapacc --json timeline --since 2026-11-01
  yapacc --json list --explicit
  yapacc --json doctor

A command that has no JSON form prints nothing and exits 2 with
"no JSON output for this command". It never prints text with a success
status. These commands have a JSON form:

"#;

const JSON_TAIL: &str = r#"
"Arch only" commands exit 6 on another package manager, with or without
--json.

With --json, what a command has to add in words (a source it could not
read, "no repository metadata on disk") is printed to standard error as
`note: …` lines; standard output holds the JSON alone.

Fields are only ever added. Treat a key you do not know as ignorable, and
do not depend on the order of keys.

Shapes
  platform    one object; native_pkg_manager is "pacman", "apt", "dnf",
              "zypper", "apk", "xbps" or "pkg"
  why         one object: query, native {manager, name, version, reason,
              required_by[], …}
  timeline    array of {ts, source, action, name, old_version,
              new_version}; ts is Unix seconds
  doctor      array of {name, state, detail}; state is ok, warn, fail
              or n/a
  clean       array of {what, size_bytes, count, safety, command,
              detail[]}
  get         with --explain: {name, candidates[{lane, source, version,
              found, note}], chosen, runtime}
  inspect owns    {path, owner}
  inspect files   array of paths
  list, search    array of objects; `name` and `version` are in each, the
                  other keys differ between pacman and the other managers
  check           array of {name, old_version, new_version, repo} on
                  every package manager. `repo` names the repository or
                  origins where the manager gives them (apt gives several,
                  separated by ", "); on pacman it is "" for a repository
                  update and "aur" for an AUR update
  info            pacman: one object with the package's fields;
                  elsewhere: the object of `why`
  inspect orphans pacman: array of package objects; elsewhere: array of
                  names
"#;

const CONFIG: &str = r#"CONFIGURATION

yapacc needs no configuration. Without a file the defaults below apply.

File: ~/.config/yapacc/config.toml, or $XDG_CONFIG_HOME/yapacc/config.toml.
Another file for one run: --config PATH, before the command.
  yapacc config
prints the effective values and the file they came from. yapacc never
writes this file. An example with every key ships as yapacc.example.toml.

  * A file that does not parse, or a known key with an invalid value, is
    an error (exit 2), never a silent fallback to the defaults.
  * A section or key this version does not know is ignored with one
    warning line each on standard error; `yapacc config` lists them under
    unknown_keys.
  * Paths are absolute or start with ~/. Any other path is an error.

Keys and defaults
  [safety]
    snapshot = "off"             off | auto | auto-best-effort: a snapshot
                                 before install, remove, upgrade and undo
    snapshot_provider = "auto"   auto | snapper | timeshift | zfs | none
    pretest_enabled = true       the pre-check step of `safety apply`
    require_snapshot_before_upgrade = false
                                 read only by `safety apply`, for the
                                 operations arch-upgrade and arch-update
    rollback_on_failure = false  accepted; no command reads it
  [mcp]
    apply = "plan"               plan | allow: what an AI assistant may
                                 do. "allow" counts only in a file owned
                                 by root: `yapacc docs ai`
  [install]
    order = ["repo", "aur", "flatpak", "recipe"]
                                 the order `get` tries sources (Arch)
    runtime_default = "container"
                                 copied into the JSON form of `get` as
                                 `runtime`; nothing else reads it
  [build]                        builds of third-party recipes (Arch)
    sandbox = "require"          require | prefer | off
    network = "fetch-only"       fetch-only | always
  [repair]
    enabled = true               classify a failed build and retry
    max_attempts = 3             builds in total, the first one included
  [fixes]
    remote                       unset; the git URL of your fixes store
  [ledger]
    witnesses = []               "journald" or file paths that receive a
                                 copy of the ledger head
  [paths]
    foreign_root                 unset; where isolated installs go
    ledger_user, ledger_system   unset; where the two ledgers live
  [foreign]
    expose = "attributed"        attributed | none
    allow_unsigned_repos = false
  [plugins]
    dirs = []                    extra directories searched for adapters
    disabled = []                adapter names to ignore
  [general]                      accepted; it has no key

Environment
  XDG_CONFIG_HOME, XDG_CACHE_HOME, XDG_STATE_HOME, XDG_DATA_HOME
                         where the configuration, caches, the ledger and
                         data live
  LC_ALL, LC_MESSAGES, LANG   the letters of the [y/N] question
  COLUMNS                the width `list` cuts descriptions to
  NO_COLOR               honoured by the interactive interface; the
                         command line prints no colour codes
  YAPACC_FOREIGN_ROOT    where isolated installs go; wins over [paths]
  YAPACC_LIBEXEC         a directory searched first for yapacc-tui and
                         yapacc-mcp
  YAPACC_CONTRIB_DIR     a directory with replacements for the helper
                         scripts of `safety apply`; a script there is used
                         only when it is owned by root or by you
"#;

const GET: &str = r#"CHOOSING A SOURCE (yapacc get NAME)

`get` looks for NAME in the sources this machine has and installs from the
first that has it. With the default order the distribution's own package
wins when it exists. --explain shows the decision table and stops.

  yapacc get ripgrep --explain
  yapacc get ripgrep --dryrun

Everywhere
  repo      the distribution's repositories: installed through the native
            package manager, behind the gate, like `install repo`.
  flatpak   listed in the table when flatpak is installed. Off Arch it is
            never chosen for you: the message names the command
            (`yapacc install flatpak org.example.App`).

On Arch as well
  aur       built from the AUR behind the PKGBUILD review:
            `yapacc docs aur`. Asking sends the name to aur.archlinux.org.
  flatpak   installed system-wide when nothing earlier has the name.
  recipe    last resort: a PKGBUILD is generated (from --source-url when
            given), shown for review, built as an ordinary user in the
            sandbox, and the result is unpacked as an isolated install.
            pacman's database is not touched. An edited PKGBUILD under
            ~/.local/share/yapacc/recipes is kept and reused. With
            --dryrun the recipe is printed and nothing is written.
  brew      off by default: a Homebrew formula built through the recipe
            lane. Only with --from brew, or when added to the order. It
            fetches the formula from raw.githubusercontent.com.
  The order is `[install] order` in the configuration; the default is
  repo, aur, flatpak, recipe. The first source in that order that has
  the name wins, and recipe is always tried last.

--from LANE uses one source only: repo or flatpak everywhere; aur, brew
or recipe on Arch (exit 6 elsewhere). Any other value is a usage error
(exit 2).

The review of a recipe is not answered by --yes: it needs --no-review, or
your answer at the terminal. A non-free licence needs --non-free.

  yapacc --json get ripgrep --explain
prints the decision as JSON. `get` without --explain has no JSON form.

When no source is chosen for a name, `get` exits 4, with --explain too:
the table (or the JSON, with "chosen": null) is printed first. Without
repository metadata on disk the repo row says "not known" and why.
"#;

const INSPECTION: &str = r#"INSPECTING PACKAGES

Read-only, from the package manager's database on disk.

Everywhere
  list                   installed packages. --explicit: only what you
                         asked for. --limit N.
  info PKG               details of one installed package (exit 4 when it
                         is not installed)
  inspect owns PATH      which package owns this file; PATH is absolute
                         (exit 4 when no package owns it)
  inspect files PKG      the files of a package, one per line
  inspect orphans        dependencies nothing needs any more; always
                         empty on apk, which removes them itself
  check                  pending updates, as of the last refresh; old
                         and new version of each

On a terminal `list` cuts descriptions at a word to fit the width; names,
versions and sizes are never cut. Into a pipe every line is whole.

Arch only (exit 6 elsewhere)
  list --foreign             packages that are in no repository
  list --include-external    also what the ledger records
  inspect leaves             packages you installed that nothing needs
  inspect facets PKG         how a package is classified
  inspect deps PKG           dependency tree (pactree); --depth N
  inspect rdepends PKG       what depends on it, as a tree; --depth N
  inspect file-search PATH   which repository package provides a file;
                             --refresh runs `pacman -Fy` first
  inspect groups             package groups, or the members of one
  inspect pacdiff            .pacnew, .pacsave and .orig files under /etc;
                             --root DIR scans another directory
  inspect alpm list          the same reads through libalpm, in a build
                             with the `alpm` feature; any other build
                             names the command that gives the same
                             answer and exits 6

To remove orphans with the list shown and a question asked:
  yapacc remove orphans --dryrun
"#;

const SEARCH: &str = r#"SEARCH AND THE SOFTWARE TREE

  yapacc search ripgrep
  yapacc search ripgrep --limit 5

`search` looks in the repository indexes already on disk. It refreshes
nothing and contacts nothing. Best matches first: the exact name, then
names that contain the query, then descriptions.

On Arch
  --aur          search the AUR instead; --all: the repositories and the
                 AUR. Both send the query to aur.archlinux.org.
  --category C, --kind K, --type T
                 keep only results whose category, kind, or package type
                 or size class contains the text
  When a query finds nothing, it is expanded once, offline, from a fixed
  table of synonyms and categories, and the expansion is printed:
  yapacc search "draw in the terminal"
  query NAME     repository and AUR metadata for one package; --repo or
                 --aur limits it to one
  index build, index search QUERY
                 a file index of the repositories for repeated searches,
                 kept under ~/.cache/yapacc unless --path is given
On other systems these options exit 6.

The software tree
  yapacc tree
  yapacc tree graphics
lists 18 categories and their subcategories, from a table built into the
program. On Arch a subcategory lists its packages (exit 6 elsewhere):
  yapacc tree graphics.terminal-graphics --installed
A file ~/.config/yapacc/taxonomy.toml, when present, replaces the built-in
table. A file that cannot be used is named once, with the reason, and the
built-in table applies.
"#;

const LEDGER: &str = r#"THE LEDGER

The ledger records what yapacc installed OUTSIDE the native package
manager: flatpaks, AppImages and isolated installs. Native packages are
never recorded there: the package manager's own log is their record.

Where
  user     ~/.local/state/yapacc/ledger/   (journal.jsonl, state.json)
  system   /var/lib/yapacc/ledger/         (when yapacc runs as root)
  --scope user|system chooses one; --path DIR reads another directory.
  `[paths] ledger_user` and `ledger_system` move them.

How it is kept
  * An append-only journal; each event carries the SHA-256 of the one
    before it. state.json is only a cache: every command answers from
    the journal, replayed.
  * A kernel file lock around changes; a second writer waits up to 10
    seconds, then fails.
  * User ledgers are created with modes 0700 and 0600.

Commands, all read-only (installs and removals write the ledger)
  ledger status      where it is, how many records, its head, its
                     witnesses
  ledger list        the records
  ledger head        the head as SEQ:HASH
  ledger verify      check the chain; --anchor SEQ:HASH compares it with
                     a head you kept
  ledger audit       overlapping paths between records, name collisions,
                     missing dependencies (best effort)
  ledger conflicts PATH…
                     whether paths belong to a recorded install, exist on
                     disk, or are owned by a pacman package
  external list      the records of both ledgers
  external audit     the audit over both ledgers

`verify` and `status` exit 5 when the cache differs from the journal. A
state file without a journal is exit 5 as well. Events written with the
old non-cryptographic checksum make `verify` fail unless --accept-legacy
is given, which reports them as unverified.

What the chain proves
  On its own it detects accidental corruption. Someone who can write the
  journal can rewrite the whole chain. Tampering is detected only against
  something the ledger's owner cannot rewrite: a witness
  (`[ledger] witnesses`: "journald" or a file, ideally on another
  machine), or a head from `ledger head` that you kept elsewhere and pass
  back with --anchor.

  yapacc ledger status
  yapacc --json ledger list
"#;

const FOREIGN: &str = r#"ISOLATED INSTALLS (deb, rpm, snap)

`install foreign` unpacks a package made for another distribution into a
directory of its own. Nothing is installed into the system, the package
manager is not involved, and no maintainer script is run.

Where the files go
  as root        /var/lib/yapacc/isolated/foreign/FORMAT/NAME
  otherwise      ~/.local/share/yapacc/isolated/foreign/FORMAT/NAME
  `[paths] foreign_root` or YAPACC_FOREIGN_ROOT moves the root; --prefix
  DIR chooses the directory of one install.

Programs
  With --expose attributed (the default, `[foreign] expose`) each program
  found is linked in the install's own bin/ directory under a name that
  says where it came from: fgn.FORMAT.NAME--PROGRAM, for example
  fgn.deb.hello--hello. Nothing is added to PATH. --expose none links
  nothing.

One package (all gated; --dryrun shows the plan)
  foreign info --format F --source FILE     read its metadata
  install foreign --name N --format F --source FILE
                                             --replace: over an install
                                             of the same name
  foreign list                               what is installed, with IDs
  remove foreign --id ID
  update foreign --id ID --source FILE

Several packages as one install, in dependency order; a failure removes
what this run installed
  install tx --format F FILE-OR-URL…         files or URLs you give; no
                                             signature or hash is checked
  install tx-apt, tx-apt-sources, tx-rpm     the packages you name and
                                             their dependencies found in
                                             a repository, with signature
                                             and hashes checked:
                                             `yapacc docs repo-trust`
  install tx-apt-repo, tx-rpm-repo           the same, from an index you
                                             give; no signature
The matching `plan foreign` and `plan tx…` commands print the plan and
install nothing (Arch only).

What extraction refuses or drops
  setuid and setgid bits and foreign owners are dropped; symbolic links
  that lead out of the install are not followed. A package whose section
  or licence field marks it non-free asks about the licence; --yes does
  not answer that, --non-free does.

Limits
  Only the files are unpacked. Libraries the program needs must be on
  the system or in the set installed with it: a dependency that is in
  neither is reported, not fetched. No services, no confinement.

To run another distribution's software with its own dependencies, use a
container: `yapacc install container --image debian:stable --dryrun`
shows the distrobox command it would run.
"#;

const REPO_TRUST: &str = r#"REPOSITORY TRUST (isolated installs from a repository)

`install tx-apt`, `tx-apt-sources` and `tx-rpm` take packages from an APT
or RPM repository. What is checked:

Signature
  --keyring FILE   the repository's signed index is checked with gpgv
                   against this keyring: InRelease, or Release with
                   Release.gpg, for APT; repomd.xml with repomd.xml.asc
                   for RPM. FILE must be a binary keyring. Without gpgv
                   the command exits 6.
  no keyring       the install is refused, unless --allow-unsigned is
                   given or `[foreign] allow_unsigned_repos = true`.
  A signature that does not verify stops the install, also with
  --allow-unsigned. Of a signed InRelease only the signed part is read.

Hashes
  Every index and every package is compared with the SHA-256 (or
  stronger) recorded in the index above it. A mismatch is exit 5. A
  repository that offers only MD5 or SHA-1 is refused.

`install tx-apt-repo` and `install tx-rpm-repo` take an index you give
directly: no signature is checked, and an index or packages at a remote
location need --allow-unsigned.

`install tx` with files or URLs checks nothing: each file is installed
as it was given or downloaded, and the output says so.

A plan or a dry run reads the repository index without a keyring: it
installs nothing.

Example. On Debian the keyring is in the debian-archive-keyring package.
With --dryrun the index is read and nothing is installed:
  yapacc install tx-apt --base-url http://deb.debian.org/debian \
      --suite stable --component main --name hello \
      --keyring /usr/share/keyrings/debian-archive-keyring.gpg --dryrun
"#;

const AUR: &str = r#"THE AUR (Arch only)

Reading needs no helper program. Each of these contacts aur.archlinux.org
and sends the name or query you give:
  search QUERY --aur, search QUERY --all, query NAME --aur
  aur deps PKG…      the build order: what must be built, what the
                     repositories provide, what is installed, what is
                     missing
  aur pkgbuild PKG   print the PKGBUILD
  aur review PKG     the metadata, the PKGBUILD and the install plan
`query NAME --aur`, `aur pkgbuild` and `install aur` exit 4 for a name the
AUR does not have, and 6 when aur.archlinux.org cannot be reached.
`check` compares the installed packages that are in no repository with
their AUR versions, and sends their names to do so. With no repository
database on disk it does not: which packages those are cannot be told.

Building: `install aur PKG`
  0. ask the AUR whether it has PKG (not with --url): a name it does not
     have is refused before anything is planned.
  1. clone into ./PKG-aur-build (--dir DIR, or --url URL for another
     git repository)
  2. show the whole PKGBUILD and ask. This review is not answered by
     --yes: without a terminal it needs --no-review, otherwise it is a
     refusal (exit 3).
  3. build with makepkg in a bubblewrap sandbox: a throwaway home
     directory, an empty /run, a cleared environment, the network only
     while sources are fetched and prepare() runs.
     `[build] sandbox = "require"` (the default) refuses to build when
     the sandbox does not work; --no-sandbox builds without it;
     --build-net keeps the network for the whole build.
  4. the gate, then `pacman -U` of the packages this build produced.
  --deps builds the missing AUR dependencies first, in order, behind one
  review of all their PKGBUILDs. As root, --build-user USER is required:
  makepkg refuses to run as root.

  yapacc install aur yay --dryrun

`install repo --aur` and `upgrade --aur` go through yay. yay asks its own
questions: yapacc never answers them, and refuses when nobody can.

A failed build is classified and may be retried: `yapacc docs autorepair`.

Always read a PKGBUILD before building it. The sandbox keeps a build away
from your files; it has no seccomp filter and no resource limits.
"#;

const AUTOREPAIR: &str = r#"BUILD REPAIR

Only Arch has builds to repair; `repair` and `fixes` run on any system.

A build started by `install aur` or by the recipe lane of `get` that
fails is classified from its output, repaired where a repair is known,
and tried again: at most `[repair] max_attempts` builds in total (3 by
default). --no-autorepair turns this off for one command.

  repair LOG    classify the failures in any build log (`-` reads
                standard input). It changes nothing. Each finding is
                marked "yapacc can try" (a repair below exists) or
                "needs you".

What is recognised
  missing dependency, missing build tool, checksum mismatch, source gone
  (404), missing PGP key, corrupt archive, makepkg run as root, software
  for macOS only, and other build errors.

The repairs
  * Missing build dependencies found in the repositories are listed and
    installed only after a yes at the terminal, or with --repair-deps.
    Without a terminal they are reported and the build stops.
  * A checksum mismatch is retried once. The checksums are rewritten only
    with --allow-checksum-refresh: it changes the PKGBUILD in the build
    directory and prints the old and the new hashes. That gives up the
    tamper check, so it is never done on its own.
  * Another build error opens $EDITOR on the recipe at a terminal;
    without one it is reported.

Recorded fixes
  A successful repair is saved under ~/.local/share/yapacc/fixes, unless
  --no-fix-record is given.
  fixes list           what is recorded
  fixes push           push to the git remote in `[fixes] remote` (gated)
  fixes pull           fetch from it and merge into the local store
  There is no default remote: without `[fixes] remote` nothing is pushed
  or pulled.

  yapacc fixes list
"#;

const PLANS: &str = r#"SAVED PLANS AND STAGED CHANGES (Arch only)

A plan in a file
  plan install PKG… --out FILE
  plan remove PKG… --out FILE
  plan upgrade --out FILE
  Each prints the plan and changes nothing; --out saves it as JSON, and
  --json prints that JSON.
  plan apply FILE
  applies a saved plan through the gate (--dryrun, --yes as usual). The
  file is treated as data from outside:
  * the operation, the source and the package names are validated;
  * the command stored in the file is ignored: it is built again;
  * a plan older than 48 hours, or dated in the future, is refused
    unless --force is given;
  * a missing file is exit 4; a refusal is exit 3.

  yapacc plan install ripgrep --out /tmp/ripgrep-plan.json
  yapacc plan apply /tmp/ripgrep-plan.json --dryrun

Plans that are only printed
  plan flatpak, flatpak-remove, flatpak-update, appimage, foreign, tx,
  tx-apt, tx-apt-repo, tx-apt-sources, tx-rpm, tx-rpm-repo, container
  print what the matching install, remove or update command would do.
  plan cross NAME --format F says which commands handle a format.
  plan safety prints the stages below for this host.

A staged change: safety apply
  safety apply --op OP TARGET…
  OP is arch-install, arch-remove, arch-update, arch-upgrade,
  flatpak-install, flatpak-remove or flatpak-update.
  1. plan         the preview of the change
  2. pre-check    read-only checks of the transaction; skipped with
                  --skip-pretest or `[safety] pretest_enabled = false`.
                  A failure stops here with nothing changed.
  3. gate         the question, as for any change
  4. snapshot     only with --snapshot-tag TAG: a read-only btrfs
                  snapshot of /, through a helper built into yapacc,
                  taken after you consent. If it cannot be taken,
                  nothing is changed.
  5. apply        the change
  6. health       only with --health-check COMMAND: a non-zero exit
                  counts as a failed change
  7. on failure   with --auto-rollback, packages this run installed are
                  removed again with `pacman -R`, and only when the
                  failure came after pacman ran. Nothing else is rolled
                  back.
  With --dryrun no snapshot is taken and no package command is run.
  A snapshot is never restored by yapacc.

  yapacc safety apply --op arch-install tree --dryrun
"#;

const DAEMON: &str = r#"SCHEDULED CHECKS

Nothing runs in the background unless you install it yourself. Both
commands exist only on Arch: on another system they say so and exit 6.

  monitor check   one report of pending updates: the repositories (as of
                  the last refresh), the AUR, flatpak. It changes nothing
                  and applies nothing. It sends the names of packages
                  that are in no repository to aur.archlinux.org.

  monitor daemon-install
                  writes yapacc-monitor.service and yapacc-monitor.timer,
                  from templates built into the program, and enables the
                  timer with systemctl. Gated: --dryrun shows the files
                  and the command. System-wide by default
                  (/etc/systemd/system, needs root); --user installs for
                  your user (~/.config/systemd/user).
                  The timer runs `monitor check` ten minutes after boot
                  and every thirty minutes after that. It never applies
                  an update.

  yapacc monitor daemon-install --user --dryrun

To remove it: disable the timer with systemctl and delete the two files.
"#;

const PLUGINS: &str = r#"PLUGIN ADAPTERS

An adapter is a separate program that tells yapacc whether a source of
its own has a package. yapacc runs it and reads its answer; it never
loads code from it.

  yapacc-source-NAME    answers for a source
  yapacc-runtime-NAME   says whether a runtime is available

Protocol, version 1
  --protocol     prints 1
  --probe NAME   prints {"found":bool,"source":str,"version":str,
                 "note":str}                         (source adapters)
  --available    prints true or false                (runtime adapters)

Where adapters are looked for, first match wins
  the directories in `[plugins] dirs`, then
  ~/.local/share/yapacc/plugins, then PATH.
  `[plugins] disabled` lists names to ignore.

  yapacc plugins
lists the adapters found, whether each speaks the protocol, and for a
runtime whether it is available.

What an adapter can and cannot do
  * A source adapter is asked by `get` on Arch, and its answer appears as
    a line of the decision table.
  * yapacc installs nothing through an adapter: when an adapter's source
    is the one chosen, `get` says so and stops.
  * Adapters are asked after the built-in sources of `[install] order`,
    so with the default order a package in the repositories still wins.
  * The name is passed as one argument, after it was validated.
  * An adapter runs with your privileges, like any program on PATH: only
    install adapters you trust.
"#;

const TUI: &str = r#"THE INTERACTIVE INTERFACE (yapacc i)

`yapacc i` opens a full-screen interface in the terminal. It is the
optional program yapacc-tui: when that is not installed, or there is no
terminal, the command says so and exits 6.

It shows the same reports as the command line and adds none of its own.
It opens on a home screen, a menu of questions (what changed, what the
next upgrade will do, what can be cleaned up, what works here). The
browsing tabs are 1 Installed, 2 Search, 3 External, 4 Docs, 5 Tree and
6 Run.

Everywhere
  ?            this help
  Esc or q     back (q on the home screen quits)
  Ctrl-C       quit at once
  j/k, arrows  move      PgUp/PgDn  a page      g/G  first / last

Home
  Enter opens the highlighted line; each line also has its own letter.

Report panes (timeline, briefing, clean, doctor…)
  /            filter by text       r  read again
  Enter        the whole line, when it is cut
  u            (upgrade briefing) plan the upgrade
  o            (clean up) plan the removal of unneeded packages

Browse (Tab or 1-6 changes tab)
  Installed    / filter  space select  i why  d deps  x remove  e mark explicit
  Search       type, Enter  a install  i info  / edit the query
               A  also search the AUR (Arch family; the query is then sent
                  to aur.archlinux.org), A again to stop
  External     Enter details  x remove
  Docs         Enter opens a topic   Tree  Enter opens a category
  Run          any yapacc arguments; output is shown, nothing can prompt.
               Not run from here: --yes, --noconfirm, i, mcp

Changing the system
  Every change is first shown as a plan (a dry run): nothing has happened
  yet. Move to [ Apply ] and press Enter to leave this interface and run
  the command in your terminal, where yapacc itself asks you to confirm.
  This interface never answers that question for you. Ctrl-C there stops
  that command and brings you back here.

The sudo or doas password prompt appears in the terminal as well. Run the
interface as yourself, not as root. --config PATH is passed on to every
command it runs.
"#;

const AI: &str = r#"AI ASSISTANTS (yapacc mcp)

`yapacc mcp` runs a server that speaks the Model Context Protocol
(revision 2024-11-05) over standard input and output. It is the optional
program yapacc-mcp; without it the command exits 6. yapacc contains no AI
and holds no API key: the assistant and the model are whatever you use.

What an assistant can do
  By default it can read and propose. It cannot apply.
  When it asks for an install, a removal, an upgrade or an undo, the
  answer begins with one of:
  PLAN ONLY — nothing was changed.     the plan, and the command for YOU
                                       to run in your own terminal
  PLAN ONLY (confirm=false) — nothing was changed.
                                       applying is allowed, and the call
                                       did not say `confirm: true`
  PLAN FAILED — nothing was changed.   the plan could not be made: a
                                       package no source has, a package
                                       to remove that is not installed, a
                                       failed simulation
  APPLIED: …                           only when applying is allowed
  FAILED: …                            the change was tried and failed

When applying is allowed
  Both of these are needed:
  1. `[mcp] apply = "allow"` in a configuration file that is owned by
     root and not writable by group or others, given to the server with
     --config (for example /etc/yapacc/config.toml). From any other file
     the server says so when it starts and stays in plan mode: an
     assistant that can edit your files cannot give itself this right.
  2. `confirm: true` from the assistant, in that call.
  `confirm: true` alone changes nothing. Even then the change runs
  without a terminal: sudo or doas is called with -n, so it works only
  where that user may run the command without a password.
  Nothing records that a change was asked for by an assistant: the
  package manager's log shows it like any other.

Connecting it
  Point the assistant at the command `yapacc mcp`. For a client with a
  JSON configuration:
  { "mcpServers": { "yapacc": { "command": "yapacc", "args": ["mcp"] } } }

The 21 tools
  read-only
    yapacc_platform         what kind of machine this is
    yapacc_doctor           what works on it
    yapacc_why              why a package is installed
    yapacc_timeline         package changes, newest first
    yapacc_briefing         what the next upgrade will do
    yapacc_check            pending updates
    yapacc_clean_report     what can be reclaimed
    yapacc_restart_report   what must be restarted
    yapacc_pending_configs  configuration files left unmerged
    yapacc_snapshots        existing filesystem snapshots
    yapacc_search           find a package
    yapacc_resolve          which source `get` would choose
    yapacc_plan             the plan of an install
    yapacc_names            names on other distributions (repology.org)
    yapacc_ledger           what the ledger records
    yapacc_capabilities     what this host supports
    yapacc_news             Arch Linux news; says so on other systems
  staged: a plan and a command, unless applying is allowed
    yapacc_install, yapacc_remove, yapacc_upgrade, yapacc_undo_last

Every string an assistant sends is validated before it reaches a command
line. The server opens no socket. A tool contacts a server exactly when
the matching command does: `yapacc docs safety` lists those.
"#;

/// The `yapacc …` command lines of a text: indented lines (or lines after a
/// `$ ` prompt) that start with `yapacc `, and spans between backticks that
/// do. A
/// line ending in `\` continues on the next one. Comments (`  # …`), pipes
/// and redirections are cut off. Used by the tests here and in
/// tests/cli_smoke.rs, so that no command line in the documentation can go
/// stale.
pub fn command_lines(text: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut joined = String::new();
    let mut indented = false;
    for raw in text.lines() {
        let line = raw.trim();
        if joined.is_empty() {
            indented = raw.starts_with(' ') || line.starts_with("$ ");
        }
        if let Some(head) = line.strip_suffix('\\') {
            joined.push_str(head);
            continue;
        }
        let whole = format!("{joined}{line}");
        joined.clear();
        let start = whole.strip_prefix("$ ").unwrap_or(&whole);
        // a sentence may begin with the program's name; a command is indented
        if indented && start.starts_with("yapacc ") {
            found.push(start.to_string());
            continue;
        }
        // spans between backticks; odd pieces of the split are inside
        for (i, span) in whole.split('`').enumerate() {
            if i % 2 == 1 && span.starts_with("yapacc ") {
                found.push(span.to_string());
            }
        }
    }
    found
        .into_iter()
        .map(|l| {
            let mut l = l.as_str();
            for cut in [" | ", " > ", "  # ", " # "] {
                if let Some(at) = l.find(cut) {
                    l = &l[..at];
                }
            }
            l.trim().to_string()
        })
        .collect()
}

/// Split a command line into arguments: spaces separate, double quotes
/// group. The words that stand for a value (`COMMAND`, `TOPIC`) are replaced
/// by one, so that the line can be parsed.
pub fn command_words(line: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut quoted = false;
    let mut any = false;
    for c in line.chars() {
        match c {
            '"' => {
                quoted = !quoted;
                any = true;
            }
            c if c.is_whitespace() && !quoted => {
                if any {
                    words.push(std::mem::take(&mut word));
                    any = false;
                }
            }
            c => {
                word.push(c);
                any = true;
            }
        }
    }
    if any {
        words.push(word);
    }
    words
        .into_iter()
        .map(|w| match w.as_str() {
            "COMMAND" => "why".to_string(),
            "TOPIC" => "daily".to_string(),
            _ => w,
        })
        .collect()
}

/// The command lines of the EXAMPLES section of the manual page, with the
/// roff escapes undone.
pub fn man_examples() -> Vec<String> {
    let Some(start) = MAN_PAGE.find(".SH EXAMPLES") else {
        return Vec::new();
    };
    let section = &MAN_PAGE[start + ".SH EXAMPLES".len()..];
    let section = section.split("\n.SH ").next().unwrap_or(section);
    // the examples are set flush left between `.RS` and `.RE`
    let plain: String = section
        .replace("\\-", "-")
        .replace("\\&", "")
        .lines()
        .map(|l| format!("  {l}\n"))
        .collect();
    command_lines(&plain)
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::{CommandFactory, Parser};

    fn all_texts() -> Vec<(String, String)> {
        let mut texts: Vec<(String, String)> = topics()
            .iter()
            .map(|(slug, _, _)| (slug.to_string(), render(slug).expect(slug)))
            .collect();
        texts.push(("(index)".into(), index()));
        texts
    }

    #[test]
    fn every_topic_renders_and_is_listed() {
        let idx = index();
        for (slug, title, about) in topics() {
            let doc = render(slug).unwrap_or_else(|| panic!("topic {slug} does not render"));
            assert!(doc.len() > 200, "topic {slug} is suspiciously short");
            let first = doc.split_whitespace().next().unwrap_or("");
            assert!(
                first.chars().any(char::is_alphabetic) && !first.chars().any(char::is_lowercase),
                "topic {slug} should start with a heading in capitals: {first:?}"
            );
            assert!(
                doc.ends_with('\n'),
                "topic {slug} should end with a newline"
            );
            assert!(idx.contains(&format!("  {slug:<16} {title}\n")), "{slug}");
            assert!(idx.contains(about), "index lacks the summary of {slug}");
        }
        // no topic is listed twice
        let mut slugs: Vec<&str> = topics().iter().map(|t| t.0).collect();
        slugs.sort_unstable();
        slugs.dedup();
        assert_eq!(slugs.len(), topics().len());
    }

    #[test]
    fn aliases_lead_to_listed_topics() {
        for (alias, slug) in ALIASES {
            assert!(topics().iter().any(|t| t.0 == *slug), "{alias} -> {slug}");
            assert!(!topics().iter().any(|t| t.0 == *alias), "{alias} is a slug");
            assert_eq!(render(alias), render(slug), "{alias}");
        }
        // the MCP server tells its users to read `yapacc docs mcp`
        assert!(render("mcp").unwrap().starts_with("AI ASSISTANTS"));
        assert!(render("no-such-topic").is_none());
    }

    #[test]
    fn no_line_is_wider_than_80_columns() {
        for (slug, text) in all_texts() {
            for line in text.lines() {
                assert!(
                    line.chars().count() <= 80,
                    "{slug}: {} columns: {line}",
                    line.chars().count()
                );
                assert_eq!(line, line.trim_end(), "{slug}: trailing space: {line:?}");
            }
        }
    }

    #[test]
    fn nothing_that_does_not_exist_is_mentioned() {
        let gone = [
            "sepio",
            "armada",
            "eyes-engine",
            "job card",
            "not yet implemented",
            "not implemented",
            "upcoming",
            "roadmap",
            "dry_run_default",
            "confirm_actions",
            "DESIGN_",
            "opencode",
            "package_cleaner",
            "store gc",
            "safe-apply",
            "apply-plan",
            "install-plan",
            "aur-deps",
            "aur-install",
            "monitor-check",
            "cache-clean",
            "foreign-tx",
            "pacdiff-list",
            "safety pipeline",
            "any later version",
        ];
        let mut texts = all_texts();
        texts.push(("(manual page)".into(), MAN_PAGE.replace("\\-", "-")));
        for (slug, text) in texts {
            let lower = text.to_lowercase();
            for word in gone {
                assert!(
                    !lower.contains(&word.to_lowercase()),
                    "{slug} mentions {word:?}"
                );
            }
            // `package scaffold` is a command; nothing else is a scaffold
            assert_eq!(
                lower.matches("scaffold").count(),
                lower.matches("package scaffold").count(),
                "{slug} says scaffold"
            );
        }
    }

    #[test]
    fn every_command_line_in_the_topics_and_the_manual_examples_parses() {
        let mut lines: Vec<(String, String)> = Vec::new();
        for (slug, text) in all_texts() {
            lines.extend(command_lines(&text).into_iter().map(|l| (slug.clone(), l)));
        }
        let examples = man_examples();
        assert!(examples.len() >= 8, "manual page EXAMPLES: {examples:?}");
        lines.extend(examples.into_iter().map(|l| ("(manual page)".into(), l)));
        assert!(lines.len() > 60, "only {} command lines found", lines.len());
        for (slug, line) in lines {
            let words = command_words(&line);
            if let Err(e) = crate::cli::Cli::try_parse_from(&words) {
                // --help and --version are "errors" that print and succeed
                assert!(!e.use_stderr(), "{slug}: `{line}` does not parse:\n{e}");
            }
        }
    }

    #[test]
    fn the_splitter_handles_prompts_comments_pipes_quotes_and_continuations() {
        let text = "  yapacc why bash   # a comment\n$ yapacc docs --man | man -l -\nrun `yapacc list --explicit` or `yapacc-tui`\n  yapacc install tx-apt --name a \\\n      --suite stable\n  yapacc completions bash > file\nyapacc is a sentence here, not a command\n";
        assert_eq!(
            command_lines(text),
            [
                "yapacc why bash",
                "yapacc docs --man",
                "yapacc list --explicit",
                "yapacc install tx-apt --name a --suite stable",
                "yapacc completions bash",
            ]
        );
        assert_eq!(
            command_words("yapacc search \"draw in the terminal\" --limit 3"),
            ["yapacc", "search", "draw in the terminal", "--limit", "3"]
        );
        assert_eq!(
            command_words("yapacc help COMMAND"),
            ["yapacc", "help", "why"]
        );
    }

    /// The names of a command and of all those below it, as `a b c` paths.
    fn paths(cmd: &clap::Command, prefix: &str, out: &mut Vec<String>) {
        for sub in cmd.get_subcommands() {
            if sub.get_name() == "help" {
                continue;
            }
            let path = format!("{prefix}{}", sub.get_name());
            out.push(path.clone());
            paths(sub, &format!("{path} "), out);
        }
    }

    fn mentions(text: &str, word: &str) -> bool {
        text.split(|c: char| !(c.is_alphanumeric() || c == '-'))
            .any(|w| w == word)
    }

    #[test]
    fn the_commands_topic_and_the_manual_page_name_every_command() {
        let cli = crate::cli::Cli::command();
        let topic = render("commands").unwrap();
        let man = MAN_PAGE.replace("\\-", "-");
        let man_commands = man
            .split(".SH COMMANDS")
            .nth(1)
            .and_then(|s| s.split("\n.SH ").next())
            .expect("the manual page has a COMMANDS section");
        for top in cli.get_subcommands() {
            let name = top.get_name();
            if name == "help" {
                continue;
            }
            assert!(mentions(&topic, name), "`docs commands` lacks {name}");
            assert!(mentions(man_commands, name), "manual COMMANDS lacks {name}");
            // and every subcommand of it, by its own word
            for sub in top.get_subcommands() {
                let sub = sub.get_name();
                if sub == "help" {
                    continue;
                }
                assert!(mentions(&topic, sub), "`docs commands` lacks {name} {sub}");
                assert!(
                    mentions(man_commands, sub),
                    "manual COMMANDS lacks {name} {sub}"
                );
            }
        }
        // what `--help` shows is exactly the everyday part, plus ledger
        let shown: Vec<&str> = cli
            .get_subcommands()
            .filter(|c| !c.is_hide_set() && c.get_name() != "help")
            .map(|c| c.get_name())
            .collect();
        let everyday = topic.split("OTHER SOURCES").next().unwrap();
        for name in shown {
            assert!(
                mentions(everyday, name) || name == "ledger",
                "{name} is in --help but not under the everyday commands"
            );
        }
    }

    #[test]
    fn the_json_topic_and_the_manual_page_list_exactly_the_json_table() {
        let topic = render("json").unwrap();
        let listed: Vec<&str> = topic
            .split("have a JSON form:\n\n")
            .nth(1)
            .and_then(|s| s.split("\n\n").next())
            .expect("the list of commands")
            .lines()
            .collect();
        assert_eq!(listed.len(), crate::cli::JSON_COMMANDS.len());
        for ((path, condition), line) in crate::cli::JSON_COMMANDS.iter().zip(&listed) {
            let line = line.trim();
            assert!(line.starts_with(path), "{line:?} should be {path:?}");
            assert_eq!(line[path.len()..].trim(), *condition, "{line:?}");
        }
        // the manual page: one `.IP` entry per command, in the same order
        let man = MAN_PAGE.replace("\\-", "-");
        let section = man
            .split(".SS Commands with a JSON form")
            .nth(1)
            .and_then(|s| s.split("\n.SH ").next())
            .expect("the manual page lists the commands with a JSON form");
        let entries: Vec<&str> = section
            .lines()
            .filter_map(|l| l.strip_prefix(".B "))
            .collect();
        let table: Vec<&str> = crate::cli::JSON_COMMANDS.iter().map(|(p, _)| *p).collect();
        assert_eq!(entries, table);
        // every entry is a command that exists
        let mut all = Vec::new();
        paths(&crate::cli::Cli::command(), "", &mut all);
        for (path, _) in crate::cli::JSON_COMMANDS {
            assert!(all.iter().any(|p| p == path), "{path} is not a command");
        }
    }

    #[test]
    fn the_manual_page_is_valid_in_form() {
        let version = env!("CARGO_PKG_VERSION");
        assert!(
            MAN_PAGE.starts_with(&format!(
                ".TH YAPACC 1 \"2026-11-19\" \"yapacc {version}\" \"User Commands\"\n"
            )),
            "the .TH line must carry the date and version {version}"
        );
        let mut at = 0;
        for section in [
            "NAME",
            "SYNOPSIS",
            "DESCRIPTION",
            "OPTIONS",
            "COMMANDS",
            "THE GATE",
            "EXIT STATUS",
            "FILES",
            "ENVIRONMENT",
            "EXAMPLES",
            "LIMITS",
            "SEE ALSO",
            "COPYRIGHT",
        ] {
            let found = MAN_PAGE[at..]
                .find(&format!("\n.SH {section}\n"))
                .unwrap_or_else(|| panic!("section {section} is missing or out of order"));
            at += found + 1;
        }
        // only ASCII: groff -ww warns about anything else
        assert!(MAN_PAGE.is_ascii(), "the manual page must be ASCII");
        assert!(MAN_PAGE.contains("version 2 of the License"));
        // every built-in topic is named in it
        let plain = MAN_PAGE.replace("\\-", "-");
        for (slug, _, _) in topics() {
            assert!(mentions(&plain, slug), "the manual page lacks topic {slug}");
        }
    }

    #[test]
    fn the_exit_codes_are_the_same_in_the_topic_the_help_and_the_manual() {
        let topic = render("exit-codes").unwrap();
        let man = MAN_PAGE.replace("\\-", "-");
        let status = man
            .split(".SH EXIT STATUS")
            .nth(1)
            .and_then(|s| s.split("\n.SH ").next())
            .unwrap();
        let help = crate::cli::Cli::command().render_help().to_string();
        for (code, word) in [
            (0, "ok"),
            (1, "failed"),
            (2, "usage"),
            (3, "refused"),
            (4, "not found"),
            (5, "integrity"),
        ] {
            assert!(
                topic.contains(&format!("  {code}  {word}")),
                "{code} {word}"
            );
            assert!(
                help.contains(&format!("{code} {word}")),
                "help: {code} {word}"
            );
            assert!(
                status.contains(&format!(".B {code}\n")) && status.contains(word),
                "manual: {code} {word}"
            );
        }
        assert!(topic.contains("  6  unavailable"));
        assert!(status.contains(".B 6\n"));
    }

    /// The interface's own help text is the source of truth for its keys:
    /// the `tui` topic carries every line of it.
    #[test]
    fn the_tui_topic_carries_the_help_text_of_the_interface() {
        let source = include_str!("tui.rs");
        let help = source
            .split("const HELP: &str = \"\\\n")
            .nth(1)
            .and_then(|s| s.split("\";\n").next())
            .expect("src/tui.rs defines HELP");
        let topic = render("tui").unwrap();
        let mut lines = 0;
        for line in help.lines().filter(|l| !l.trim().is_empty()) {
            assert!(topic.contains(line), "the tui topic lacks: {line}");
            lines += 1;
        }
        assert!(lines > 20, "only {lines} lines of HELP were read");
    }
}
