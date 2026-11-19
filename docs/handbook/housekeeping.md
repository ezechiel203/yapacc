# clean, restart, configs

Three reports for the day after an upgrade. All three are read-only: each
prints what it found and the command you would run yourself, and none of
them runs it for you.

## clean

`yapacc clean` measures what can be reclaimed.

```console
$ yapacc clean
 124.9 MiB  [safe] downloaded packages kept by apt (/var/cache/apt/archives) — 28 item(s)
            reclaim: sudo apt-get clean
            size is a lower bound (some directories are readable by root only); safe, with one cost: `yapacc undo` looks here first for the previous version; an emptied cache means undo has to download it

Measured total: 124.9 MiB. Nothing was changed: `clean` only reports.
```

It looks at:

- packages installed as dependencies that nothing needs any more (orphans);
- the package manager's download cache;
- yapacc's own cache;
- unused runtimes of other sources, such as flatpak, when they are present.

Each item is labelled `[safe]` or `[review]`. `safe` means removing it loses
nothing that cannot be fetched again, and the line says what the cost is.
`review` means read the list first: an orphan may be something you use
directly.

When part of a cache cannot be read without root, the size is a lower bound
and the output says so. Only package files are counted: a cache directory
that holds nothing but a lock file or an index is not listed.

On dnf the cache is reported as two items with two commands, because they
are two different decisions: the downloaded packages
(`sudo dnf clean packages`) and the repository metadata
(`sudo dnf clean metadata`), which dnf fetches again on its next run. On
Fedora a third item can appear: the copy of the repository metadata dnf5
keeps for each ordinary user in `~/.cache/libdnf5`. It is labelled `safe`,
and dnf makes the copy again the next time that user asks it about the
repositories.

Orphans come from the package manager's own answer. On a machine with no
repository metadata on disk, dnf and zypper cannot give that answer;
`clean` then says "orphans are not known" and why, and reports the rest.

To remove orphans through yapacc, with the list shown and a question asked:
`yapacc remove orphans`. See [install, remove, upgrade](changes.md).

## restart

`yapacc restart` says what still runs old code after an upgrade.

```console
$ sudo yapacc restart
Reboot: yes
  a newer kernel is installed: reboot to use it (6.12.117+deb13-amd64 was written after the running 6.12.107+deb13-amd64; judged by date only, so a second kernel kept side by side also shows up here)
  the distribution asks for a reboot (/run/reboot-required or /run/reboot-needed exists)
  asked for by: linux-image-6.12.117+deb13-amd64

Services to restart: 9
  dbus.service (system unit, pid 740)
      still maps: /usr/lib/x86_64-linux-gnu/libpcre2-8.so.0.14.0, /usr/lib/x86_64-linux-gnu/libexpat.so.1.12.3
      sudo systemctl restart dbus.service
      caution: restarting the message bus breaks the programs connected to it: reboot (or log out and in) instead
  ssh.service (system unit, pid 784)
      still maps: /usr/lib/x86_64-linux-gnu/libcrypto.so.3, /usr/lib/x86_64-linux-gnu/libz.so.1.3.1, /usr/lib/x86_64-linux-gnu/libpcre2-8.so.0.14.0
      sudo systemctl restart ssh.service
  systemd-logind.service (system unit, pid 748)
      still maps: /usr/lib/x86_64-linux-gnu/libpcre2-8.so.0.14.0, /usr/lib/x86_64-linux-gnu/libcrypto.so.3, /usr/lib/x86_64-linux-gnu/libz.so.1.3.1
      sudo systemctl restart systemd-logind.service
      caution: restarting it can end the graphical sessions that are open: save your work first, or reboot instead
  …

Programs to restart: 3
  (sd-pam) (pid 837, uid 1000)
      still maps: /usr/lib/x86_64-linux-gnu/libpcre2-8.so.0.14.0, /usr/lib/x86_64-linux-gnu/libcrypto.so.3, /usr/lib/x86_64-linux-gnu/libz.so.1.3.1
  systemd (pid 1, uid 0)
      still maps: /usr/lib/x86_64-linux-gnu/libcrypto.so.3, /usr/lib/x86_64-linux-gnu/libz.so.1.3.1, /usr/lib/x86_64-linux-gnu/libpcre2-8.so.0.14.0
      pid 1 cannot be restarted: `sudo systemctl daemon-reexec` on systemd, otherwise reboot
  systemd (pid 835, uid 1000)
      still maps: /usr/lib/x86_64-linux-gnu/libcrypto.so.3, /usr/lib/x86_64-linux-gnu/libz.so.1.3.1, /usr/lib/x86_64-linux-gnu/libpcre2-8.so.0.14.0

Examined 104 process(es).
Limits: Detects programs still mapping replaced files. Does not detect: interpreters holding replaced scripts, statically linked programs built against a replaced library, kernel live patches.
Nothing was restarted: `restart` only reports.
```

That is a Debian 13 machine just after an upgrade that brought a new
kernel and new OpenSSL, zlib and PCRE libraries. Each service comes with
the command to restart it, and with a caution where restarting it would
hurt (the message bus, the login manager). After the reboot it asks for,
the same command has nothing left to say:

```console
$ sudo yapacc restart
Reboot: no
  running kernel 6.12.117+deb13-amd64 still has its modules, no other kernel is more recent, no distribution flag is set

Services to restart: 0

Programs to restart: 0

Examined 102 process(es).
Limits: Detects programs still mapping replaced files. Does not detect: interpreters holding replaced scripts, statically linked programs built against a replaced library, kernel live patches.
Nothing was restarted: `restart` only reports.
```

**Reboot** is `yes`, `no` or `unknown`, with the reason. It uses what the
system offers: the distribution's own reboot flag (`/run/reboot-required`,
`/run/reboot-needed`), `needs-restarting -r` on dnf systems, a comparison of
the running kernel with the installed ones, and `freebsd-version` on
FreeBSD. `unknown` means none of these could answer, and the line says why.

**Services** and **programs** are processes that still map a file an
upgrade replaced. On systemd, processes are matched to their service. On
other init systems the programs are listed and the output tells you how to
restart a service there.

An ordinary user cannot read every process. `restart` then says how many it
could not examine ("Not examined: 16 process(es) that this user may not
read") and to run it as root for the whole picture: on the machine above,
before the reboot, the same command without `sudo` found the kernel and
none of the nine services. The `Limits` line
at the end states what this method cannot see at all.

## configs

`yapacc configs` lists configuration files that an upgrade left for you to
merge: `.pacnew` and `.pacsave` on Arch; `.dpkg-dist`, `.dpkg-old`,
`.dpkg-new` and `.ucf-dist` on Debian and Ubuntu; `.rpmnew` and `.rpmsave`
on Fedora and openSUSE; `.apk-new` on Alpine.

```console
$ yapacc configs
/etc/ca-certificates.conf.dpkg-old
    belongs to: /etc/ca-certificates.conf
    .dpkg-old: your old file, saved; the package's version is live
    written: 2026-09-14; your file: 2026-11-19
    the files differ: compare them, merge what you want, then delete the pending file
      diff -u /etc/ca-certificates.conf /etc/ca-certificates.conf.dpkg-old

1 pending file(s) under /etc. Nothing was changed: `configs` only reports.
5 directories under /etc could not be read and are not covered: run as root to see them.
```

For each file it says which file it belongs to, which of the two is yours,
when each was written, and the command to compare them. A saved file whose
live file is gone (a `.pacsave` or `.rpmsave` left by a removed package) is
described as that: "your old file, saved; the live file no longer exists".
It merges nothing.
Directories under `/etc` that an ordinary user cannot read are counted and
reported, so you know when to run it again as root.

[`yapacc briefing`](briefing.md) shows the same list in short form, because
unmerged configuration is worth settling before the next upgrade adds to
it.
