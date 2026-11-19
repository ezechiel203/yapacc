# yapacc timeline

`yapacc timeline` lists package installs, upgrades and removals, newest
first, from the logs your package manager already keeps. It is read-only and
it keeps no log of its own.

```console
$ yapacc timeline --limit 8
2026-11-19 13:11 (local)  apt     install    jq  1.7.1-6+deb13u4
2026-11-19 13:11 (local)  apt     install    libjq1  1.7.1-6+deb13u4
2026-11-19 13:11 (local)  apt     install    libonig5  6.9.9-1+b1
2026-11-19 13:10 (local)  apt     downgrade  libpng16-16t64  1.6.48-1+deb13u6 -> 1.6.48-1+deb13u5
2026-11-19 13:10 (local)  apt     downgrade  libcurl4t64  8.14.1-2+deb13u7 -> 8.14.1-2+deb13u5
2026-11-19 13:10 (local)  apt     downgrade  curl  8.14.1-2+deb13u7 -> 8.14.1-2+deb13u5
2026-11-19 13:10 (local)  apt     downgrade  libbrotli1  1.1.0-2+deb13u1 -> 1.1.0-2+b7
2026-11-19 13:10 (local)  apt     downgrade  xz-utils  5.8.1-1+deb13u2 -> 5.8.1-1+deb13u1
```

Each line is the time, the log it came from, what happened (`install`,
`upgrade`, `downgrade`, `reinstall`, `remove`), the package and the
version: old and new for an upgrade or a downgrade. dpkg's log calls every
version change an upgrade; yapacc compares the two versions as dpkg does
and says which it was. The downgrades above are an
[`undo`](undo.md): it is a transaction like any other.

## What changed since it last worked?

```console
$ yapacc timeline --since-boot
Changes since the start of the previous boot: 2026-11-19 13:07 UTC (from journalctl --list-boots)
── boot: 2026-11-19 13:12 UTC ──
2026-11-19 13:12 (local)  apt     upgrade    linux-image-amd64  6.12.107-1 -> 6.12.117-1
2026-11-19 13:12 (local)  apt     upgrade    libpng16-16t64  1.6.48-1+deb13u5 -> 1.6.48-1+deb13u6
2026-11-19 13:12 (local)  apt     upgrade    libcurl4t64  8.14.1-2+deb13u5 -> 8.14.1-2+deb13u7
…
2026-11-19 13:12 (local)  apt     upgrade    xz-utils  5.8.1-1+deb13u1 -> 5.8.1-1+deb13u2
note: apt and zypper log local time without a zone and yapacc reads it as UTC: their changes within the zone offset of the boot instant can be on the wrong side of it
```

`--since-boot` starts at the boot before the current one, because the
question is usually "it worked yesterday, then I rebooted". The first line
says which moment was used and where it came from, and a `── boot ──` line
marks where the machine restarted: what is listed below it was changed
before the reboot. When only the current boot is known, a note says so and
the list starts there instead.

The boot time comes from `journalctl --list-boots` where there is a
journal, otherwise from the kernel (`/proc/stat` on Linux,
`sysctl kern.boottime` on FreeBSD).

## Options

| Option | Effect |
|---|---|
| `--since YYYY-MM-DD` | only changes on or after that day (UTC) |
| `--since-boot` | only changes since the previous boot |
| `--limit N` | at most N changes; the default is 50 |
| `--html` | write one self-contained HTML page to standard output |

`--html` is meant for bug reports: `yapacc timeline --html > timeline.html`
gives a single file with no external reference. It cannot be combined with
`--json`.

## Where the entries come from

| Package manager | Log read |
|---|---|
| pacman | `/var/log/pacman.log` |
| apt | `/var/log/dpkg.log`, including rotated and compressed files |
| dnf | dnf's own history database (`dnf history`) |
| zypper | `/var/log/zypp/history`, including rotated and compressed files |
| apk | `/var/log/apk.log`, when it exists |
| pkg (FreeBSD) | `/var/log/messages` |
| xbps | none: xbps keeps no log, and `timeline` says so |

Some of these logs are readable by root only (openSUSE's
`/var/log/zypp` is one). `timeline` then says which file exists and may not
be read, tells you to run `sudo yapacc timeline`, and exits with code 6.
`yapacc platform` marks such a log `(exists; not readable by this user)`.

Times are printed as the log wrote them. A log that records local time
without a zone (apt, zypper) is labelled `(local)`. With `--since-boot`
such a time is compared with the boot instant as if it were UTC, so a
change made within your zone offset of the reboot can land on the wrong
side of it; a note says so whenever it applies.

A log can be silent about a run. apk run with `--quiet` records that it ran
and not what it did, and `timeline` then says how many runs it could not
describe ("3 apk run(s) were logged without package details") instead of
leaving them out without a word.

## In a script

```console
$ yapacc --json timeline --limit 1
[
  {
    "ts": 1795093887,
    "source": "apt",
    "action": "install",
    "name": "jq",
    "old_version": "",
    "new_version": "1.7.1-6+deb13u4"
  }
]
```

`ts` is seconds since the Unix epoch. `action` is `install`, `upgrade`,
`downgrade`, `reinstall` or `remove`; a reinstall has an empty
`old_version`.
