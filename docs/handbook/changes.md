# install, remove, upgrade

yapacc can make the common changes through your package manager. It adds
three things to running the native command yourself: the plan and the
native simulation are always shown first, the [gate](gate.md) is the same
on every system, and a [snapshot](snapshots.md) can be taken before the
change.

You never have to use these commands. Changes you make directly with
`apt`, `pacman` or `dnf` are just as visible to `timeline` and `undo`.

## The commands

| Command | What it does |
|---|---|
| `yapacc install repo PKG…` | install packages from the distribution's repositories |
| `yapacc remove repo PKG…` | remove packages |
| `yapacc remove orphans` | remove dependencies that nothing needs any more |
| `yapacc upgrade` | full system upgrade |
| `yapacc refresh` | refresh the repository indexes only |
| `yapacc get NAME` | choose a source for NAME, then install it |

All of them take `--dryrun` and `--yes`, and all of them stop at the gate
without one of the two.

```console
$ yapacc install repo figlet --dryrun
Plan: install figlet (through apt)

apt's own simulation (apt-get -s -y install -- figlet):
…
The following NEW packages will be installed:
  figlet
0 upgraded, 1 newly installed, 0 to remove and 24 not upgraded.
Inst figlet (2.2.5-3+b2 Debian:13.7/stable [amd64])
Conf figlet (2.2.5-3+b2 Debian:13.7/stable [amd64])

Will run:
  sudo apt-get install -y -- figlet

DRY-RUN: nothing was changed. Run again without --dryrun to apply.
```

A full upgrade reads the same way. This is the machine of the
[quick start](quick-start.md), two months behind:

```console
$ yapacc upgrade
Plan: upgrade the system (through apt)

apt's own simulation (apt-get -s -y upgrade --with-new-pkgs):
…
The following NEW packages will be installed:
  linux-image-6.12.117+deb13-amd64
The following packages will be upgraded:
  apt apt-utils bind9-host bind9-libs ca-certificates curl libapt-pkg7.0 libbrotli1 libcurl4t64 libexpat1
  liblzma5 libpcre2-8-0 libpng16-16t64 libssl3t64 linux-image-amd64 openssl openssl-provider-legacy tzdata
  vim vim-common vim-runtime vim-tiny xz-utils zlib1g
24 upgraded, 1 newly installed, 0 to remove and 0 not upgraded.
Inst liblzma5 [5.8.1-1+deb13u1] (5.8.1-1+deb13u2 Debian-Security:13/stable-security [amd64])
Conf liblzma5 (5.8.1-1+deb13u2 Debian-Security:13/stable-security [amd64])
…

Will run:
  sudo apt-get update
  sudo apt-get upgrade --with-new-pkgs -y
note: the preview uses the index from the last `apt-get update`; the upgrade refreshes first
Proceed [y/N]? y
…

Done. `yapacc timeline` shows the change; `yapacc undo` can take it back.
```

## What is run, exactly

yapacc builds no shell command. Package names are validated, then passed as
separate arguments after `--`, so a name can never be read as an option.
This is the command behind each verb:

| Manager | install | remove | upgrade | refresh |
|---|---|---|---|---|
| pacman | `pacman -S` | `pacman -Rs` | `pacman -Syu` | `pacman -Sy` |
| apt | `apt-get install -y` | `apt-get remove -y` | `apt-get update`, then `apt-get upgrade --with-new-pkgs -y` | `apt-get update` |
| dnf | `dnf install -y` | `dnf remove -y` | `dnf upgrade --refresh -y` | `dnf makecache --refresh` |
| zypper | `zypper install` | `zypper remove` | `zypper dist-upgrade` on rolling releases, `zypper update` otherwise | `zypper refresh` |
| apk | `apk add` | `apk del` | `apk update`, then `apk upgrade` | `apk update` |
| xbps | `xbps-install -y` | `xbps-remove -y` | `xbps-install -Syu` | `xbps-install -S` |
| pkg | `pkg install -y` | `pkg delete -y` | `pkg upgrade -y` | `pkg update` |

The simulation shown before the gate is the same manager's dry run of the
same request (`apt-get -s`, `dnf --assumeno`, `zypper --dry-run`,
`apk --simulate`, `xbps-install -n`, `pkg -n`, `pacman --print`). If the
simulation itself fails, the change is refused (exit code 3) before you are
asked anything: an unknown name or a broken dependency stops there.

Two simulations cannot be judged by their status: dnf's `--assumeno`
always ends with an error status, and so does `pkg install -n` whenever
there is something to install. yapacc reads what they print instead. When
dnf says it could not resolve the request (an unknown name, a conflict, a
protected package), an install or a removal is refused in the same way,
with exit code 3.

An upgrade is the one change a failed simulation does not refuse, on dnf
and on pacman: the simulation runs without the refresh the real command
begins with, so its failure says little. A note says that the simulation
failed, and you are still asked.

```console
$ yapacc install repo no-such-package-xyz --dryrun
Plan: install no-such-package-xyz (through apt)

apt's own simulation (apt-get -s -y install -- no-such-package-xyz):
…
E: Unable to locate package no-such-package-xyz
error: apt-get -s -y install -- no-such-package-xyz failed (status 100): the change cannot be made as asked; nothing was changed
```

A removal of something that is not installed never gets that far. It is
"not found" (exit code 4) on every package manager. A name that an
installed package provides, and a pacman group with installed members, are
not refused.

```console
$ yapacc remove repo cowsay
Plan: remove cowsay (through apt)
error: cannot remove cowsay: not installed. Nothing was changed.
```

On FreeBSD yapacc asks the catalogue on disk first and refuses a name that
is not in it (exit code 4), with the hint to run `pkg update`. A name pkg
itself answers "No packages available to install matching" for is refused
with the same code. With no catalogue on disk, `pkg install -n` prints
nothing; a note under the simulation heading then says that there is no
repository metadata and names `pkg update`.

On Arch, before the repository databases were ever fetched, `install repo`
is refused with exit code 6: nothing can be planned from no database. The
message names `pacman -Syu` and `yapacc upgrade`, which runs it behind the
gate.

A removal on pacman is `-Rs`: the package and the dependencies nothing
else needs. Configuration files you modified are kept as `.pacsave`.
pacman is always run with `--noconfirm`: you answered at the gate, and
pacman does not ask a second time.

## What the last line means

| It says | What happened |
|---|---|
| `Done.` | the packages changed as planned |
| `Nothing was changed: apt had nothing to do …` | the command ran without error and the installed packages are the same as before |
| `No package was changed: … refreshed its repository indexes and had nothing to upgrade` | an `upgrade` with nothing pending |
| `Nothing was changed: the installed packages are the same as before.` | the command failed before touching a package |
| ``Packages were changed before it failed: `yapacc timeline` shows what was changed.`` | the command failed part of the way |

yapacc works this out by comparing the installed packages before and
after, so the line is true even when the native manager's exit status is
not informative.

> [!NOTE]
> On apt, `upgrade` uses `--with-new-pkgs`, so kernel upgrades that need a
> new package are included. It does not remove packages; use your package
> manager's `full-upgrade` when a release change requires that.

## Before a full upgrade

Run [`yapacc briefing`](briefing.md) first. It tells you whether the
upgrade touches the kernel, the C library, the package manager or ssh, and
shows the distribution's announcements.

After it, [`yapacc restart`](housekeeping.md#restart) says what still runs
old code and whether a reboot is due, and
[`yapacc configs`](housekeeping.md#configs) lists configuration files the
upgrade left for you to merge. If the upgrade went wrong,
[`yapacc undo`](undo.md) takes it back.

## get: choosing a source

`yapacc get NAME` looks for NAME in the sources this machine has and
installs from the first that has it. The distribution's own package comes
first. `--explain` shows the decision and stops.

```console
$ yapacc get ripgrep --explain
== Resolution: ripgrep ==
 → repo     found    ripgrep 14.1.1-1+b4                      exact name in the apt repositories
   flatpak  missing  —                                        flatpak is not installed on this system
chosen: repo (the distribution's own package wins when it exists)
```

Off Arch, `get` knows the distribution's repositories and flatpak; it shows
a flatpak match and does not install it for you. On Arch it also knows the
AUR and local recipes, and takes the first source in the order
`repo, aur, flatpak, recipe` that has the name; `[install] order` in the
[configuration](configuration.md) changes that order, and
`--from repo|aur|flatpak|recipe` forces one source for one run.

## More than the repositories

The commands above cover the native package manager, which is what most
people need. yapacc also has specialist commands for other sources:
flatpak, AppImages, the AUR on Arch with a review step and a sandboxed
build, and isolated installs of packages made for another distribution.
`yapacc --help` does not list them; [every command](commands.md) does.
