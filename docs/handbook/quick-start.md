# Quick start: a ten-minute tour

Every command on this page is read-only. None of them writes a file, needs
root or changes a package, so you can run them on a machine you care about.

## 1. Ask what works here

`doctor` prints one line per check. It is the first thing to run on a new
machine, and the first thing to attach to a bug report.

```console
$ yapacc doctor
ok    platform              — apt: tier 1, a main target
ok    native-manager        — apt lists 324 installed packages
ok    config                — /home/tester/.config/yapacc/config.toml (not present, defaults active)
n/a   component:yapacc-tui  — not installed (optional: the interactive interface)
n/a   component:yapacc-mcp  — not installed (optional: the MCP server for AI assistants)
n/a   sandbox               — only used for source builds on pacman systems
n/a   ledger:user           — no journal (nothing recorded yet)
n/a   ledger:system         — no journal (nothing recorded yet)
n/a   tool:flatpak          — not on PATH (only needed for Flatpak applications)
n/a   tool:gpgv             — not on PATH (only needed to check a repository signature: install tx-apt, tx-rpm with --keyring)
n/a   tool:bwrap            — not on PATH (only used for source builds on pacman systems)
ok    repo-index            — repository indexes are on disk
ok    tool:curl             — on PATH
ok    undo                  — old versions from the configured repositories, then the apt cache, then snapshot.debian.org
ok    backend-maturity      — apt: exercised on virtual machines of Debian 13 and Ubuntu 24.04; not on real hardware
ok    timeline              — reads /var/log/dpkg.log and its rotated predecessor
ok    privilege             — changes run through sudo
ok    briefing              — security-update labels and reboot hints; no distribution news feed
```

`warn` means something a feature of this system uses is missing, and the
line says what you lose. `fail` means a command will not work, and the line
says why. `n/a` means the check does not apply here, or an optional part is
not installed. A healthy machine shows `ok` and `n/a` only.
[More about doctor](doctor.md).

## 2. Ask why something is installed

Pick a package you do not remember installing.

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
  packager     Debian Curl Maintainers <team+curl@tracker.debian.org>
  ledger       no yapacc record (installed outside yapacc, or by a native lane) — the facts above come straight from the package manager
```

The `because` line is the chain from this package back to something you
asked for. [More about why](why.md).

## 3. Ask what changed

```console
$ yapacc timeline --limit 5
2026-11-19 13:08 (local)  apt     upgrade    linux-image-amd64  6.12.107-1 -> 6.12.117-1
2026-11-19 13:08 (local)  apt     upgrade    libpng16-16t64  1.6.48-1+deb13u5 -> 1.6.48-1+deb13u6
2026-11-19 13:08 (local)  apt     upgrade    libcurl4t64  8.14.1-2+deb13u5 -> 8.14.1-2+deb13u7
2026-11-19 13:08 (local)  apt     upgrade    curl  8.14.1-2+deb13u5 -> 8.14.1-2+deb13u7
2026-11-19 13:08 (local)  apt     upgrade    libbrotli1  1.1.0-2+b7 -> 1.1.0-2+deb13u1
```

When something broke after a reboot, `yapacc timeline --since-boot` shows
only what changed since the machine last started.
[More about timeline](timeline.md).

## 4. Ask what the next upgrade will do

```console
$ yapacc briefing
Upgrade briefing — 25 package(s) pending (apt)

Needs attention
  reboot  new kernel — reboot to use it: linux-image-6.12.117+deb13-amd64 6.12.117-1, linux-image-amd64 6.12.107-1 -> 6.12.117-1
  care    the package manager itself — let it finish alone; do not interrupt: apt 3.0.3 -> 3.0.4
  care    remote access — if you are connected over ssh, keep a second session open until you have reconnected: libssl3t64 3.5.7-1~deb13u2 -> 3.5.7-1~deb13u3, openssl 3.5.7-1~deb13u2 -> 3.5.7-1~deb13u3

Security fixes (24)
  liblzma5
  openssl-provider-legacy
  zlib1g
  libssl3t64
  …

Disk
  /: 17.7 GiB free

Safety net
  no snapshot tool detected: `yapacc undo` can restore package versions, not configuration or data
note: pending list is as of the last repository refresh; yapacc does not refresh on a read command
note: news: this distribution has no machine-readable upgrade announcements yapacc reads; check its release notes for manual steps

Next: `yapacc upgrade` shows the native manager's plan and asks before changing anything.
```

`briefing` reads the list of pending updates as of your last repository
refresh. It does not refresh on its own. [More about briefing](briefing.md).

## 5. See what an undo would do, without doing it

`--dryrun` plans and never applies. On `undo` it shows the last transaction,
the reverse of it, and the native manager's own simulation.

```console
$ yapacc undo --dryrun
Last transaction: 2026-11-19 13:08 (local)  (apt-get install -y -- curl)
  upgraded    libcurl4t64:amd64 8.14.1-2+deb13u5 -> 8.14.1-2+deb13u7
  upgraded    curl:amd64 8.14.1-2+deb13u5 -> 8.14.1-2+deb13u7

To undo it:
  restore curl:amd64 8.14.1-2+deb13u5  (from the configured repositories)
  restore libcurl4t64:amd64 8.14.1-2+deb13u5  (from the configured repositories)

apt's own simulation (apt-get install -s -y --allow-downgrades -- curl:amd64=8.14.1-2+deb13u5 libcurl4t64:amd64=8.14.1-2+deb13u5):
…
The following packages will be DOWNGRADED:
  curl libcurl4t64
0 upgraded, 0 newly installed, 2 downgraded, 0 to remove and 22 not upgraded.
Inst curl [8.14.1-2+deb13u7] (8.14.1-2+deb13u5 Debian:13.7/stable [amd64]) []
Inst libcurl4t64 [8.14.1-2+deb13u7] (8.14.1-2+deb13u5 Debian:13.7/stable [amd64])
Conf curl (8.14.1-2+deb13u5 Debian:13.7/stable [amd64])
Conf libcurl4t64 (8.14.1-2+deb13u5 Debian:13.7/stable [amd64])

Will run:
  sudo apt-get install -y --allow-downgrades -- curl:amd64=8.14.1-2+deb13u5 libcurl4t64:amd64=8.14.1-2+deb13u5
note: Undo restores package versions only. It does not restore configuration files or data that package scripts changed; a filesystem snapshot does.

DRY-RUN: nothing was changed. Run again without --dryrun to apply.
```

A dry run writes nothing. When an old version is in no local cache, the
plan says which archive it would come from, and the download happens only
after you have agreed.

[More about undo](undo.md).

## 6. Housekeeping, still read-only

```console
$ yapacc clean      # what can be reclaimed, with the command for each
$ yapacc restart    # services and programs still running replaced code
$ yapacc configs    # configuration files an upgrade left unmerged
```

None of the three changes anything. Each prints the command you would run
yourself. [More about them](housekeeping.md).

## Where to go next

- To let yapacc make a change, read [the gate](gate.md) first, then
  [install, remove, upgrade](changes.md).
- For scripts, [JSON output and exit codes](scripting.md).
- The same documentation offline: `yapacc docs`.
