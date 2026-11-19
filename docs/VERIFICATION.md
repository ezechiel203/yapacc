# Verification log

What was run, on which system, with what result. This is the evidence
behind [STATUS.md](../STATUS.md): a cell there may say "VM" only if a
paragraph here names the release and the date.

Every paragraph is dated and describes one pass. Nothing here is a promise
about other systems. "VM" means a booted QEMU/KVM virtual machine built by
[`contrib/test/vm/vm.sh`](../contrib/test/vm/README.md) from the
distribution's public cloud image; "container" means a rootless podman
container with the static binary mounted into it. No run here was on real
hardware.

Bugs found by a pass are listed with the pass that found them, because a
fixed bug says as much about coverage as a clean run.


## Release candidate round (2026-11-19)

One static musl build of commit `2ea00fa` (after the pre-release review and
its fixes) was run on booted virtual machines of nine systems, as `tester`
through sudo unless stated, with prompts answered on a real pseudo-terminal.
On FreeBSD the same commit was built on the machine itself. What this round
found is listed with each system; the defects were fixed after the round,
and those fixes were checked by unit tests and in containers, not by a second
VM round.

**Debian 13.** Every read command ran and `--json` was valid on 22 of them;
none created a file under the home directory. `why perl` named `cowsay` as
the package that needs it (the `perl:any` case). The gate: `--dryrun`,
`--script` (exit 3), a pipe (exit 3), `n` and a bare Enter at the prompt
(exit 3, nothing installed), `y`, `--yes`. A package that does not exist was
refused after apt's simulation failed (exit 3, apt history unchanged), and
eight hostile names were refused with exit 2 before anything ran. `undo` of
an install; `undo` of an upgrade whose old versions were only on
snapshot.debian.org: the dry run named the archive and the sizes and wrote
nothing, the real run asked before downloading and again before undoing,
restored the old versions, left `dpkg -l` identical to the state before the
upgrade and removed its download directory. Multiarch: after installing
`libpng16-16t64:i386`, `undo --yes` removed exactly the seven i386 packages
of that transaction and no amd64 line of `dpkg -l` changed. An undo with a
dependent installed afterwards was refused (exit 3). With sudo requiring a
password, `--script install repo figlet --yes` failed in under a second with
the hint and changed nothing; at a terminal the sudo prompt appeared and
worked, and three wrong passwords changed nothing. MCP: 21 tools; with the
default configuration `yapacc_install` with `confirm: true` answered "PLAN
ONLY" and installed nothing; `apply = "allow"` in a file owned by the user
was ignored with a warning; in a file owned by root (0644) it applied. The
interactive interface started on a pseudo-terminal and exited 6 without one.
Found: after undoing an install, packages with configuration files stay in
dpkg's `rc` state (apt removes, it does not purge), so `dpkg -l` differs by
those lines while the set of installed packages is identical; `why` called a
package an orphan that apt keeps because another package suggests it.

**Ubuntu 24.04.** Read commands, the gate and `undo` of an install as above.
`undo` of an upgrade through Launchpad: the dry run named the two Launchpad
URLs with sizes and "NOT signature-verified" and downloaded nothing; the real
run restored the three old versions and `dpkg -l` was identical to the state
before the upgrade. launchpad.net answered with timeouts and a 503 on three
of four attempts; each time yapacc refused with exit 3 and changed nothing.
`strace` showed no network connection during `briefing --offline`,
`briefing`, `why`, `check` and `timeline`. Found: `briefing` labelled no
security update although 45 of 57 pending updates were in `noble-security`
(apt lists `noble-updates` first and only the first origin was read).

**Fedora 45 (dnf5 5.4.6) and Rocky Linux 9.9 (dnf 4.14).** The first run of
the read commands on booted dnf systems: all ran, `--json` valid on 19
commands as user and as root (on Rocky all but `inspect orphans` before the
first metadata download). The gate as on Debian. `undo --dryrun` named the
transaction ("Transaction to undo: ID 5") and ran nothing. The race the ID
is there for: with `yapacc undo` waiting at its question for ID 7, `sudo dnf
install -y zip` committed ID 8 in a second session; after `y`, `dnf history
undo 7 -y` ran, `tree` was removed and `zip` stayed (the same on Rocky with
IDs 9 and 10). On Rocky a real one-package upgrade was undone and the
package set was identical; with the repository disabled the undo was
refused from the dry run (exit 3). On Fedora a real upgrade of 138 packages
ran through `yapacc upgrade --yes`; its undo restored every package version
(`rpm -qa` identical to the state before the upgrade, clean reboot, no
failed unit) but dnf exited 1 because two `%posttrans` scriptlets failed
while libgcc was being downgraded, so yapacc reported a failed undo. Found:
on a machine with no dnf metadata on disk `inspect orphans` failed with a
bare "dnf exited with status 1" and `doctor` said the repository index was
there; `clean` reported dnf's metadata cache as "downloaded packages".

**openSUSE Tumbleweed (zypper 1.14.102).** The first run of the read
commands on a booted zypper system. `timeline` and `undo` as an ordinary
user exit 6 and name the root-only history. A real `zypper dist-upgrade`
through `yapacc upgrade --yes` (29 packages upgraded, 3 new, with glibc,
systemd and the kernel), then a reboot into the new kernel; `restart` listed
the same eight services and one program as `zypper ps -s`. Its undo was
refused (exit 3) because the old versions had left the repositories, and
nothing changed. A smaller dist-upgrade was undone from the repositories
and the package set was identical. Packages with no earlier record: the dry
run printed the note; `--yes </dev/null`, `--script --yes` and `echo y |`
all exited 3 with the set unchanged; at a terminal `n` exited 3 and `y`
undid. A package that does not exist was refused after zypper's simulation
(exit 3). Found: as root with no repository metadata on disk, read commands
(also `briefing --offline`) made zypper download it; with `--script --yes`
as an ordinary user the simulation was skipped; `briefing` did not flag the
pending `kernel-default-base`.

**Arch Linux (the pacman command-line lane, no libalpm).** The first run of
this lane on a booted machine. Read commands, also with `COLUMNS=60`.
`briefing --offline` made no `connect` call (strace). As an ordinary user
`install repo tree --dryrun` showed pacman's own `--print` preview and the
command; removal is `pacman -Rs`, and a package with an edited configuration
file left a `.pacsave`, which `configs` then listed. A real upgrade of four
packages (glibc among them; no kernel) through `yapacc upgrade --yes`; `undo
--dryrun` named four files from the Arch Linux Archive (11.5 MiB) and
downloaded nothing; `undo --yes` downloaded each package and its signature,
verified it with `pacman-key`, ran `pacman -U`, and `pacman -Q | sort |
md5sum` equalled the value before the upgrade (done twice); the download
directory was gone; the machine rebooted cleanly. AUR: the PKGBUILD was
shown, `n` exited 3 and nothing was built; with `--no-review --yes` the
package was built in the sandbox and installed. Seen from `build()` in the
sandbox: `HOME` was a throwaway directory, `/run` was empty, the user's home
held only the build directory, `/etc` and `/usr` were read-only and there
was no network. Found: an invalid package name on `install repo` exited 1
instead of 2; after `y` at yapacc's question pacman asked its own.

**Alpine 3.24.1 (apk-tools 3.0.6).** Read commands; `timeline` read
`/var/log/apk.log` (as user, through sudo and through doas); the gate
through sudo and through doas; a package that does not exist was refused at
`apk add --simulate` (exit 3, no `apk add` in the log); `undo` says it is
not available (exit 6). Found: a run made with `apk --quiet` is logged
without its package lines, so `timeline` does not show it.

**FreeBSD 15.1-RELEASE-p7 (pkg 2.9.2).** Built on the machine in 1 min 51 s
with the packaged rustc 1.98.1 (5,367,048 bytes). Read commands, the gate at
a terminal, `remove repo` of a package that is not installed refused after
`pkg delete -n` (exit 3). A package that does not exist is not refused
before the real run: `pkg install -n` cannot tell, `pkg install -y` runs
and fails (exit 1), and nothing changes.

Not run in this round: Void, openSUSE Leap, FreeBSD 14.5, Ubuntu's
privilege and MCP apply cases, the running-kernel guards, any snapshot tool
(no image had one configured), doas on anything but Alpine.

## After the release candidate round (2026-11-19)

The defects the round found were fixed in the commits that follow
`2ea00fa`. This section says how each group of fixes was checked. Unless a
VM is named, the check was a unit test plus a run of the static binary in a
rootless container of the distribution; the systems of the round were not
booted again.

**No fetch on a read command (zypper, dnf).** On Tumbleweed as root with
the zypp cache emptied, `strace -f -e trace=connect` counted 110 to 129
network connections for `briefing` before the fix and none after it, for
`briefing` (with and without `--offline`), `check`, `search`, `clean`,
`inspect orphans`, `--json check`, `get`, `why` and `doctor`; nothing new
appeared under `/var/cache/zypp/raw`. On fresh Rocky 9 and Fedora
containers `inspect orphans` exits 0 and `check`, `search` and `briefing`
print the "no repository metadata on disk" sentence. `doctor` warns about
the missing index and reports ok after a refresh.

**Consent given once (pacman).** In an `archlinux` container on a
pseudo-terminal, with `y` typed once at yapacc's question: an install as a
user with password-less sudo, a removal and an install as root, and an
upgrade as a user all finished with no other input. Not run with a sudo
that asks for a password.

**The simulation behind the elevation tool (dnf, zypper).** On Tumbleweed,
Rocky and Fedora containers, as a user with password-less sudo under
`--script`: the simulation ran behind `sudo -n`, and an unknown package
exited 3 on zypper. A user whose sudo needs a password got the "no
simulation" line. Not run with doas.

**What was changed, said from the packages.** Removing a package that is
not installed exits 4 on dnf 4, dnf 5, zypper, apt, apk and pacman. The
closing lines ("Nothing was changed: …", "No package was changed: …") are
unit-tested and were seen in the container runs.

**`undo` verifies its result.** Real undos on Rocky, Fedora, Debian (1 of
1) and Ubuntu (3 of 3) in containers ended with the "Verified" line. The
sentence for a native command that fails after restoring the versions is
unit-tested only. The archive lookups with their timeouts were seen running
on Ubuntu; no archive stayed silent during a lookup.

**Reports.** Ubuntu 24.04 container: 3 of 3 pending security updates
labelled (Debian: 1 of 1). Debian: the dependency that apt keeps because
another package suggests it is no longer called an orphan (reproduced by
lifting the image's `SuggestsImportant "false"`). Fedora with a cached rpm:
`clean` shows two items with two commands. Tumbleweed, with records
appended to a copy of the history: a version change downwards is labelled
`downgrade`. Alpine: the note counts three quiet apk runs from a real
apk-tools 3.0.9 log. Kernel names of openSUSE flavours, Void, Alpine and
FreeBSD pkgbase are unit-tested only.

**Snapshots.** Debian and Arch containers, five cases each: no tool, a
named snapper that is absent under `auto` and under `auto-best-effort`, and
a stand-in for snapper that succeeds and one that fails. Real snapper,
timeshift and zfs were not used for these cases. `snapshot list` refused
for lack of privileges exits 6: seen on a btrfs root as an ordinary user.

**FreeBSD.** The catalogue check before an install (`pkg rquery`) was
written from pkg-rquery(8) and is unit-tested. It was not run on FreeBSD.

**Version comparison.** `vercmp` as pacman does it and as rpm does it:
66 pairs answered by `vercmp` (pacman 7.1.0) and 103 by `rpm.vercmp`
(rpm 6.1.0) are held in the tests.

**The MCP server.** Over standard input and output on Debian:
`yapacc_install` of an unknown name answers `PLAN FAILED` and sets
`isError`; a known one answers `PLAN ONLY`.

**The read side, reviewed statically.** A review of the backends, the log
parsers and platform detection found seventeen defects on uncommon inputs,
each reproduced before it was fixed: openSUSE with the `dnf` package
installed was taken for a dnf system (Tumbleweed container); orphaned AUR
packages were missing from search because their maintainer is `null` (59
of 68 results before, 68 after, against the real RPC); `yapacc sources`
panicked on a non-ASCII repository id; `clean` made flatpak create a
repository directory in an empty home; an empty `XDG_CONFIG_HOME` read a
configuration file from the current directory; apt packages of a foreign
architecture got the wrong reason and origin (debian:stable with i386);
dpkg reinstalls and downgrades were shown as upgrades (the comparison is
held against 29 answers of `dpkg --compare-versions`); dnf 4 local times
were labelled UTC (Rocky 9 with a zone set); `why` took 68 seconds on a
long apt chain (4 after); on Arch before the first `pacman -Sy`, `list`
called every package foreign. Thirteen read commands were run with an
empty home directory, and after the fixes none writes a file. Not run for
this review: FreeBSD, WSL, LXC, ostree systems, a real Manjaro machine.

**Debian 13 VM, the session behind the handbook's examples.** A booted VM
from the cloud image of 2026-09-14 (kernel 6.12.107), as `tester` through
sudo, every command on a pseudo-terminal: `check` before any
`apt-get update` (the "no repository metadata" note); `platform`, `doctor`,
`briefing` (25 pending, 24 of them security fixes, the kernel, apt and
OpenSSL flagged), `check`, `why`; `install repo curl` answered `y` (an
upgrade of two packages), `undo --dryrun` and `undo` (2 of 2 verified, old
versions from the configured repositories); `install repo figlet` with
`--dryrun`, answered `n` (exit 3), under `--script` (exit 3) and answered
`y`; an unknown package (exit 3 after apt's simulation failed) and the
removal of a package that is not installed (exit 4); `upgrade` answered `y`
(24 upgraded, the new kernel installed); `timeline`, `restart` as the user
(reboot: yes, 16 processes not examined) and as root (nine services, three
programs), `configs`, `clean`; `undo` of the whole upgrade answered `y`:
24 packages downgraded and the new kernel removed, "Verified: 25 of 25
package(s) are at the planned state", 25 updates pending again; `upgrade
--yes` again, a reboot into 6.12.117, then `timeline --since-boot` (the
previous boot, with the boot marker) and `restart` (reboot: no, nothing to
restart). Found in this session and fixed afterwards: dpkg logs a downgrade
as an upgrade and `timeline` repeated that; `doctor` reported absent
optional components as warnings; `inspect orphans` printed an empty line
when there was none.


**Four more booted systems, with the build of `a43efb9`.** FreeBSD
15.1-RELEASE-p7 (pkg 2.9.2, yapacc built on the machine), Fedora 45 (dnf5
5.4.6), openSUSE Tumbleweed 20261111 (zypper 1.14.102) and Arch (image of
2026-11-15, pacman 7.1.0), as `tester` through password-less sudo.
FreeBSD: the catalogue check ran for the first time: an unknown name is
refused with exit 4, as the user, as root and with `--yes`; a real package
is not refused and pkg's simulation is shown; a real install and removal
with `--yes`; `undo --dryrun` says undo is not available with pkg (exit
6). Fedora and Tumbleweed before any refresh: `check`, `search`,
`briefing`, `inspect orphans`, `clean`, `why` and `doctor` created no file
and sent no packet (nftables counters at zero), and name `dnf makecache`
or `zypper refresh` where it applies. On Fedora, Tumbleweed and Arch an
install answered `y` on a pseudo-terminal, then `undo`, ended with
"Verified: 1 of 1 package(s) are at the planned state". On Arch one answer
was enough: pacman printed its own question and went on without waiting.
Removing a package that is not installed exits 4 on all four. Not run: a
real `upgrade` on these four (140 packages were pending on Fedora, 30 on
Tumbleweed), a sudo that asks for a password.

**What that round found, and how each fix was checked** (commits
`a6eaee4`, `6ba9b5f`, `00da892`, `8756610`). None of it was run on a booted
machine again; every check below was a container, as `tester` through
password-less sudo unless said otherwise.
dnf: an unknown package ended a dry run with "Run again without --dryrun
to apply" (exit 0) and `--yes` ran the real command (exit 1). dnf's
messages are now read: an install or a removal dnf cannot resolve is
refused with exit 3. Checked on `fedora:latest` (dnf5 5.4.8) and
`rockylinux:9` (dnf 4.14): an unknown name, a conflict (`coreutils-single`
against `coreutils`), a normal install and `upgrade --dryrun` (exit 0), a
real install and removal of `tree`, and the removal of a name that is not
installed (still exit 4). The removal of a protected package (`sudo`) is
now refused at the simulation; before, the real command failed.
Arch with no sync database: `check` and `search` answered as if nothing
were pending or found, and the install plan sent the user to the AUR. They
now say "no repository metadata on disk" and name `pacman -Syu`; `check`
no longer sends every installed name to the AUR in that state;
`install repo` is refused with exit 6. Checked in an `archlinux` container
before `pacman -Sy`, and after it for the unchanged behaviour, with a real
install and removal.
AUR names: the hint named commands that do not exist; `aur pkgbuild`
failed without yay; `install aur` of an unknown name planned a clone. Now
the hint names `search NAME --all` and `install aur NAME`, `aur pkgbuild`
needs only curl, and a name that is not in the AUR is exit 4 before
anything is cloned. Checked in the same container without yay, and with
the network cut (`--network=none`): exit 6, "could not be reached".
openSUSE: `timeline` as an ordinary user listed the logs of other package
managers. It now names `/var/log/zypp/history` as existing and closed to
this user, and `platform` marks it. Checked in a `tumbleweed` container
whose `/var/log/zypp` is `drwxr-x--- root`.
Fedora: `clean` did not list `~/.cache/libdnf5`. It does now. Pointing
dnf5 at the system cache instead was tried and fails for an ordinary user
("Permission denied" on a temporary file there), so the copy is not
prevented.
FreeBSD: with no catalogue on disk the simulation was empty, and a root
dry run of an unknown name exited 0. pkg's "No packages available to
install matching" is now a refusal (exit 4), and an empty simulation gets
a note naming `pkg update`. **Not run on FreeBSD**: unit tests on the lines
recorded during the round, and a stand-in `pkg` printing those lines. Not
addressed: as an ordinary user with no catalogue, `--yes` on an unknown
name still exits 1.
apt: nothing was to change there. The builds of `a43efb9` and of
`8756610` were compared over 24 commands, as root and as a user, in
`debian:stable` and `ubuntu:24.04` containers: the outputs differ only in
the text of two built-in `docs` topics and in values that change from run
to run. The handbook's examples, taken with the build of `a43efb9`, were
therefore not taken again.
The final code passes `cargo xtask ci` (all four jobs; 680 tests, static
core 5,929,072 bytes). One earlier run during these fixes failed in the
`portable` job (a test run ended with status 101) while the machine was
under heavy load from unrelated builds; only the end of that log was kept,
so the failing test is not known. The same command then passed five times
and the whole of `cargo xtask ci` twice. A test that fails under load is
therefore possible and was not found.


## Debian and Ubuntu (apt)

**apt lane, real VMs** (QEMU/KVM, Debian 13 and Ubuntu 24.04, as `tester` with sudo and as root, 2026-11-17): `platform`, `doctor`, `why`, `list`, `info`, `search`, `check`, `timeline`, `briefing`, `clean`, `restart`, `configs`, `names`, gated install/remove/upgrade and `undo --dryrun/--yes` ran; the pty `[y/N]` prompt (y and n) and a sudo password prompt were driven. `undo` of a full `apt upgrade` including a kernel upgrade (Debian 13 via trixie-backports, reboot into the new kernel; Ubuntu 24.04, 56 packages), Launchpad fallback (38 downloads), snapshot.debian.org fallback (apt cache emptied; the downloaded file's SHA-1 matched), `rc` state. Hash mismatch (a local TLS server impersonating snapshot.debian.org, test CA installed in the VM) is refused with nothing changed; a version missing from every archive is refused (exit 3). `restart` on Debian 13 after a libc6 reinstall lists the same 11 services as `needrestart -b`; reboot-required and the briefing kernel check were exercised. `timeline` reads rotated `dpkg.log.N.gz` (a real logrotate run; an unreadable or corrupt .gz gives a note). Bugs found there and fixed: SIGPIPE panic when piped to `head`; briefing missed kernel upgrades (plain `apt upgrade` simulation keeps them back, now `--with-new-pkgs`); undo could remove the running kernel (now refused); hostile file names with ESC/newline in `configs` and `restart` (now escaped). Not tested: apt `history.log` as a timeline source (timeline does not read it), checkrestart output was not compared (it only printed Python warnings), held packages through undo, Ubuntu 26.04, `restart` on Ubuntu.

**`undo` via Launchpad** (Ubuntu): run in an Ubuntu 22.04 container (single package and a six-package upgrade; one apt bug found and fixed). Not run on 26.04, non-amd64 or Mint-like derivatives; a version missing from every archive and a hash mismatch were not exercised. Old `.deb` files from snapshot.debian.org are hash-checked but not signature-verified; Launchpad downloads are not signature-verified either. Superseded in part by the apt-lane entry below, which records a Launchpad run on a booted Ubuntu 24.04 VM.


## Fedora and Rocky Linux (dnf)

**dnf on real booted VMs (Fedora 45 dnf5, Rocky Linux 9.9 dnf 4.14; observed 2026-11-17, QEMU/KVM, as `tester` with sudo and as root)**: `install`/`remove`/`upgrade`/`undo` ran for real; the batched `dnf repoquery --available` check and the dnf 4 history wording were observed. Bugs found and fixed: dnf 4 `timeline` was empty (`Packages Altered`, `Removed`, `|` list rows and C-locale `Begin time` were not parsed); `undo --dryrun` as non-root wrongly refused on a stale per-user dnf 4 cache (now re-checked as root, including with `--script` when sudo/doas is available); `restart` hid why `needs-restarting -r` failed; `yapacc ... | head` panicked (now exits quietly); a command containing `|` broke the dnf 4 history row (now unit-tested). A kernel upgrade on Fedora set the reboot-needed signal and a real reboot cleared it. The [y/N] prompt (y and n) and the sudo password prompt (correct password) were driven through a pty. Round 2 (2026-11-18, one VM at a time, 3 GiB, locally built noarch RPMs `ytest-epoch` (`Epoch: 1`, 1.0 and 2.0) and `ytest-gone` served by `createrepo_c` + `python3 -m http.server`): on Rocky 9.9 (dnf 4.14) and Fedora 45 (dnf5 5.4.6) an upgrade 1:1.0 -> 1:2.0 was undone with `undo --dryrun` and `--yes` (the epoch-bearing NEVRA from `dnf history info` matches `dnf repoquery --available`; dnf downgraded it); with 1.0 deleted from the repository and the repodata rebuilt, `undo` is refused with "ytest-epoch-1:1.0-1.noarch is in no enabled repository"; a removal (`Removed ytest-gone-1.0-1.noarch` on dnf 4, `Remove ytest-gone-0:1.0-1.noarch` on dnf5) whose package was deleted from the repository is refused likewise, so the real `Remove`/`Removed` wording is parsed. A dnf5 install of `kernel-core` 7.3.4 (plus `kernel-modules-core`) was undone for real (both removed, running 7.2.11 kept). Bug found and fixed: as an ordinary user the availability check read that user's own dnf metadata cache, which can be stale in either direction (Rocky: a version deleted from the repository was still listed, so the dry run passed and dnf would refuse at the real run); it now runs through sudo/doas as root (`sudo -n` in a dry run or `--script`, so a dry run never asks for a password) and falls back to the user's cache only when root cannot be used. A refusal in that fallback says it was checked as an ordinary user. Observed with no fix needed: root's own cache is stale too until `dnf clean all`/expiry (a package deleted from a local repo was still listed as root until the cache was cleaned, and dnf would use the same cache, so it is not a yapacc fault); wrong sudo passwords over `ssh -tt` (three wrong attempts, then "sudo: 3 incorrect password attempts" and `error: sudo dnf history undo last -y exited with status 1`, exit 1, nothing changed; a wrong password in the availability check falls back to the user check and the undo step asks again); on a Rocky host with sudo moved away and no doas, the undo of a missing version is refused from the user check. `restart` as root and as `tester` on Fedora 45: `dnf -C --disablerepo=* needs-restarting -r` exited 0 ("Reboot should not be necessary") and yapacc said `Reboot: no`; after installing a newer `kernel-core` it exited 1 ("Reboot is required ... kernel-core") and yapacc said `Reboot: yes` with that line among the reasons; the earlier "unknown" is not reproduced with the current call (the cause of the old answer was not isolated). Not observed: `doas` on dnf, an undo whose `dnf history undo` itself fails after the check passed, no-sudo undo of an upgrade that is still available.


## openSUSE (zypper)

**zypper on real booted openSUSE VMs (Leap 16.1, Tumbleweed; observed, 2026-11)**: a real `zypper dup`/`up` then `sudo yapacc undo --dryrun/--yes`; `zypper_uncertain_installs` guard; hostile names and offline runs; y/n and sudo-password prompts over `ssh -tt` (SUSE sudo asks for the root password: `targetpw`; `/usr/local/bin` is not in its secure_path, so `sudo yapacc` needs the binary in /usr/bin); `configs` found both `.rpmnew` and `.rpmsave`; `restart` and `briefing` ran as root and as tester. Bugs found and fixed: (1) multiversion packages (`multiversion =` in zypp.conf, e.g. kernel-default): undoing an upgrade planned a restore of the old version, which zypper reports as already installed, so "Undone" left the new version in place; it now plans `zypper remove name=newver` (fixed; verified on a fake multiversion RPM pair and on the real `kernel-default-base` 7.3.3 -> 7.3.4 upgrade of the Tumbleweed image, since `kernel-default` conflicts with it; a second round, 2026-11-18, found that the old version is looked up in the repositories before the multiversion check, so undoing a kernel upgrade whose old version the repositories no longer list was refused, and that the removal would take out the running kernel, which zypper itself allows: the old version still installed now needs no source, and an undo that would remove the running kernel's package (owner of `/boot/vmlinuz-$(uname -r)`) is refused until the machine is rebooted into the other kernel; both unit-tested and re-run in the VM); (2) logrotate moves `/var/log/zypp/history` to `history-DATE.gz` (no new file until the next zypper run), after which undo and timeline saw nothing and blamed permissions; rotated `.gz/.xz/.bz2/.zst` files are now read (verified with `.gz`); (3) non-root `timeline` blamed a missing log instead of the 0750 /var/log/zypp; (4) `yapacc config` ignored `--config` in its "Loaded from" line; (5) `restart` ignored openSUSE's `/run/reboot-needed` flag (fixed; verified on Tumbleweed after a real kernel-default install: "Reboot: yes"); (6) a failed zypper simulation was silent, now warned. PackageKit (`pkcon install`, `pkcon update`, Tumbleweed): libzypp records `'/usr/libexec/packagekitd'` as the command, not an empty one, so `undo` and `timeline` show those runs normally (an undo of a `pkcon update` was refused because the old versions were no longer in the repositories); a history with no command record at all can only be produced by editing the file: such a run is grouped with the previous command by time, and a package removed and then installed in that group is now restored rather than removed (found by that edit; unit-tested). Untested: `restart` listing a service that holds a replaced library (reinstalling coreutils replaced nothing detectable), doas, missing rpm/zypper on PATH.

**openSUSE Leap 16.1 kernel multiversion across a reboot:** with `kernel-default` 8.1 installed beside the running `kernel-default-base` 9.1 (the newer `kernel-default` conflicts with the base, and yapacc cannot pin `name=version`, so zypper installed it), undo of that install was allowed; after a real one-shot grub boot into 8.1, `undo --dryrun` and `--yes` refused with the running-kernel message (exit 3) and `restart` went from `Reboot: yes` to `Reboot: no`. Undo of removing/installing the non-running base kernel worked for real. Known: `restart`'s "newer kernel" line is judged by file date, so an older kernel installed later is a false positive (the text says so). Tumbleweed (image `kernel-default-base` 7.3.3, repo 7.3.4, multiversion on): a real `yapacc upgrade --yes` (`zypper dist-upgrade`, 28 packages) installed `kernel-default-base` 7.3.4 beside the running 7.3.3; `restart` said `Reboot: yes` (newer kernel plus the reboot flag), then `Reboot: no` after a real reboot into 7.3.4. `undo --dryrun` of that `dist-upgrade` was refused with exit 3 both before and after the reboot by the new-dependency guard (`ucode-intel` and `patterns-glibc-hwcaps-x86_64_v3` were new installs), so the whole-upgrade undo plan, kernel included, is never shown (a known false-alarm gap). With 7.3.4 running, a hand-made `zypper rm` plus `zypper in kernel-default-base=7.3.4` pair planned `remove kernel-default-base=7.3.4`, and `--dryrun` and `--yes` were both refused by the running-kernel guard with nothing changed; a `zypper in -f` of the same version was planned as "nothing: only reinstalled". A kernel upgrade was also reverted for real (Tumbleweed VM, 2026-11-18): `zypper -n in kernel-default-base=7.3.4-1.1.36.16` (also pulled in the new package `ucode-intel`) beside the running 7.3.3; `undo --dryrun` was allowed while 7.3.3 ran (the new-dependency guard did not trigger on this `in` run; that reading of why was not tested), `sudo yapacc undo --yes` removed both, `rpm` showed only 7.3.3 and `/boot` only `vmlinuz-7.3.3-1-default`, and after a real reboot `uname -r` was 7.3.3-1-default, no failed units, `zypper verify` clean, `restart` said `Reboot: no`. `timeline` lists the undo as plain removes. Whether multiversion was on in that image was not checked.

**`undo` of a whole `zypper dup`/`up` (observed 2026-11-18, Tumbleweed VM):** packages the history records as installed with no earlier record (here `ucode-intel` and `patterns-glibc-hwcaps-x86_64_v3`, new dependencies) used to make the undo refuse outright, because libzypp writes new installs and upgrades the same way and rpm keeps no trace of the replaced version. Now `undo --dryrun` shows the plan and a note, and a real run asks at the terminal whether to remove them; `--yes`, `--script` and a pipe do not answer it (refusal, exit 3, package set unchanged: checked by `rpm -qa | sort | md5sum`), `n` aborts with nothing changed, `y` ran. On the VM a `yapacc upgrade --yes` (32 packages, kernel 7.3.3 -> 7.3.4, systemd, glibc) was undone: the package set was byte-identical to the one before the upgrade, and after a real reboot `uname -r` was 7.3.3-1-default, no failed units, `zypper verify` clean. That needed the openSUSE history snapshot added as a repo by hand: Tumbleweed had already dropped the old versions, and yapacc has no archive source for zypper, so on a stock Tumbleweed an undo of an upgrade more than a day or two old is refused ("in no cache, repository or archive yapacc can reach"). Not observed: the `rpm -e --test` dependents refusal on this path (the new packages had no dependents), a rotated history file with `dup`.


## `undo` on dnf and zypper: the guards

**`undo` on dnf/zypper**: tested as non-root with sudo (Debian, Fedora, Leap) and on Leap 16; the tty prompt, doas and multiversion are covered in their own entries. zypper undo cannot read the root-only history as non-root without `sudo yapacc undo`. The dnf dry run now refuses when `dnf repoquery --available` does not list the old version of an upgrade: unit-tested and run on real dnf 4 and dnf5 (see the dnf entry below). A zypper `up`/`dup`/`patch`/`inr` run whose install record has no earlier record is asked about at the terminal since 2026-11-18 (before: refused), since it may be an upgrade; an upgrade made by a plain `zypper install` of a leaf package with no earlier record is still planned as a removal (a note warns). A zypper mixed undo runs two zypper commands and says the next undo sees only the last. What was run for the zypper/dnf guards: hand-written history files and, later, histories produced by real `zypper dup` runs (see the zypper entries below); unit tests for `zypper --root /mnt up`-style global options; dnf availability is checked for the old versions of upgrades/downgrades and, since 2026-11-17, also for `Remove`/`Removed` entries (undoing a removal reinstalls that version); the parsing and matching is unit-tested and was run on Rocky 9 and Fedora 45 (see the dnf entry); the dnf availability check is one batched `dnf repoquery --available -q` and, if that fails (offline), only a warning is printed and the undo continues. Known gaps: a transaction with no recorded command (PackageKit, YaST, GNOME Software) is not refused, only noted, so an upgrade made that way with no earlier record is still planned as a removal.


## Arch Linux, Alpine and Void

**Booted Arch, Alpine and Void (2026-11-17, QEMU/KVM)**: Arch (official cloud image, default libalpm build) and Alpine (cloud image, musl build, doas; doas run for real, see the doas entry) via `vm.sh`; Void via `contrib/test/vm/void-image.sh` (no official VM image exists: built from the void-glibc container, direct kernel boot, `void-vm.sh`). On Void (musl build, xbps, runit) as `tester` with sudo and as root, all read commands ran; `install repo`/`remove repo`/`upgrade` with `--dryrun` and `--yes`; `[y/N]` answered n (exit 3, nothing changed) and y on a pty; a sudo password prompt (wrong passwords, then the right one) on a pty. Void doctor reports the truth: no undo, no timeline (xbps keeps no log), no news feed, curl absent. Not run on Void: doas, a real upgrade with pending packages (none were pending), no-network behaviour. Arch and Alpine results are those of the earlier passes of this lane; they were not re-run with the final binary. Other booted systems of this pass are reported by the other lanes.

**Arch (default libalpm build, snapper on a btrfs root):** libalpm is used for the plan analysis only, and needs root for the database lock (as a plain user the plan still prints, with "unable to lock database"). Real install/remove ran plain `pacman`; only `install alpm` commits through libalpm. `install aur` (figlet-fonts; a local two-package PKGBUILD) took one snapshot per build-set entry, after the build and before `pacman -U` (the split package: one snapshot for both files). `--repair-deps` takes one snapshot per repo dependency it installs. `plan apply --yes` took a snapshot; `--dryrun` previews only. Fixed after this run: a relative AUR build dir broke the bwrap sandbox, makepkg 7's "Missing dependencies" list was not parsed, and the pacman flows hard-coded `sudo` (now doas works). Through a restricted doas rule (pacman only): `auto` refused, `auto-best-effort` continued, and with `cmd snapper` permitted the snapshot was taken. Not observed: a `--deps` closure with a second AUR package (it failed upstream in CMake), `snapshot-hook.sh` via `--snapshot-tag`, timeshift in this lane.

`timeline` reads `/var/log/apk.log` for apk; xbps keeps no log, so it says so (observed on Void) and `undo` is unavailable there.


## FreeBSD (pkg)

**FreeBSD `pkg` backend** (observed 2026-11-18, QEMU/KVM, amd64, pkg 2.9.2): run on FreeBSD 14.5-RELEASE (UFS and ZFS root) and FreeBSD 15.1-RELEASE-p7 (UFS; yapacc built natively there with the packaged Rust, `--no-default-features`, about 2 minutes), also with doas (see the doas entry). Not run on real hardware. A 15.1 ZFS root was run too (see the round-5 entry). On 15.1: `platform`, `doctor`, `list`, `info`, `search`, `why`, `inspect owns/files/orphans`, `check`, `clean`, `install repo`/`remove repo`/`upgrade` with `--dryrun`, `--yes`, the `[y/N]` prompt over `ssh -tt`, as `tester` through sudo and doas (nopass; persist with the password typed over a pty) and as root. Timeline times: the stamps of `/var/log/messages` (`Nov 18 04:41:42 host pkg[1412]: rust-1.98.1 installed`) give the same Unix time as `date -u -r` (1794976902); with the zone set to Asia/Kolkata and syslogd restarted a real `pkg install figlet` was stamped `10:14:53` and yapacc showed `10:14` while `date` said 10:14:53 (the stamp is kept as written, so a zone change mid-history leaves older lines in the old zone). Found and fixed from this: a stamp from a zone already in the new year (`Jan  1 03:30` while UTC says Dec 31) was dated eleven months back. `restart` with `sysutils/checkrestart`: a process started before `pkg install -f libnghttp2` was listed; as `tester` checkrestart cannot see root's processes (verified: root's curl shown only to root), so the output now says so, and an empty answer is stated as such (before: `Processes: not checked. checkrestart said:` followed by nothing, also when it had listed processes). The reboot verdict was seen with the real `freebsd-version` (all three equal, "no"); the "reboot needed" branch was only run with a shim printing a newer `-k`, not with a really updated kernel. `pkg` not on PATH while `/var/db/pkg/local.sqlite` exists (PATH without `/usr/sbin`): found broken: the Portage check treated `/var/db/pkg` as its own database, so `list`/`search`/`check`/`info`/`inspect` said "this system uses portage but `emerge` is not on PATH" and `install`/`remove`/`upgrade` fell through to pacman plans (`sudo pacman -S curl`). Fixed: Portage ignores a `/var/db/pkg` that holds `local.sqlite`; `list`, `search`, `check`, `info`, `inspect`, `briefing`, `install`, `remove`, `upgrade`, `undo`, `why`, `clean`, `timeline` and `doctor` now say "this system uses pkg but `pkg` is not on PATH (PATH=...)" (doctor: `native-manager` fails). Real offline, two ways: pf `block drop all` except ssh (fetch: "Transient resolver failure", ping: Permission denied) and a resolver that never answers (`nameserver 192.0.2.1`: `fetch` hangs). `check`, `search`, `list`, `briefing`, `doctor`, `clean`, `restart`, `why`, `timeline`, `upgrade --dryrun` and `install --dryrun` answered in under a second in both (from the catalogue on disk); `names` failed in 15 s with a message naming repology.org. `install --yes`/`upgrade --yes` call `pkg`, which exits 3 with "Error updating repositories!" under pf (a hint now says the network is the likely cause and nothing was changed); with the silent resolver `pkg install -y` printed "Updating FreeBSD-ports repository catalogue..." and then sat for 16+ minutes (pkg's own retries, yapacc waits for it; killed by hand), and after `timeout 60` killed yapacc the `pkg` child kept running. Also seen: with the silent resolver sshd logins into the VM took 30 s. OpenBSD and NetBSD are unsupported.

**FreeBSD 15.1-RELEASE-p7 on a ZFS root (pkgbase):** `platform` (zfs, bios), `doctor`, `search`, `info`, dry-runs, real `install repo`/`remove repo --yes`, the `[y/N]` prompt over a pty, `timeline`, `undo` (reports unavailable) and `restart` (all three `freebsd-version` outputs equal). `[safety] snapshot` took `zfs snapshot` of `zroot/ROOT/default` before real changes; under a sudoers rule without zfs `auto` refused and `auto-best-effort` continued. yapacc does not use `bectl`. Fixed after this run: a failed `pkg delete` simulation (package not installed) now stops before the snapshot, and `timeline --since-boot` reads `sysctl kern.boottime`; both re-run on a 15.1 ZFS VM afterwards (no snapshot created for the missing package, a normal install/remove still snapshots, the since-boot header matched `sysctl kern.boottime`). The since-boot header says "/proc/stat btime or sysctl kern.boottime" even where only one exists. Not achieved: a really changed `freebsd-version -k` (the repo had no newer kernel).


## Snapshots

**`[safety] snapshot = "auto"|"auto-best-effort"`** (snapshot before a change through yapacc): the decision function, config parsing and the "--dryrun runs no snapshot tool" path are unit-tested. Run for real: against zfs on a FreeBSD 14.5 ZFS VM, and against snapper on openSUSE Leap 16.1 and Tumbleweed VMs (separate btrfs volume because the cloud images have an XFS root; re-run 2026-11-18 on Tumbleweed: `install repo` and `upgrade` (`zypper dist-upgrade`) each took a pre snapshot, also when the upgrade found nothing to do, and `snapshot list` needs root): `auto` took a real snapper pre snapshot around a real `zypper install`, `auto-best-effort` continued and plain `auto` refused when snapper failed (SELinux enforcing denies `snapper create-config` until `setenforce 0`; a distro policy quirk). timeshift (Debian 13, timeshift 24.06.6, rsync mode on an ext4 second disk; see the parser entry): `auto` took a real timeshift snapshot (named in the preview, run after consent, tagged `O`, comment `yapacc: before install cowsay`) around a real `apt-get install` and a `remove`; with the snapshot device missing, `auto` refused and `auto-best-effort` continued. timeshift in btrfs mode (Arch cloud image, timeshift 26.11.1, btrfs `/`): `[safety] snapshot = "auto"` took a real timeshift snapshot (`O`, comment `yapacc: before install cowsay`) around a real `pacman -S` and `pacman -Rns`. Found there: on Arch (pacman) the setting was silently ignored, because the pacman flows (`install repo`, `remove repo`, `upgrade`, `plan apply`) never called the snapshot step (only the portable managers did); now `install repo`, `remove repo` and `upgrade` preview it, take it after consent and fail closed on a bad value, and `plan apply` previews it (not under `--dryrun`) and takes it after consent (preview added without a VM run); the AUR build flow goes through the same install step, so with `auto` it takes one snapshot per AUR package in the build set after the build and before `pacman -U` (seen on an Arch VM, see the round-5 entry); `safety apply --op arch-install` does take one (it goes through the same install step); its separate "Snapshot step" is the `snapshot-hook.sh` script, used only with `--snapshot-tag`. Verified in a VM only for `install repo` and `remove repo`. Through a restricted doas rule (Debian 13, `permit nopass tester cmd apt-get`): `auto` refused with the snapshot command named (`doas -n timeshift ...`, before: `timeshift exited with status 1`, though status 1 came from doas), `auto-best-effort` continued, and with `cmd timeshift` also permitted `auto` took the snapshot and then ran the removal. `require_snapshot_before_upgrade` is read only by `safety apply` (for `arch-upgrade` and `arch-update`); the gated `upgrade` command uses `snapshot` instead.

**snapper, btrfs, zfs** parsers behind `snapshot`: checked against the real tools (snapper and `btrfs subvolume list -s` on the Leap and Tumbleweed VMs, on a separate btrfs volume, not a btrfs `/`; the `snapshot create` plain-btrfs fallback was not exercised; zfs `snapshot list/create` on the FreeBSD ZFS VM). **timeshift** (real `timeshift --list`/`--create` 24.06.6 on Debian 13, rsync mode, snapshot device `/dev/vdb` ext4; run as root, `snapshot list` and `create` as `tester` through sudo): the list format the parser expects (`0    >  2026-11-18_04-43-19  D     first one`, tag letter column, comment) is what the tool prints, `--json` and `snapshot create --dryrun` ran. Bug found and fixed: with the snapshot device missing or stale in `timeshift.json`, `timeshift --create` prompted "Enter device name or number" and aborted on invalid input; it now gets `--scripted` and fails at once with "Snapshot device not available". With no config at all, `--create --scripted` silently picks the root device (`/dev/vda1`) as the snapshot device and writes the config, so the "if timeshift was never set up" warning in the preview is real. timeshift btrfs mode (Arch cloud image, btrfs `/`, timeshift 26.11.1; a root with top-level subvolume `/` is refused as "unsupported subvolume layout" until `@` and `@home` exist AND `/etc/fstab` has a `/` entry with `subvol=@`; made real by snapshotting `/` to `@`, creating `@home`, `btrfs subvolume set-default`, fstab lines, reboot; a second btrfs disk with `@`/`@home` is ignored: "--snapshot-device should not be specified for creating snapshots in BTRFS mode"): `timeshift --list` prints the same table as in rsync mode, with tags `D`, `H`, `W`, `M`, `B`, `O` and a two-tag `D W` seen, and `snapshot list`/`create` (also `--json`) read it. `timeshift --create` without `--btrfs` follows the mode found; `timeshift --list --json` does not exist in 26.11.1. Bug found and fixed: a comment that begins with a tag letter (`M to the max`, `H`) lost that word, because tags were told from comment by shape; the comment now starts at the `Description` column of the header. Not run: `timeshift --restore`, `@home` included in snapshots. A stale device also makes `timeshift --list` itself exit 1 (with a GLib assertion), so `snapshot list` reports a provider-list failure then.

**Not a yapacc feature:** yapacc only creates and lists timeshift snapshots; it never calls `timeshift --restore`, so there is nothing of ours to test there.


## doas

**doas, for real (observed 2026-11-18, QEMU/KVM; Alpine 3.24 opendoas, Debian 13 opendoas 6.8.2 with sudo purged, FreeBSD 14.5 UFS and ZFS with sudo deleted; `tester` and root)**: `permit persist tester` (password) and `permit nopass tester` (script) both configured. `install repo`/`remove repo`/`upgrade`/`undo`/`undo --dryrun` with `--yes` and with the `[y/N]` prompt, the doas password prompt over `ssh -tt` (right password; a wrong one makes doas exit 1 and yapacc reports the failed step), `--script` (runs `doas -n`; under persist it fails at once with "Authentication required", since opendoas `-n` ignores the persist timestamp; under nopass everything runs), hostile package names (rejected before doas), `restart`/`briefing`/`clean`/`configs` as tester and via `doas yapacc`; `clean` hints say `doas`. A real Alpine `upgrade` (kernel included) ran through doas. ZFS root: `snapshot create` and the `[safety] snapshot = "auto"` snapshot-before-change ran through doas (password), and `--script` with a password-needing doas fails closed before the change. Notes: opendoas `persist` is keyed to the parent process, so every `yapacc` run asks once (steps inside one run share it); `sudo` is preferred when both exist. Bugs found and fixed: (1) on FreeBSD `/proc` is not mounted, so `is_root()` always said "not root": root running `yapacc` put `doas` in front of every command (now falls back to `id -u`); (2) FreeBSD doas with no controlling terminal and a password needed waited forever (`yapacc install repo --yes x </dev/null` over plain ssh): the tool now gets `-n` whenever `--script` or `/dev/tty` cannot be opened, and fails with "Authorization required"; (3) a failed `doas -n` step was reported as an ordinary command failure ("`yapacc timeline` shows what was changed"): a hint now explains `-n` and `permit nopass`, shown only when a harmless `doas -n true` is refused too. Restricted rules (Debian 13, sudo moved away, rules `permit nopass tester cmd apt-get` / `cmd dpkg`, `args install -y -- figlet`): the steps `apt-get install/remove`, `upgrade` and `undo` (`apt-get install --allow-downgrades`) ran when `apt-get` was permitted and `doas -n true` itself was refused ("Operation not permitted"); with `apt-get` not permitted or the args not matching, the step failed and the hint blamed a missing password (`permit nopass`), which was wrong: the hint now tells "not permitted" from "Authentication required" by what `doas -n true` printed, says a restricted rule is the cause, and for any other probe failure (a `doas.conf` syntax error) quotes the tool's own line. `doas.conf` env: doas resets the environment, so a caller's `DEBIAN_FRONTEND=noninteractive` never reaches `apt-get` (a debconf package, `keyboard-configuration`, fell back to Teletype); `keepenv` and `setenv { DEBIAN_FRONTEND=noninteractive } tester` (options before the user) restore it; yapacc itself sets no `DEBIAN_FRONTEND`. Not run: doas on Void, Ubuntu, any rpm distro; `snapshot` on btrfs/snapper through doas; Debian `undo` of an upgrade with real pending packages through doas (none were pending); on FreeBSD `restart` uses `freebsd-version` and checkrestart, not `/proc`. Also run on FreeBSD 15.1-RELEASE (UFS, sudo moved away): doas `permit nopass` (`--yes`, `--script`, root via `doas yapacc`) and `permit persist` (`--script` fails at once with "Authorization required" and the hint; the password prompt over `ssh -tt` works for `--noconfirm install` and the `[y/N]` remove).


## Packaging

**Packaging** (`contrib/packaging/`): built from the source tarball and installed in clean containers on 2026-11-16 for Arch, Debian testing, Fedora 45, Alpine and Void; `yapacc --version` and `doctor` ran on each. Alpine and Void ship the core only. `contrib/test/pkg-lifecycle.sh` installed, upgraded (to a bumped package version built from a second tarball) and removed the Debian testing and Fedora 45 packages in clean containers, the two builds differing by a marker file each (added to a copy of the recipes) so that the stale-file check can fail: old marker gone, new one present, no owned file or yapacc directory left, a non-root user's `~/.config/yapacc` unchanged (recipe test steps skipped; the bump changes the package version, not `Cargo.toml`). The Alpine path of the script (including its marker) was not run in this pass; an earlier Alpine run had no differing files, so its stale-file check proved nothing. `fill-release.sh` fills version and checksums into a copy of the recipes (run locally only). The recipes' source URL now points at the release asset the workflow publishes, a design that has not run: the workflow's tarball/version checks have never executed on GitHub. Source URLs are still placeholders, only x86_64, Arch and Void lifecycle untried, and no real repository or release workflow has run.


## Continuous integration on GitHub

**First hosted runs (2026-11-19, the day the repository became public).**
`ci.yml` ran on GitHub for the first time, on `ubuntu-latest` (Ubuntu
26.04). The first run passed 12 of 14 jobs, the second 13, the third all
14. What failed, none of which `cargo xtask ci` could have shown:
a test compared gpgv's output with the signed text byte for byte, and the
gpgv of that image ends its output with one more empty line, which the
parser skips (the test now ignores trailing newlines); the Void row of the
`families` job could not start, because the runner's own actions need
Node, which needs libstdc++, which the Void image lacks (the job installs
it first; yapacc itself was never the problem there); three smoke tests
asserted success before looking at the machine, and on a machine without
pacman `get NAME --explain` for a name found nowhere is "not found" (exit
4) and the MCP install tool answers "PLAN FAILED". Those three had never
run without pacman: the `portable` job of `cargo xtask ci` runs on the
developer's machine, an Arch system. After the fix the whole test suite
(680 tests) was run in `ubuntu:26.04` and `fedora:latest` containers,
which have no pacman, and passed in both.

## Continuous integration, run locally

**CI commands run locally (2026-11-18; not a substitute for the runner):** the `portable` job's fmt, clippy and test commands (`--no-default-features --features tui`) pass (275 lib, 85 octo-head, 36 CLI tests), and the `musl` job's checks hold on the local static build (static-pie, no dynamic dependencies, `--version` runs, 5,199,920 bytes against the 6 MiB budget). The `arch` job was run in a podman `archlinux:base-devel` container (clippy, full tests, read-only command surface) and the `families` matrix against all seven images with the static binary. That found two defects CI would have hit, both fixed: `inspect owns` of an unowned symlink (Alpine's `/usr/bin/env -> /bin/busybox`) now follows the link like `apk info -W` does, and `container_plan_reports_engine` assumed a container engine exists (now it also accepts the clear "no supported engine" refusal). After the fixes the Arch container run passes and all seven `families` rows pass with the rebuilt static binary. The GitHub runner has not run these jobs.

**`cargo xtask ci` (added 2026-11-18):** runs the four jobs of `.github/workflows/ci.yml` locally (`portable`, `musl`, `arch` and `families` in podman/docker; name jobs to run fewer, `--no-containers` skips the container ones). A full run passed: `portable`, `musl`, `arch` (after tracking the new crate; the job copies tracked files only, like a checkout) and all seven `families` rows. It reproduces the workflow's commands; it is not the GitHub runner.
