# Notes per distribution

What is specific to each system: which commands yapacc runs, where the
history comes from, and the things worth knowing before you rely on it.

## Arch Linux (pacman)

- **Changes** run `pacman -S`, `pacman -Rs` and `pacman -Syu`, always with
  `--noconfirm`: you answer once, at yapacc's gate, with pacman's plan in
  front of you. yapacc never runs `pacman -Sy` followed by an install: a
  partial upgrade is the usual way to break an Arch system. `yapacc
  refresh` exists and prints that warning.
- **Timeline** reads `/var/log/pacman.log`.
- **Undo** looks in `/var/cache/pacman/pkg` first, then downloads from the
  Arch Linux Archive with the package's signature; a package whose
  signature is missing or does not verify stops the undo. A restored
  package is upgraded again by the next `pacman -Syu` unless you
  add it to `IgnorePkg`.
- **Briefing** shows the Arch news feed, which is where manual
  interventions are announced. Security labels need `arch-audit`. The
  pending list comes from `checkupdates` (pacman-contrib) when it is
  installed, which does not touch the system's sync database; when it is
  installed and fails, the briefing says the list is not current and why.
- **Before the first `pacman -Syu`** (a fresh image, a new container)
  there is no sync database. `list` then cannot tell which packages are
  foreign and says so instead of calling all of them foreign. `check` and
  `search` say "no repository metadata on disk" and name `pacman -Syu`;
  `check` does not ask the AUR in that state, because it cannot tell which
  installed packages come from there and would send every name.
  `install repo` is refused with exit code 6 and the same advice.
- **Network:** on Arch, `check` asks the AUR about packages that are not in
  the repositories, and `get` asks it about the name you give. See
  [safety and privacy](safety.md#what-leaves-the-machine).
- **libalpm.** The release binaries talk to pacman through its command and
  are not linked against libalpm. A few specialist commands (`install
  alpm`, `inspect alpm …`) exist only in a build made with the `alpm`
  feature; in any other build they exit with code 6 and name the command
  that does the same through pacman.
- **The AUR** is available through specialist commands (`yapacc install
  aur NAME`): the PKGBUILD is shown for review before anything is built,
  and the build runs in a bubblewrap sandbox. The name is looked up first:
  one that is not in the AUR is "not found" (exit code 4) before anything
  is cloned, and when aur.archlinux.org cannot be reached the command stops
  with exit code 6. `aur pkgbuild NAME` needs only `curl`. See
  [every command](commands.md#arch-only).

## Debian and Ubuntu (apt)

- **Changes** run `apt-get install`, `apt-get remove`, and for an upgrade
  `apt-get update` then `apt-get upgrade --with-new-pkgs`. That installs new
  kernels and never removes a package; a release upgrade that needs
  removals is still `apt full-upgrade`, by hand.
- **Timeline** reads `/var/log/dpkg.log` and its rotated, compressed
  predecessors. dpkg logs local time without a zone, so entries are
  labelled `(local)`. A fresh cloud image has no `dpkg.log` until the
  first change; `doctor` says so.
- **Undo** tries the configured repositories, then
  `/var/cache/apt/archives`, then the distribution's archive:
  snapshot.debian.org on Debian (the file is checked against the hash the
  service records) and Launchpad on Ubuntu (not verified, and the output
  says so). It refuses to remove the kernel the machine is running.
- **Multiarch** names are kept as they are: `libfoo:i386` stays
  `libfoo:i386` through `undo`, and `why` answers for a package of a
  foreign architecture with its own reason and origin. Three gaps remain:
  `why libfoo:i386` is refused as a name (ask for `libfoo`), `list` shows
  a package installed for two architectures as two rows that look alike,
  and `timeline` drops the architecture suffix.
- **Briefing** marks updates that come from the security repository
  (`-security` on both; on Ubuntu an update offered by `-security` and
  `-updates` at once counts). There is no machine-readable news feed for
  Debian or Ubuntu, and the briefing says so.
- **Why** tells an orphan from a dependency that apt keeps because another
  package recommends or suggests it: only the first is in
  `yapacc inspect orphans`, and only the first would go with
  `apt-get autoremove`.
- **Restart** honours `/run/reboot-required`. On a Debian 13 machine,
  after the C library was reinstalled, it listed the same services as
  `needrestart`.

## Fedora, Rocky Linux and relatives (dnf)

- Both dnf 4 (Rocky 9, RHEL 9 family) and dnf5 (Fedora) are handled; their
  history output differs and both are parsed.
- **Changes** run `dnf install`, `dnf remove` and `dnf upgrade --refresh`.
  dnf can only simulate as root, so as an ordinary user the simulation runs
  through `sudo`; in a run that may not ask for a password it runs behind
  `sudo -n` when that works, and is left out, with a line saying so, when
  it does not. The simulation (`--assumeno`) always ends with an error
  status, so yapacc reads dnf's messages, which it asks for in English
  (`LC_ALL=C`): a request dnf cannot resolve is refused before the gate.
  That includes the removal of a package dnf protects, such as `sudo`.
- **Read commands never fetch.** Left alone, dnf downloads repository
  metadata whenever its cache is stale, even for a query. yapacc asks it
  for cached data only (`-C`), and on a machine with no metadata on disk it
  says so and names `dnf makecache` instead of answering from nothing.
- **Timeline** reads dnf's own history database.
- **Clean** reports downloaded packages and repository metadata as two
  items, with `dnf clean packages` and `dnf clean metadata`. dnf5 also
  copies the repository metadata into `~/.cache/libdnf5` whenever an
  ordinary user asks it a question (84 MiB on a fresh Fedora 45), and
  `check`, `search` and `briefing` make it do so. yapacc cannot prevent
  that: pointed at the system cache, dnf5 fails for an ordinary user,
  because it writes there. `clean` lists the copy as an item of its own
  when it exists.
- **Undo** is dnf's own `dnf history undo` for the transaction shown, behind
  the gate. Before asking, yapacc checks that every old version is still in
  an enabled repository, because dnf would otherwise fail half-way through
  the question. There is no archive fallback: once the repository drops a
  version, that upgrade cannot be undone.
- **Briefing** takes security labels from `dnf updateinfo`.
- **Restart** uses `needs-restarting -r` for the reboot verdict.

## openSUSE Leap and Tumbleweed (zypper)

- **Run `undo` and `timeline` with `sudo`.** `/var/log/zypp/history` is
  readable by root only. openSUSE's `sudo` asks for the *root* password by
  default, and `/usr/local/bin` is not on its path, so install the binary
  into `/usr/bin` or give the full path.
- **Changes** run `zypper install`, `zypper remove`, and for an upgrade
  `zypper dist-upgrade` on Tumbleweed and `zypper update` on Leap.
- **dnf installed beside zypper** (openSUSE packages it) does not make
  the machine a dnf system: zypper stays the native manager, and
  `yapacc platform` lists dnf under `other managers`.
- **Read commands never fetch.** As root, zypper refreshes repositories on
  its own before answering. yapacc calls it for repository data only when
  every enabled repository has metadata on disk; otherwise it names the
  repositories that lack it and tells you to run `zypper refresh`.
- **Why** gives the repository that offers the package now. The zypp
  history does not record where the installed version came from, and the
  `from` line says so.
- **Undo** finds old versions in the repositories only. On Tumbleweed they
  are gone within a day or two of an upgrade; on Leap they stay longer.
  Kernels are multiversion packages: undoing a kernel upgrade removes the
  new kernel and leaves the old one, and is refused while the new kernel
  is the one running.
- **Packages with no earlier record.** libzypp writes an upgrade and a new
  install the same way. When undoing a whole upgrade, yapacc names the
  packages it cannot classify and asks at the terminal whether to remove
  them. `--yes` does not answer that question.
- **Snapshots** through snapper are the natural safety net here; see
  [snapshots](snapshots.md).
- **Restart** honours `/run/reboot-needed`.

## Alpine Linux (apk)

- Alpine uses `doas`, and yapacc uses it when there is no `sudo`. With
  opendoas, `persist` is tied to the calling process, so each yapacc run
  asks for the password once.
- **Changes** run `apk add`, `apk del`, and `apk update` then `apk upgrade`.
- **Timeline** reads `/var/log/apk.log`. Only apk-tools 3 writes that file,
  and only when run as root; on older Alpine releases there is no history,
  and `timeline` says so.
- **No undo.** yapacc does not reverse apk transactions.
- A run made with `apk --quiet` is logged without its package lines, so
  `timeline` cannot show what it changed. It counts those runs in a note.
- Use the static binary: Alpine is a musl system and the release archive
  is built for exactly that.

## Void Linux (xbps)

- **Changes** run `xbps-install`, `xbps-remove` and `xbps-install -Syu`.
- **No timeline and no undo.** xbps keeps no log of transactions. `doctor`
  reports both as unavailable on Void, with that reason.
- Void's runit is detected; `restart` lists programs holding replaced
  files and tells you to restart services with `sv restart NAME`.

## FreeBSD (pkg)

- Build yapacc on FreeBSD itself; see [install](install.md#freebsd).
- **Changes** run `pkg install`, `pkg delete` and `pkg upgrade`. These
  cover packages, not the base system: `freebsd-update` and pkgbase
  upgrades of the base system remain yours to run. `pkg install -n` ends
  with an error status whenever there is something to install, so yapacc
  cannot judge the simulation by its status; it asks the catalogue on disk
  whether the name exists and refuses an unknown one before the gate, and
  it refuses when pkg itself says no package matches the name.
- **Timeline** reads the `pkg` lines in `/var/log/messages`. syslog stamps
  have no year and no zone; yapacc works the year out and keeps the time as
  written.
- **Restart** compares `freebsd-version -k`, `-r` and `-u` for the reboot
  verdict. Listing processes that hold replaced files needs the
  `sysutils/checkrestart` package.
- **Snapshots** on a ZFS root are taken with `zfs snapshot` of the root
  dataset. yapacc does not create boot environments; `bectl` is the tool
  for that.
- **No undo.**
- With no network, `pkg` itself can wait for minutes before giving up, and
  yapacc waits with it. The read commands answer from the catalogue on
  disk at once.
- `sudo` and `doas` both work. On FreeBSD `doas` with no terminal would
  wait forever for a password, so yapacc calls it non-interactively
  whenever nobody could type one.

## Everywhere else

Run `yapacc platform` and `yapacc doctor`. They do not guess: an unknown
system gets "unknown" and the commands that cannot work there say so
instead of trying.
