# yapacc undo

`yapacc undo` takes the last package transaction back. It reads the last
transaction from the native log, works out its reverse, shows you the plan
and the exact command, and asks before doing anything. On apt the plan
includes apt's own simulation of the reverse; on dnf it is dnf's own
description of the transaction.

It works on pacman, apt, dnf and zypper. On apk, xbps and FreeBSD's pkg it
says that undo is not available there.

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

That is the plan for taking back an upgrade of two packages on Debian 13.
A whole system upgrade is undone the same way. This one was a
`yapacc upgrade` of 24 packages and a new kernel, taken back for real on a
virtual machine:

```console
$ yapacc undo
Last transaction: 2026-11-19 13:08 (local)  (apt-get upgrade --with-new-pkgs -y)
  installed   linux-image-6.12.117+deb13-amd64:amd64 6.12.117-1
  upgraded    bind9-host:amd64 1:9.20.26-1~deb13u1 -> 1:9.20.30-1~deb13u1
  upgraded    apt:amd64 3.0.3 -> 3.0.4
  upgraded    tzdata:amd64 2026c-0+deb13u1 -> 2026f-0+deb13u1
  …

To undo it:
  remove  linux-image-6.12.117+deb13-amd64:amd64
  restore openssl:amd64 3.5.7-1~deb13u2  (from the configured repositories)
  restore libpng16-16t64:amd64 1.6.48-1+deb13u5  (from the configured repositories)
  restore zlib1g:amd64 1:1.3.dfsg+really1.3.1-1+b1  (from the configured repositories)
  …

…
Will run:
  sudo apt-get install -y --allow-downgrades -- openssl:amd64=3.5.7-1~deb13u2 …
note: Undo restores package versions only. It does not restore configuration files or data that package scripts changed; a filesystem snapshot does.
Undo this transaction [y/N]? y
…
Undone. `yapacc timeline` shows it as a new transaction.
Verified: 25 of 25 package(s) are at the planned state.
```

The last line is a check. When the native command has returned, yapacc
reads the installed versions again and compares them with the plan.

## What "undo" means

| The transaction did this | Undo does this |
|---|---|
| installed a package | removes it |
| removed a package | installs that version again |
| upgraded a package | restores the previous version |
| downgraded a package | restores the newer version |

Undo acts on one transaction: the most recent one in the native log. An
undo is itself a transaction, and [`timeline`](timeline.md) shows it like
any other.

> [!IMPORTANT]
> Undo restores **package versions**. It does not restore configuration
> files, databases or anything a package script changed. Only a filesystem
> snapshot does that; see [snapshots](snapshots.md). The output repeats
> this every time.

## Where old versions come from

To restore a previous version, yapacc has to find that exact version. It
looks in this order and tells you, per package, where it found it:

| Package manager | Sources, in order | Verification |
|---|---|---|
| pacman | the pacman cache, then the Arch Linux Archive | the archive's detached signature, verified with `pacman-key` before the package is installed |
| apt on Debian | the configured repositories, the apt cache, then snapshot.debian.org | repositories: apt's own; snapshot.debian.org: checked against the recorded hash, not signature-verified |
| apt on Ubuntu | the configured repositories, the apt cache, then Launchpad | repositories: apt's own; Launchpad downloads are not verified, and the output says so |
| dnf | the enabled repositories, through dnf's own `history undo` | dnf's own |
| zypper | the configured repositories only | zypper's own |

**All or nothing.** If one old version cannot be found anywhere, the whole
undo is refused (exit code 3) and nothing is changed. yapacc does not apply
half of an undo.

On dnf, the old versions are checked for availability before the gate, so a
refusal comes from the dry run and not from the middle of the real one. On
zypper there is no archive to fall back on: on a rolling distribution such
as Tumbleweed, the repositories drop old versions quickly, and an undo of an
upgrade more than a day or two old is usually refused for that reason.

A dry run downloads nothing: it asks the archive whether the file exists
and reports where each version would come from, with its size when the
archive gives one. The real run downloads after you have agreed, into a
private directory under `~/.cache/yapacc/undo` that is deleted when the
undo ends. On apt, when files have to be downloaded, you are asked twice:
once before the download, and once more with apt's simulation of the
restore in front of you. `--yes` answers both.

An archive that does not answer is not an archive that lacks the version.
A lookup waits 20 seconds for the connection and 60 in all, and is tried
twice. If there is still no answer the undo is refused (exit code 3) with
"did not answer … so it is not known whether it has that version", and you
try again later. Only an archive that answered "no" produces "is in no
local cache and no configured repository, and the archive of this
distribution … does not have it".

On zypper the same care applies to the repositories: without repository
metadata on disk, yapacc cannot know whether a repository still has the old
version, says so, and refuses. `zypper refresh` first.

## After the command ran

| The last lines say | What happened |
|---|---|
| `Verified: N of N package(s) are at the planned state.` | the undo is complete |
| `The package versions were restored (N of N verified), but apt reported an error …` | every version is back, and the native manager still ended with an error, often a package script that failed. Read its output above; exit code 1 |
| `K of N package(s) are not at the planned state: …` | the native command failed part of the way, or reported success and left a difference. The packages are named; `yapacc timeline` shows the current state; exit code 1 |

## Guards

Undo refuses rather than doing something it cannot stand behind.

- **The running kernel.** An undo that would remove the kernel the machine
  is running on is refused until you have rebooted into another kernel.
- **Packages that something else needs.** If removing a package would break
  a package installed since, the native manager's dependency check refuses
  it and yapacc stops.
- **Failed verification.** A file from the Arch Linux Archive without a
  signature that verifies, or a file from snapshot.debian.org whose hash
  does not match, is deleted and the undo stops with exit code 5.
- **More than was planned.** On apt, if the simulation shows that the undo
  would also remove a package that was not part of the transaction, the
  undo is refused.
- **Unknown history on zypper.** libzypp records an upgrade as an install
  of the new version, so for a package with no earlier record yapacc cannot
  tell "new dependency" from "upgraded". When undoing a whole `zypper up` or
  `dup`, it names those packages and asks at the terminal whether to remove
  them. `--yes`, `--script` and a pipe do not answer that question: without
  a terminal the undo is refused.

## Privileges

Reading the last transaction may itself need root: openSUSE keeps
`/var/log/zypp/history` readable by root only, so there you run
`sudo yapacc undo`. Elsewhere, run it as yourself; yapacc calls `sudo` or
`doas` for the one command that changes the system.

## Options

| Option | Effect |
|---|---|
| `--dryrun` | show the last transaction and the plan; download and apply nothing |
| `--yes` | apply without the `[y/N]` question |

Without either, undo shows the plan and asks. See [the gate](gate.md).

With `[safety] snapshot` set, a [snapshot](snapshots.md) is taken after you
consent and before the first package is touched, as for any other change.

On dnf the plan names the transaction by its ID, and that ID is what is
undone, even if another transaction has been committed since you read the
plan. On Debian and Ubuntu, multiarch packages are shown and handled with
their architecture (`libfoo:i386`).

## Limits

- One transaction, the last one. There is no "undo the last three" and no
  undo of a transaction further back.
- A restored package is upgraded again by the next full upgrade unless you
  hold it yourself (`IgnorePkg` in pacman.conf, `apt-mark hold`, a dnf or
  zypper lock).
- Undoing an install removes the package and does not purge it. On apt its
  configuration files stay, as after any `apt-get remove` (state `rc` in
  `dpkg -l`).
- apt does not remove a package it counts as essential under `-y`, and
  yapacc does not override that, with one exception: packages of a foreign
  architecture that the transaction itself installed. An undo that would
  need more is refused, and the message says which package.
- No undo on apk, xbps or pkg.
- It has been run end to end on booted virtual machines of Arch, Debian 13,
  Ubuntu 24.04, Fedora 45, Rocky Linux 9.9, openSUSE Leap 16.1 and
  Tumbleweed, including kernel upgrades followed by a real reboot. It has
  not been run on real hardware.
  [What works where](support.md) has the detail.
