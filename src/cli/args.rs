// yapacc — CLI
// Copyright (C) 2026 Alexandre BETRY
// SPDX-License-Identifier: GPL-2.0-only
//! Command-line grammar: verbs, subverbs and the shared gate flags.

// The parsers clap derives for the two large verb enums are flagged by a
// lint that only newer toolchains know; the older ones must not trip on its
// name.
#![allow(unknown_lints)]
#![allow(clippy::large_unopt_stack_frames)]

use clap::{Args, Parser, Subcommand};

// ---------------------------------------------------------------------------
// Gating args shared by every mutating subcommand
// ---------------------------------------------------------------------------

/// The heading the global options are printed under, in every command's
/// help, so that they do not sit among the command's own options.
pub const GLOBAL_HEADING: &str = "Global options";

#[derive(Args, Debug, Clone)]
pub struct Gate {
    /// Show what would be done, and the native manager's simulation where there is one; change nothing
    #[arg(long, conflicts_with = "yes")]
    pub dryrun: bool,
    /// Apply without the [y/N] question (the plan is still shown)
    #[arg(long)]
    pub yes: bool,
}

/// Flags shared by everything that builds third-party code (`get` through
/// the aur/brew/recipe lanes, `install aur`).
#[derive(Args, Debug, Clone, Copy, Default)]
pub struct RepairFlags {
    /// Do not classify a failed build and retry it
    #[arg(long)]
    pub no_autorepair: bool,
    /// On a checksum mismatch, rewrite the checksums in the build directory (both hashes are printed); this gives up the tamper check
    #[arg(long)]
    pub allow_checksum_refresh: bool,
    /// Do not record a successful repair under ~/.local/share/yapacc/fixes
    #[arg(long)]
    pub no_fix_record: bool,
    /// Install missing build dependencies from the repositories without asking (otherwise it asks, or only reports when nobody can answer)
    #[arg(long)]
    pub repair_deps: bool,
    /// Build without the sandbox: the build code then runs with your full user access
    #[arg(long)]
    pub no_sandbox: bool,
    /// Allow network access during build() and package() (default: only while sources are fetched)
    #[arg(long)]
    pub build_net: bool,
}

// ---------------------------------------------------------------------------
// Top-level verbs
// ---------------------------------------------------------------------------

/// Printed under the list of commands. Lines are at most 78 columns, so that
/// an 80-column terminal does not wrap them a second time.
const AFTER_HELP: &str = "\
Read commands change nothing. A command marked (gated) shows its plan, then
asks; --dryrun only plans, --yes applies without asking. Both go after the
command. Global options go before it: yapacc --json why bash

Specialist commands (flatpak, AppImage, the AUR on Arch, isolated installs,
saved plans) are not listed above: `yapacc docs commands` lists every
command, and `yapacc help COMMAND` describes each.

Exit codes:
  0 ok   1 failed   2 usage   3 refused, nothing was changed   4 not found
  5 integrity check failed    6 something the command needs is unavailable

Documentation: `yapacc docs` lists the built-in topics, and
`yapacc docs --man | man -l -` shows the manual page.
Handbook: https://ezechiel203.github.io/yapacc/";

#[derive(Parser, Debug)]
#[command(
    name = "yapacc",
    version,
    max_term_width = 100,
    about = "An optional companion to the package manager you already have (pacman, apt, dnf, zypper, apk, xbps, FreeBSD pkg): why a package is installed, what changed, what the next upgrade will do, and how to take the last change back",
    after_help = AFTER_HELP
)]
pub struct Cli {
    /// Never prompt: a command that would ask is refused (exit 3) unless --yes or --dryrun is given
    #[arg(long, global = true, help_heading = GLOBAL_HEADING)]
    pub script: bool,
    /// Consent in advance to the [y/N] question, like --yes on a gated command
    #[arg(long, global = true, help_heading = GLOBAL_HEADING)]
    pub noconfirm: bool,
    /// Read this configuration file instead of ~/.config/yapacc/config.toml
    #[arg(long, global = true, value_name = "PATH", help_heading = GLOBAL_HEADING)]
    pub config: Option<String>,
    /// Print JSON instead of text; exits 2 on a command that has no JSON form (`yapacc docs json` lists those that have)
    #[arg(long, global = true, help_heading = GLOBAL_HEADING)]
    pub json: bool,
    #[command(subcommand)]
    pub command: Option<Verb>,
}

#[derive(Subcommand, Debug)]
pub enum Verb {
    /// Why is this package here? Install reason, what needs it, where it came from, how it was verified (read-only)
    #[command(display_order = 1)]
    Why {
        /// Package name: a native package, a flatpak, or something yapacc installed outside the package manager
        name: String,
    },
    /// What changed? Installs, upgrades and removals from the package manager's own log, newest first (read-only)
    #[command(display_order = 2)]
    Timeline {
        /// Only changes on or after this day (YYYY-MM-DD, UTC)
        #[arg(long, value_name = "YYYY-MM-DD")]
        since: Option<String>,
        /// Show at most this many changes
        #[arg(long, default_value_t = 50, value_name = "N")]
        limit: usize,
        /// Only changes since the boot before the current one ("what changed since it last worked?")
        #[arg(long)]
        since_boot: bool,
        /// Write one self-contained HTML page to standard output (not with --json)
        #[arg(long)]
        html: bool,
    },
    /// What will the next upgrade do? Kernel and reboot, security fixes, news, unmerged configuration, disk, snapshots (read-only)
    #[command(display_order = 3)]
    Briefing {
        /// Contact nothing: no news feed, and on Arch no `checkupdates`
        #[arg(long)]
        offline: bool,
    },
    /// What can be reclaimed? Orphans, package cache, unused runtimes, with the command for each (read-only)
    #[command(display_order = 4)]
    Clean,
    /// What must be restarted after an upgrade? Services and programs still using replaced files (read-only)
    #[command(display_order = 5)]
    Restart,
    /// Configuration files an upgrade left unmerged (.pacnew, .dpkg-dist, .rpmnew…), with how to compare each (read-only)
    #[command(display_order = 6)]
    Configs,
    /// What works on this machine? One line per check: ok, warn, fail or n/a (read-only)
    #[command(display_order = 7)]
    Doctor,
    /// What kind of machine is this? Distribution, package manager, init, boot, logs, snapshots, support tier (read-only)
    #[command(display_order = 8)]
    Platform,

    // ----- looking things up -------------------------------------------------
    /// Installed packages (read-only)
    #[command(display_order = 10)]
    List {
        /// Only packages that are in no repository (Arch only)
        #[arg(long)]
        foreign: bool,
        /// Only packages you asked for, not their dependencies
        #[arg(long)]
        explicit: bool,
        /// Show at most this many packages
        #[arg(long, value_name = "N")]
        limit: Option<usize>,
        /// Also list what yapacc installed outside the package manager (Arch only)
        #[arg(long)]
        include_external: bool,
    },
    /// Details of one installed package (read-only)
    #[command(display_order = 11)]
    Info {
        /// Name of an installed package
        pkg: String,
    },
    /// Search the repositories, from the indexes on disk (read-only)
    #[command(display_order = 12)]
    Search {
        /// Words to look for in package names and descriptions
        query: String,
        /// Search the AUR instead of the repositories; sends the query to aur.archlinux.org (Arch only)
        #[arg(long)]
        aur: bool,
        /// Search the repositories and the AUR; sends the query to aur.archlinux.org (Arch only)
        #[arg(long)]
        all: bool,
        /// Show at most this many results
        #[arg(long, default_value_t = 15, value_name = "N")]
        limit: usize,
        /// Keep only results in this category, for example graphics (Arch only)
        #[arg(long)]
        category: Option<String>,
        /// Keep only results of this kind, for example library or gui-app (Arch only)
        #[arg(long)]
        kind: Option<String>,
        /// Keep only results of this package type or size class, for example library or large (Arch only)
        #[arg(long = "type", alias = "ptype", value_name = "TYPE")]
        ptype: Option<String>,
    },
    /// Pending updates, as of the last repository refresh (read-only; refreshes nothing)
    #[command(display_order = 13)]
    Check,
    /// Look closer: owns PATH, files PKG, orphans everywhere; more on Arch (read-only)
    #[command(display_order = 14)]
    Inspect {
        #[command(subcommand)]
        cmd: InspectVerb,
    },
    /// Browse software by category, offline (read-only; lists packages on Arch only)
    #[command(display_order = 15)]
    Tree {
        /// CATEGORY or CATEGORY.SUBCATEGORY; without it the categories are listed
        path: Option<String>,
        /// List installed packages only (default: installed and repository packages)
        #[arg(long)]
        installed: bool,
    },
    /// What is this package called on other distributions? Sends the name to repology.org
    #[command(display_order = 16)]
    Names {
        /// Package name as it is known on this system
        name: String,
    },

    // ----- changes ------------------------------------------------------------
    /// Install: `repo PKG…` from the distribution's repositories; also flatpak, appimage and more (gated)
    #[command(display_order = 20)]
    Install {
        #[command(subcommand)]
        cmd: InstallVerb,
    },
    /// Remove: `repo PKG…`, or `orphans` (dependencies nothing needs); also flatpak, appimage and more (gated)
    #[command(display_order = 21)]
    Remove {
        #[command(subcommand)]
        cmd: RemoveVerb,
    },
    /// Full system upgrade through the native package manager (gated)
    #[command(display_order = 22)]
    Upgrade {
        /// Upgrade AUR packages too, through yay, which asks its own questions (Arch only)
        #[arg(long)]
        aur: bool,
        #[command(flatten)]
        gate: Gate,
    },
    /// Refresh the repository indexes only, with the native manager's own command (gated)
    #[command(display_order = 23)]
    Refresh {
        #[command(flatten)]
        gate: Gate,
    },
    /// Choose a source for NAME and install it: the distribution's package wins; --explain only shows the choice (gated)
    #[command(display_order = 24)]
    Get {
        /// Package names
        #[arg(required = true, value_name = "NAME")]
        names: Vec<String>,
        /// Use this source only: repo | flatpak everywhere; aur | brew | recipe on Arch
        #[arg(long, value_name = "LANE")]
        from: Option<String>,
        /// Show the decision table and stop
        #[arg(long)]
        explain: bool,
        /// Source archive (http or https) for a generated recipe (Arch only)
        #[arg(long, value_name = "URL")]
        source_url: Option<String>,
        /// Accept the PKGBUILD or generated recipe without reading it at the terminal (Arch only)
        #[arg(long)]
        no_review: bool,
        /// User the build runs as when yapacc runs as root; makepkg refuses root (Arch only)
        #[arg(long, value_name = "USER")]
        build_user: Option<String>,
        /// Link the programs in the install's own bin/ directory under attributed names (fgn.FORMAT.PKG--NAME), or not: attributed | none (default: [foreign] expose); PATH is not changed
        #[arg(long, value_name = "MODE")]
        expose: Option<String>,
        #[command(flatten)]
        build: RepairFlags,
        /// Build system of a generated recipe: cargo | go | meson | cmake | autotools | make | python | node | custom (default: detected)
        #[arg(long, value_name = "NAME")]
        factory: Option<String>,
        /// Keep the build directory after a successful custom build, without asking
        #[arg(long)]
        keep_factory: bool,
        /// Delete the build directory after a successful custom build, without asking
        #[arg(long)]
        discard_factory: bool,
        /// Accept a non-free licence in advance; without it the licence is asked about, and --yes does not answer
        #[arg(long)]
        non_free: bool,
        #[command(flatten)]
        gate: Gate,
    },
    /// Take the last package transaction back (pacman, apt, dnf, zypper; gated)
    #[command(display_order = 25)]
    Undo {
        #[command(flatten)]
        gate: Gate,
    },
    /// Filesystem snapshots through snapper, timeshift or zfs: list, create
    #[command(display_order = 26)]
    Snapshot {
        #[command(subcommand)]
        cmd: SnapshotVerb,
    },
    /// What yapacc installed outside the package manager: status, list, head, verify, audit, conflicts (read-only)
    #[command(display_order = 27)]
    Ledger {
        #[command(subcommand)]
        cmd: LedgerVerb,
    },

    // ----- the program itself -------------------------------------------------
    /// Built-in documentation: list the topics, or show one; --man prints the manual page
    #[command(display_order = 30)]
    Docs {
        /// Topic to show (`yapacc docs` lists them)
        topic: Option<String>,
        /// Print the manual page in roff, for `man -l -`
        #[arg(long)]
        man: bool,
    },
    /// The effective configuration and the file it was loaded from (read-only)
    #[command(display_order = 31)]
    Config,
    /// Print a completion script for a shell
    #[command(display_order = 32)]
    Completions {
        /// The shell to write the script for
        shell: clap_complete::Shell,
    },
    /// Interactive interface (needs the optional yapacc-tui component)
    #[command(alias = "interactive")]
    #[command(display_order = 33)]
    I,
    /// Server for AI assistants, over standard input and output (needs the optional yapacc-mcp component)
    #[command(display_order = 34)]
    Mcp,

    // ----- specialist commands, not listed by `--help` ------------------------
    /// What this host supports, per package format and snapshot tool (read-only)
    #[command(hide = true)]
    Capabilities,
    /// Repository and AUR metadata for one package (Arch only; read-only)
    #[command(hide = true)]
    Query {
        /// Package name
        pkg: String,
        /// Look in the repositories only
        #[arg(long)]
        repo: bool,
        /// Look in the AUR only; sends the name to aur.archlinux.org
        #[arg(long)]
        aur: bool,
    },
    /// Recent Arch Linux news headlines, fetched from archlinux.org (Arch only; read-only)
    #[command(hide = true)]
    News {
        /// Show at most this many headlines
        #[arg(long, default_value_t = 5, value_name = "N")]
        limit: usize,
    },
    /// Read flatpak: list, search, info (read-only)
    #[command(hide = true)]
    Flatpak {
        #[command(subcommand)]
        cmd: FlatpakVerb,
    },
    /// Packages made for another distribution: inspect a file, list the isolated installs (read-only)
    #[command(hide = true)]
    Foreign {
        #[command(subcommand)]
        cmd: ForeignReadVerb,
    },
    /// A file index of the repositories for repeated searches: build, search (Arch only)
    #[command(hide = true)]
    Index {
        #[command(subcommand)]
        cmd: IndexVerb,
    },
    /// Everything yapacc installed outside the package manager, in both ledgers: list, audit (read-only)
    #[command(hide = true)]
    External {
        #[command(subcommand)]
        cmd: ExternalVerb,
    },
    /// Read APT or RPM repository definitions from a directory (read-only)
    #[command(hide = true)]
    Sources {
        #[command(subcommand)]
        cmd: SourcesVerb,
    },
    /// Scheduled update check: one-shot check, and the systemd timer that runs it (Arch only)
    #[command(hide = true)]
    Monitor {
        #[command(subcommand)]
        cmd: MonitorVerb,
    },
    /// Classify the failures in a build log and name a repair for each (read-only)
    #[command(hide = true)]
    Repair {
        /// Log file to read, or - for standard input
        log: String,
    },
    /// Build repairs recorded on this machine: list, push, pull through your own git remote
    #[command(hide = true)]
    Fixes {
        #[command(subcommand)]
        cmd: FixesVerb,
    },
    /// List the adapter programs found (yapacc-source-*, yapacc-runtime-*) (read-only)
    #[command(hide = true)]
    Plugins,
    /// Read an AUR package: deps, review, pkgbuild (Arch only; contacts aur.archlinux.org)
    #[command(hide = true)]
    Aur {
        #[command(subcommand)]
        cmd: AurVerb,
    },
    /// Help with writing a PKGBUILD: scaffold, validate, srcinfo (Arch only)
    #[command(hide = true)]
    Package {
        #[command(subcommand)]
        cmd: PackageVerb,
    },
    /// Update what was installed outside the repositories: flatpak, appimage, foreign, external (gated)
    #[command(hide = true)]
    Update {
        #[command(subcommand)]
        cmd: UpdateVerb,
    },
    /// Change a package's install reason: explicit, dependency (Arch only; gated)
    #[command(hide = true)]
    Mark {
        #[command(subcommand)]
        cmd: MarkVerb,
    },
    /// Download packages into the pacman cache without installing them (Arch only; gated)
    #[command(hide = true)]
    Download {
        /// Package names
        #[arg(required = true, value_name = "PKG")]
        pkgs: Vec<String>,
        #[command(flatten)]
        gate: Gate,
    },
    /// The pacman package cache: clean (Arch only; gated)
    #[command(hide = true)]
    Cache {
        #[command(subcommand)]
        cmd: CacheVerb,
    },
    /// Print a plan without applying it, save one to a file, apply a saved one (Arch only)
    #[command(hide = true)]
    Plan {
        #[command(subcommand)]
        cmd: PlanVerb,
    },
    /// A staged change: apply (Arch only; gated)
    #[command(hide = true)]
    Safety {
        #[command(subcommand)]
        cmd: SafetyVerb,
    },
}

/// The default of `--scope` on the ledger commands.
const SCOPE_DEFAULT: &str = "user";

#[derive(Subcommand, Debug)]
pub enum SnapshotVerb {
    /// List existing snapshots, newest first (read-only; may need root)
    List,
    /// Take a snapshot now, through snapper, timeshift or zfs (gated)
    Create {
        /// Text stored with the snapshot
        #[arg(long, value_name = "TEXT")]
        description: Option<String>,
        #[command(flatten)]
        gate: Gate,
    },
}

#[derive(Subcommand, Debug)]
pub enum InspectVerb {
    /// Which package owns this file
    Owns {
        /// Absolute path of a file
        path: String,
    },
    /// The files of an installed package, one per line
    Files {
        /// Name of an installed package
        pkg: String,
    },
    /// Packages installed as dependencies that nothing needs any more
    Orphans,
    /// Packages you installed that nothing depends on (Arch only)
    Leaves,
    /// How a package is classified: kind, type, categories, size class (Arch only)
    Facets {
        /// Name of an installed package
        pkg: String,
    },
    /// Dependency tree of a package, through pactree (Arch only)
    Deps {
        /// Package name
        pkg: String,
        /// How many levels to show
        #[arg(long, default_value_t = 2, value_name = "N")]
        depth: u32,
    },
    /// What depends on a package, as a tree, through pactree (Arch only)
    Rdepends {
        /// Package name
        pkg: String,
        /// How many levels to show
        #[arg(long, default_value_t = 1, value_name = "N")]
        depth: u32,
    },
    /// Which repository package provides a file, from the pacman file databases (Arch only)
    FileSearch {
        /// File name or path to look for
        path: String,
        /// Run `pacman -Fy` first to update the file databases (needs root)
        #[arg(long)]
        refresh: bool,
    },
    /// Package groups, or the members of one group (Arch only)
    Groups {
        /// Group name; without it all groups are listed
        group: Option<String>,
    },
    /// .pacnew, .pacsave and .orig files under a directory (Arch only)
    Pacdiff {
        /// Directory to scan
        #[arg(long, default_value = "/etc", value_name = "DIR")]
        root: String,
    },
    /// The same reads through libalpm: list, info, leaves, search, sync-query (Arch only; builds with the alpm feature)
    Alpm {
        #[command(subcommand)]
        cmd: AlpmVerb,
    },
}

#[derive(Subcommand, Debug)]
pub enum AlpmVerb {
    /// Installed packages, read through libalpm
    List {
        /// Show at most this many packages
        #[arg(long, default_value_t = 20, value_name = "N")]
        limit: usize,
        /// Only packages that nothing depends on
        #[arg(long)]
        leaves: bool,
    },
    /// Details of one installed package, read through libalpm
    Info {
        /// Name of an installed package
        pkg: String,
    },
    /// Installed packages that nothing depends on (the first 30), read through libalpm
    Leaves,
    /// Search the repository databases through libalpm
    Search {
        /// Text to look for in names and descriptions
        query: String,
        /// Show at most this many results
        #[arg(long, default_value_t = 20, value_name = "N")]
        limit: usize,
    },
    /// Repository, version and description of one package, read through libalpm
    SyncQuery {
        /// Package name
        pkg: String,
    },
}

#[derive(Subcommand, Debug)]
pub enum FlatpakVerb {
    /// Installed flatpak applications and runtimes
    List,
    /// Search the configured flatpak remotes
    Search {
        /// Text to look for
        query: String,
    },
    /// Details of one installed flatpak
    Info {
        /// Application ID, for example org.mozilla.firefox
        app: String,
    },
}

#[derive(Subcommand, Debug)]
pub enum ForeignReadVerb {
    /// Show the metadata of a package file (deb, rpm or snap) without installing it
    Info {
        /// Package format: deb | rpm | snap
        #[arg(long)]
        format: String,
        /// Path of the package file
        #[arg(long, value_name = "FILE")]
        source: String,
    },
    /// List the isolated installs recorded in the ledger
    List {
        /// Read this ledger directory instead of the user ledger
        #[arg(long, value_name = "DIR")]
        ledger_path: Option<String>,
    },
}

#[derive(Subcommand, Debug)]
pub enum IndexVerb {
    /// Write the index file from the repository databases on disk
    Build {
        /// Index file to write (default: $XDG_CACHE_HOME/yapacc/repo-index.tsv, or ~/.cache/yapacc/repo-index.tsv)
        #[arg(long, value_name = "FILE")]
        path: Option<String>,
        /// Index at most this many packages; 0 means all
        #[arg(long, default_value_t = 0, value_name = "N")]
        limit: usize,
    },
    /// Search an index file written by `index build`
    Search {
        /// Text to look for
        query: String,
        /// Index file to read (default: $XDG_CACHE_HOME/yapacc/repo-index.tsv, or ~/.cache/yapacc/repo-index.tsv)
        #[arg(long, value_name = "FILE")]
        path: Option<String>,
        /// Keep only rows in this category
        #[arg(long)]
        category: Option<String>,
        /// Show at most this many rows
        #[arg(long, default_value_t = 30, value_name = "N")]
        limit: usize,
    },
}

#[derive(Subcommand, Debug)]
pub enum LedgerVerb {
    /// Where the ledger is, how many records it holds, its head and its witnesses; exits 5 when the cache differs from the journal
    Status {
        /// Which ledger: user | system
        #[arg(long, default_value = SCOPE_DEFAULT, value_parser = ["user", "system"])]
        scope: String,
        /// Read this ledger directory instead
        #[arg(long, value_name = "DIR")]
        path: Option<String>,
    },
    /// The records, replayed from the journal
    List {
        /// Which ledger: user | system
        #[arg(long, default_value = SCOPE_DEFAULT, value_parser = ["user", "system"])]
        scope: String,
        /// Read this ledger directory instead
        #[arg(long, value_name = "DIR")]
        path: Option<String>,
    },
    /// Print the current head as SEQ:HASH, to keep somewhere the ledger's owner cannot rewrite
    Head {
        /// Which ledger: user | system
        #[arg(long, default_value = SCOPE_DEFAULT, value_parser = ["user", "system"])]
        scope: String,
        /// Read this ledger directory instead
        #[arg(long, value_name = "DIR")]
        path: Option<String>,
    },
    /// Check the hash chain, and compare it with the witnesses and anchors; exits 5 on a difference
    Verify {
        /// Which ledger: user | system
        #[arg(long, default_value = SCOPE_DEFAULT, value_parser = ["user", "system"])]
        scope: String,
        /// Read this ledger directory instead
        #[arg(long, value_name = "DIR")]
        path: Option<String>,
        /// A head printed earlier by `ledger head` (SEQ:HASH); may be given several times
        #[arg(long, value_name = "SEQ:HASH")]
        anchor: Vec<String>,
        /// Accept events written with the old checksum as unverified instead of failing
        #[arg(long)]
        accept_legacy: bool,
    },
    /// Look for overlapping paths between recorded installs, name collisions and missing dependencies (best effort)
    Audit {
        /// Which ledger: user | system
        #[arg(long, default_value = SCOPE_DEFAULT, value_parser = ["user", "system"])]
        scope: String,
        /// Read this ledger directory instead
        #[arg(long, value_name = "DIR")]
        path: Option<String>,
    },
    /// Say whether the given paths belong to a recorded install, exist on disk, or are owned by a pacman package
    Conflicts {
        /// Which ledger: user | system
        #[arg(long, default_value = SCOPE_DEFAULT, value_parser = ["user", "system"])]
        scope: String,
        /// Read this ledger directory instead
        #[arg(long, value_name = "DIR")]
        path: Option<String>,
        /// Paths to check
        #[arg(required = true, value_name = "PATH")]
        paths: Vec<String>,
    },
}

#[derive(Subcommand, Debug)]
pub enum ExternalVerb {
    /// List the installs recorded in the user and system ledgers
    List,
    /// Look for overlapping paths, name collisions and missing dependencies in both ledgers (best effort)
    Audit,
}

#[derive(Subcommand, Debug)]
pub enum SourcesVerb {
    /// Parse sources.list and sources.list.d under a directory
    Apt {
        /// The APT configuration directory
        #[arg(long, default_value = "/etc/apt", value_name = "DIR")]
        root: String,
    },
    /// Parse the .repo files in a directory
    Rpm {
        /// The directory holding the .repo files
        #[arg(long, default_value = "/etc/yum.repos.d", value_name = "DIR")]
        dir: String,
    },
}

#[derive(Subcommand, Debug)]
pub enum MonitorVerb {
    /// Report pending repository, AUR and flatpak updates once; changes nothing
    Check,
    /// Install a systemd timer that runs `monitor check` every 30 minutes (gated)
    DaemonInstall {
        /// Install for your user (~/.config/systemd/user) instead of system-wide (/etc/systemd/system)
        #[arg(long)]
        user: bool,
        #[command(flatten)]
        gate: Gate,
    },
}

#[derive(Subcommand, Debug)]
pub enum FixesVerb {
    /// List the repairs recorded on this machine
    List,
    /// Push the recorded repairs to the git remote in [fixes] remote (gated)
    Push {
        #[command(flatten)]
        gate: Gate,
    },
    /// Fetch repairs from the git remote in [fixes] remote and merge them into the local store
    Pull,
}

#[derive(Subcommand, Debug)]
pub enum AurVerb {
    /// The build order of AUR packages: what must be built, what the repositories provide, what is missing
    Deps {
        /// AUR package names
        #[arg(required = true, value_name = "PKG")]
        pkgs: Vec<String>,
    },
    /// Print the AUR metadata, the PKGBUILD and the install plan of a package
    Review {
        /// AUR package name
        pkg: String,
    },
    /// Print the PKGBUILD of an AUR package
    Pkgbuild {
        /// AUR package name
        pkg: String,
    },
}

#[derive(Subcommand, Debug)]
pub enum PackageVerb {
    /// Write a PKGBUILD and .SRCINFO to start from
    Scaffold {
        /// Name of the new package
        name: String,
        /// Directory to write into (default: ./NAME-pkg)
        #[arg(long)]
        dir: Option<String>,
    },
    /// Check a PKGBUILD for missing fields and placeholders; runs namcap when it is installed
    Validate {
        /// A PKGBUILD, or the directory that holds one
        target: String,
    },
    /// Print the .SRCINFO of a PKGBUILD directory, through makepkg
    Srcinfo {
        /// Directory holding the PKGBUILD
        #[arg(default_value = ".")]
        dir: String,
    },
}

#[derive(Subcommand, Debug)]
pub enum InstallVerb {
    /// Install from the distribution's repositories, through the native package manager
    Repo {
        /// Package names
        #[arg(required = true, value_name = "PKG")]
        pkgs: Vec<String>,
        /// Install through yay, so that AUR packages are found too; yay asks its own questions (Arch only)
        #[arg(long)]
        aur: bool,
        #[command(flatten)]
        gate: Gate,
    },
    /// Build an AUR package: clone, show the PKGBUILD for review, build in a sandbox, install (Arch only)
    Aur {
        /// AUR package name
        pkg: String,
        /// Clone from this git URL instead of the AUR
        #[arg(long)]
        url: Option<String>,
        /// Build in this directory (default: ./PKG-aur-build)
        #[arg(long)]
        dir: Option<String>,
        /// User the build runs as when yapacc runs as root; makepkg refuses root
        #[arg(long, value_name = "USER")]
        build_user: Option<String>,
        /// Accept the PKGBUILD without reading it at the terminal; required when nobody can answer
        #[arg(long)]
        no_review: bool,
        /// Build and install the missing AUR dependencies first, in dependency order
        #[arg(long)]
        deps: bool,
        #[command(flatten)]
        build: RepairFlags,
        #[command(flatten)]
        gate: Gate,
    },
    /// Install package files you already have, with `pacman -U` (Arch only)
    Local {
        /// Package files (.pkg.tar.*)
        #[arg(required = true, value_name = "FILE")]
        files: Vec<String>,
        #[command(flatten)]
        gate: Gate,
    },
    /// Install through libalpm, with the transaction prepared before the gate (Arch only; needs root and a build with the alpm feature)
    Alpm {
        /// Package names
        #[arg(required = true, value_name = "PKG")]
        pkgs: Vec<String>,
        /// Choose the package that provides a virtual one: VIRTUAL=PACKAGE; may be given several times
        #[arg(long = "provider", value_name = "VIRTUAL=PACKAGE")]
        providers: Vec<String>,
        #[command(flatten)]
        gate: Gate,
    },
    /// Install a flatpak application
    Flatpak {
        /// Application ID, for example org.mozilla.firefox
        app: String,
        /// Install for your user only, without root
        #[arg(long)]
        user: bool,
        #[command(flatten)]
        gate: Gate,
    },
    /// Download or copy an AppImage under your home directory and record it in the ledger
    Appimage {
        /// Name to record it under
        #[arg(long)]
        name: String,
        /// File path or URL of the AppImage
        #[arg(long, value_name = "FILE-OR-URL")]
        source: String,
        /// Directory to put it in (default: ~/Applications/yapacc/appimages)
        #[arg(long, value_name = "DIR")]
        dest: Option<String>,
        /// Record it in this ledger directory instead of the user ledger
        #[arg(long, value_name = "DIR")]
        ledger_path: Option<String>,
        /// The SHA-256 the file must have; a mismatch deletes it (exit 5). Without it the file is NOT verified
        #[arg(long, value_name = "HEX")]
        sha256: Option<String>,
        /// Accept a plain http:// download (refused otherwise)
        #[arg(long)]
        allow_http: bool,
        #[command(flatten)]
        gate: Gate,
    },
    /// Unpack one package made for another distribution into an isolated directory; its scripts are not run
    Foreign {
        /// Name to record it under
        #[arg(long)]
        name: String,
        /// Package format: deb | rpm | snap
        #[arg(long)]
        format: String,
        /// Path of the package file
        #[arg(long, value_name = "FILE")]
        source: String,
        /// Directory to unpack into (default: a directory per package under the isolated root)
        #[arg(long, value_name = "DIR")]
        prefix: Option<String>,
        /// Record it in this ledger directory instead of the default ledger
        #[arg(long, value_name = "DIR")]
        ledger_path: Option<String>,
        /// Replace an existing install of the same name
        #[arg(long)]
        replace: bool,
        /// Link the programs in the install's own bin/ directory under attributed names (fgn.FORMAT.PKG--NAME), or not: attributed | none (default: [foreign] expose); PATH is not changed
        #[arg(long, value_name = "MODE")]
        expose: Option<String>,
        /// Accept a non-free licence in advance; without it the licence is asked about, and --yes does not answer
        #[arg(long)]
        non_free: bool,
        #[command(flatten)]
        gate: Gate,
    },
    /// Unpack several package files, or URLs, as one isolated install, in dependency order
    Tx {
        /// Package format: deb | rpm | snap
        #[arg(long)]
        format: String,
        /// Take the packages named with --name from this directory
        #[arg(long)]
        dir: Option<String>,
        /// Package to take from --dir; may be given several times
        #[arg(long)]
        name: Vec<String>,
        /// Package files or URLs (when --dir is not used)
        #[arg(value_name = "FILE-OR-URL")]
        sources: Vec<String>,
        /// Record it in this ledger directory instead of the default ledger
        #[arg(long, value_name = "DIR")]
        ledger_path: Option<String>,
        /// Link the programs in the install's own bin/ directory under attributed names (fgn.FORMAT.PKG--NAME), or not: attributed | none (default: [foreign] expose); PATH is not changed
        #[arg(long, value_name = "MODE")]
        expose: Option<String>,
        /// Accept a non-free licence in advance; without it the licence is asked about, and --yes does not answer
        #[arg(long)]
        non_free: bool,
        #[command(flatten)]
        gate: Gate,
    },
    /// Isolated install of packages named in an APT Packages index you give; no signature is checked
    TxAptRepo {
        /// Base URL the package paths of the index are relative to
        #[arg(long, value_name = "URL")]
        base_url: String,
        /// The Packages index: a file path or a URL
        #[arg(long, value_name = "FILE-OR-URL")]
        packages: String,
        /// Package to install; may be given several times
        #[arg(long)]
        name: Vec<String>,
        /// Record it in this ledger directory instead of the default ledger
        #[arg(long, value_name = "DIR")]
        ledger_path: Option<String>,
        /// Link the programs in the install's own bin/ directory under attributed names (fgn.FORMAT.PKG--NAME), or not: attributed | none (default: [foreign] expose); PATH is not changed
        #[arg(long, value_name = "MODE")]
        expose: Option<String>,
        /// Accept a remote index or package location that nothing verifies (refused otherwise)
        #[arg(long)]
        allow_unsigned: bool,
        /// Accept a non-free licence in advance; without it the licence is asked about, and --yes does not answer
        #[arg(long)]
        non_free: bool,
        #[command(flatten)]
        gate: Gate,
    },
    /// Isolated install of packages from an APT repository, given by URL, suite and component
    TxApt {
        /// Repository URL, for example http://deb.debian.org/debian
        #[arg(long, value_name = "URL")]
        base_url: String,
        /// Suite, for example stable
        #[arg(long)]
        suite: String,
        /// Component, for example main
        #[arg(long)]
        component: String,
        /// Architecture of the packages
        #[arg(long, default_value = "amd64")]
        arch: String,
        /// Package to install; may be given several times
        #[arg(long)]
        name: Vec<String>,
        /// Record it in this ledger directory instead of the default ledger
        #[arg(long, value_name = "DIR")]
        ledger_path: Option<String>,
        /// Link the programs in the install's own bin/ directory under attributed names (fgn.FORMAT.PKG--NAME), or not: attributed | none (default: [foreign] expose); PATH is not changed
        #[arg(long, value_name = "MODE")]
        expose: Option<String>,
        /// Binary keyring file the repository signature is checked against, with gpgv
        #[arg(long, value_name = "FILE")]
        keyring: Option<String>,
        /// Install without a keyring: the repository signature is then not checked (refused otherwise)
        #[arg(long)]
        allow_unsigned: bool,
        /// Accept a non-free licence in advance; without it the licence is asked about, and --yes does not answer
        #[arg(long)]
        non_free: bool,
        #[command(flatten)]
        gate: Gate,
    },
    /// Isolated install of packages from the APT repositories defined under a directory
    TxAptSources {
        /// The APT configuration directory to read the repositories from
        #[arg(long, default_value = "/etc/apt", value_name = "DIR")]
        root: String,
        /// Architecture of the packages
        #[arg(long, default_value = "amd64")]
        arch: String,
        /// Package to install; may be given several times
        #[arg(long)]
        name: Vec<String>,
        /// Record it in this ledger directory instead of the default ledger
        #[arg(long, value_name = "DIR")]
        ledger_path: Option<String>,
        /// Link the programs in the install's own bin/ directory under attributed names (fgn.FORMAT.PKG--NAME), or not: attributed | none (default: [foreign] expose); PATH is not changed
        #[arg(long, value_name = "MODE")]
        expose: Option<String>,
        /// Binary keyring file the repository signature is checked against, with gpgv
        #[arg(long, value_name = "FILE")]
        keyring: Option<String>,
        /// Install without a keyring: the repository signature is then not checked (refused otherwise)
        #[arg(long)]
        allow_unsigned: bool,
        /// Accept a non-free licence in advance; without it the licence is asked about, and --yes does not answer
        #[arg(long)]
        non_free: bool,
        #[command(flatten)]
        gate: Gate,
    },
    /// Isolated install of packages named in an RPM primary.xml you give; no signature is checked
    TxRpmRepo {
        /// Base URL the package paths of the index are relative to
        #[arg(long, value_name = "URL")]
        base_url: String,
        /// The primary.xml index: a file path or a URL
        #[arg(long, value_name = "FILE-OR-URL")]
        primary: String,
        /// Package to install; may be given several times
        #[arg(long)]
        name: Vec<String>,
        /// Record it in this ledger directory instead of the default ledger
        #[arg(long, value_name = "DIR")]
        ledger_path: Option<String>,
        /// Link the programs in the install's own bin/ directory under attributed names (fgn.FORMAT.PKG--NAME), or not: attributed | none (default: [foreign] expose); PATH is not changed
        #[arg(long, value_name = "MODE")]
        expose: Option<String>,
        /// Accept a remote index or package location that nothing verifies (refused otherwise)
        #[arg(long)]
        allow_unsigned: bool,
        /// Accept a non-free licence in advance; without it the licence is asked about, and --yes does not answer
        #[arg(long)]
        non_free: bool,
        #[command(flatten)]
        gate: Gate,
    },
    /// Isolated install of packages from an RPM repository, given by URL
    TxRpm {
        /// Repository URL (the directory that holds repodata/)
        #[arg(long, value_name = "URL")]
        base_url: String,
        /// Package to install; may be given several times
        #[arg(long)]
        name: Vec<String>,
        /// Record it in this ledger directory instead of the default ledger
        #[arg(long, value_name = "DIR")]
        ledger_path: Option<String>,
        /// Link the programs in the install's own bin/ directory under attributed names (fgn.FORMAT.PKG--NAME), or not: attributed | none (default: [foreign] expose); PATH is not changed
        #[arg(long, value_name = "MODE")]
        expose: Option<String>,
        /// Binary keyring file the repository signature is checked against, with gpgv
        #[arg(long, value_name = "FILE")]
        keyring: Option<String>,
        /// Install without a keyring: the repository signature is then not checked (refused otherwise)
        #[arg(long)]
        allow_unsigned: bool,
        /// Accept a non-free licence in advance; without it the licence is asked about, and --yes does not answer
        #[arg(long)]
        non_free: bool,
        #[command(flatten)]
        gate: Gate,
    },
    /// Create a distrobox container from an image, to run another distribution's software with its own dependencies
    Container {
        /// Container image, for example debian:stable
        #[arg(long)]
        image: String,
        /// Name of the container (default: derived from the image)
        #[arg(long)]
        name: Option<String>,
        #[command(flatten)]
        gate: Gate,
    },
}

#[derive(Subcommand, Debug)]
pub enum RemoveVerb {
    /// Remove packages through the native package manager
    Repo {
        /// Package names
        #[arg(required = true, value_name = "PKG")]
        pkgs: Vec<String>,
        #[command(flatten)]
        gate: Gate,
    },
    /// Remove the dependencies that nothing needs any more; the list is shown first
    Orphans {
        #[command(flatten)]
        gate: Gate,
    },
    /// Remove through libalpm, with the transaction prepared before the gate (Arch only; needs root and a build with the alpm feature)
    Alpm {
        /// Package names
        #[arg(required = true, value_name = "PKG")]
        pkgs: Vec<String>,
        #[command(flatten)]
        gate: Gate,
    },
    /// Remove a flatpak application
    Flatpak {
        /// Application ID, for example org.mozilla.firefox
        app: String,
        /// Remove it from your user installation
        #[arg(long)]
        user: bool,
        #[command(flatten)]
        gate: Gate,
    },
    /// Remove an AppImage that yapacc installed, by its ledger ID
    Appimage {
        /// ID of the record, as printed by `yapacc external list`
        #[arg(long)]
        id: String,
        /// Use this ledger directory instead of the user ledger
        #[arg(long, value_name = "DIR")]
        ledger_path: Option<String>,
        #[command(flatten)]
        gate: Gate,
    },
    /// Remove an isolated install, by its ledger ID
    Foreign {
        /// ID of the record, as printed by `yapacc foreign list`
        #[arg(long)]
        id: String,
        /// Use this ledger directory instead of the default ledger
        #[arg(long, value_name = "DIR")]
        ledger_path: Option<String>,
        #[command(flatten)]
        gate: Gate,
    },
    /// Remove anything the ledger records, by its ID; the record says how
    External {
        /// ID of the record, as printed by `yapacc external list`
        id: String,
        #[command(flatten)]
        gate: Gate,
    },
}

#[derive(Subcommand, Debug)]
pub enum UpdateVerb {
    /// Update one flatpak application, or all of them
    Flatpak {
        /// Application ID; without it everything is updated
        app: Option<String>,
        /// Update your user installation
        #[arg(long)]
        user: bool,
        #[command(flatten)]
        gate: Gate,
    },
    /// Replace an AppImage that yapacc installed with a newer file
    Appimage {
        /// ID of the record, as printed by `yapacc external list`
        #[arg(long)]
        id: String,
        /// File path or URL of the new AppImage
        #[arg(long, value_name = "FILE-OR-URL")]
        source: String,
        /// Use this ledger directory instead of the user ledger
        #[arg(long, value_name = "DIR")]
        ledger_path: Option<String>,
        #[command(flatten)]
        gate: Gate,
    },
    /// Replace an isolated install with a newer package file
    Foreign {
        /// ID of the record, as printed by `yapacc foreign list`
        #[arg(long)]
        id: String,
        /// Path of the new package file
        #[arg(long, value_name = "FILE")]
        source: String,
        /// Use this ledger directory instead of the default ledger
        #[arg(long, value_name = "DIR")]
        ledger_path: Option<String>,
        /// Link the programs in the install's own bin/ directory under attributed names (fgn.FORMAT.PKG--NAME), or not: attributed | none (default: [foreign] expose); PATH is not changed
        #[arg(long, value_name = "MODE")]
        expose: Option<String>,
        #[command(flatten)]
        gate: Gate,
    },
    /// Update anything the ledger records, by its ID; the record says how
    External {
        /// ID of the record, as printed by `yapacc external list`
        id: String,
        /// File path or URL of the new version
        #[arg(long, value_name = "FILE-OR-URL")]
        source: String,
        #[command(flatten)]
        gate: Gate,
    },
}

#[derive(Subcommand, Debug)]
pub enum MarkVerb {
    /// Mark packages as installed on request, so that they are never orphans
    Explicit {
        /// Package names
        #[arg(required = true, value_name = "PKG")]
        pkgs: Vec<String>,
        #[command(flatten)]
        gate: Gate,
    },
    /// Mark packages as dependencies, so that they become orphans when nothing needs them
    Dependency {
        /// Package names
        #[arg(required = true, value_name = "PKG")]
        pkgs: Vec<String>,
        #[command(flatten)]
        gate: Gate,
    },
}

#[derive(Subcommand, Debug)]
pub enum CacheVerb {
    /// Delete cached packages that are no longer installed (`pacman -Sc`)
    Clean {
        /// Delete every cached package (`pacman -Scc`)
        #[arg(long)]
        all: bool,
        #[command(flatten)]
        gate: Gate,
    },
}

#[derive(Subcommand, Debug)]
pub enum PlanVerb {
    /// Print the plan of an install; --out saves it for `plan apply`
    Install {
        /// Package names
        #[arg(required = true, value_name = "PKG")]
        pkgs: Vec<String>,
        /// Plan the install through yay, so that AUR packages are found too
        #[arg(long)]
        aur: bool,
        /// Save the plan to this file
        #[arg(long, value_name = "FILE")]
        out: Option<String>,
    },
    /// Print the plan of a removal; --out saves it for `plan apply`
    Remove {
        /// Package names
        #[arg(required = true, value_name = "PKG")]
        pkgs: Vec<String>,
        /// Save the plan to this file
        #[arg(long, value_name = "FILE")]
        out: Option<String>,
    },
    /// Print the plan of a full upgrade; --out saves it for `plan apply`
    Upgrade {
        /// Plan the upgrade through yay, so that AUR packages are included
        #[arg(long)]
        aur: bool,
        /// Save the plan to this file
        #[arg(long, value_name = "FILE")]
        out: Option<String>,
    },
    /// Apply a saved plan: the file is validated again and the change goes through the gate
    Apply {
        /// Plan file written with --out
        file: String,
        /// Apply a plan that is older than 48 hours or dated in the future
        #[arg(long)]
        force: bool,
        #[command(flatten)]
        gate: Gate,
    },
    /// Print what `install flatpak` would run
    Flatpak {
        /// Application ID
        app: String,
        /// For your user installation
        #[arg(long)]
        user: bool,
    },
    /// Print what `remove flatpak` would run
    FlatpakRemove {
        /// Application ID
        app: String,
        /// For your user installation
        #[arg(long)]
        user: bool,
    },
    /// Print what `update flatpak` would run
    FlatpakUpdate {
        /// Application ID; without it everything is updated
        app: Option<String>,
        /// For your user installation
        #[arg(long)]
        user: bool,
    },
    /// Print what `install appimage` would do
    Appimage {
        /// Name to record it under
        #[arg(long)]
        name: String,
        /// File path or URL of the AppImage
        #[arg(long, value_name = "FILE-OR-URL")]
        source: String,
        /// Directory to put it in (default: ~/Applications/yapacc/appimages)
        #[arg(long, value_name = "DIR")]
        dest: Option<String>,
    },
    /// Print what `install foreign` would do
    Foreign {
        /// Name to record it under
        #[arg(long)]
        name: String,
        /// Package format: deb | rpm | snap
        #[arg(long)]
        format: String,
        /// Path of the package file
        #[arg(long, value_name = "FILE")]
        source: String,
        /// Directory to unpack into (default: a directory per package under the isolated root)
        #[arg(long, value_name = "DIR")]
        prefix: Option<String>,
    },
    /// Print what `install tx` would do
    Tx {
        /// Package format: deb | rpm | snap
        #[arg(long)]
        format: String,
        /// Take the packages named with --name from this directory
        #[arg(long)]
        dir: Option<String>,
        /// Package to take from --dir; may be given several times
        #[arg(long)]
        name: Vec<String>,
        /// Package files or URLs (when --dir is not used)
        #[arg(value_name = "FILE-OR-URL")]
        sources: Vec<String>,
    },
    /// Print what `install tx-apt-repo` would do
    TxAptRepo {
        /// Base URL the package paths of the index are relative to
        #[arg(long, value_name = "URL")]
        base_url: String,
        /// The Packages index: a file path or a URL
        #[arg(long, value_name = "FILE-OR-URL")]
        packages: String,
        /// Package to install; may be given several times
        #[arg(long)]
        name: Vec<String>,
    },
    /// Print what `install tx-apt` would do; reads the repository index
    TxApt {
        /// Repository URL, for example http://deb.debian.org/debian
        #[arg(long, value_name = "URL")]
        base_url: String,
        /// Suite, for example stable
        #[arg(long)]
        suite: String,
        /// Component, for example main
        #[arg(long)]
        component: String,
        /// Architecture of the packages
        #[arg(long, default_value = "amd64")]
        arch: String,
        /// Package to install; may be given several times
        #[arg(long)]
        name: Vec<String>,
        /// Binary keyring file the repository signature is checked against, with gpgv
        #[arg(long, value_name = "FILE")]
        keyring: Option<String>,
        /// Accepted for symmetry with the install command; a plan reads an unsigned index anyway
        #[arg(long)]
        allow_unsigned: bool,
    },
    /// Print what `install tx-apt-sources` would do; reads the repository indexes
    TxAptSources {
        /// The APT configuration directory to read the repositories from
        #[arg(long, default_value = "/etc/apt", value_name = "DIR")]
        root: String,
        /// Architecture of the packages
        #[arg(long, default_value = "amd64")]
        arch: String,
        /// Package to install; may be given several times
        #[arg(long)]
        name: Vec<String>,
        /// Binary keyring file the repository signature is checked against, with gpgv
        #[arg(long, value_name = "FILE")]
        keyring: Option<String>,
        /// Accepted for symmetry with the install command; a plan reads an unsigned index anyway
        #[arg(long)]
        allow_unsigned: bool,
    },
    /// Print what `install tx-rpm-repo` would do
    TxRpmRepo {
        /// Base URL the package paths of the index are relative to
        #[arg(long, value_name = "URL")]
        base_url: String,
        /// The primary.xml index: a file path or a URL
        #[arg(long, value_name = "FILE-OR-URL")]
        primary: String,
        /// Package to install; may be given several times
        #[arg(long)]
        name: Vec<String>,
    },
    /// Print what `install tx-rpm` would do; reads the repository index
    TxRpm {
        /// Repository URL (the directory that holds repodata/)
        #[arg(long, value_name = "URL")]
        base_url: String,
        /// Package to install; may be given several times
        #[arg(long)]
        name: Vec<String>,
        /// Binary keyring file the repository signature is checked against, with gpgv
        #[arg(long, value_name = "FILE")]
        keyring: Option<String>,
        /// Accepted for symmetry with the install command; a plan reads an unsigned index anyway
        #[arg(long)]
        allow_unsigned: bool,
    },
    /// Print the commands that create and enter a container for an image, with the engine found on this host
    Container {
        /// Container image, for example debian:stable
        #[arg(long)]
        image: String,
        /// Name of the container (default: derived from the image)
        #[arg(long)]
        name: Option<String>,
    },
    /// Say which yapacc commands handle a package of a given format
    Cross {
        /// Package name
        name: String,
        /// Package format: deb | rpm | flatpak | snap | appimage | arch
        #[arg(long, default_value = "unknown")]
        format: String,
    },
    /// Print the snapshot tool detected and the stages `safety apply` would go through
    Safety {
        /// Package names the change would be about
        #[arg(value_name = "PKG")]
        targets: Vec<String>,
    },
}

#[derive(Subcommand, Debug)]
pub enum SafetyVerb {
    /// Plan, optional btrfs snapshot, pre-check, gated change, optional health check
    Apply {
        /// The change: arch-install | arch-remove | arch-update | arch-upgrade | flatpak-install | flatpak-remove | flatpak-update
        #[arg(long)]
        op: String,
        /// Packages or flatpak applications the change is about
        #[arg(value_name = "TARGET")]
        targets: Vec<String>,
        /// Go through yay, so that AUR packages are found too
        #[arg(long)]
        aur: bool,
        /// Take a read-only btrfs snapshot with this tag before the change
        #[arg(long, value_name = "TAG")]
        snapshot_tag: Option<String>,
        /// Skip the pre-check of the transaction
        #[arg(long)]
        skip_pretest: bool,
        /// Shell command to run after the change; a non-zero exit counts as a failed change
        #[arg(long, value_name = "COMMAND")]
        health_check: Option<String>,
        /// After a failed install, remove the targets this run installed (`pacman -R`); nothing else is rolled back
        #[arg(long)]
        auto_rollback: bool,
        #[command(flatten)]
        gate: Gate,
    },
}
