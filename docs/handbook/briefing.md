# yapacc briefing

`yapacc briefing` tells you what the next upgrade will do before you run
it. It is read-only: it reads the list of pending updates and reports what
deserves attention.

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

## The sections

**Needs attention** sorts the pending packages by what they mean for you:

| Label | Meaning |
|---|---|
| `reboot` | a kernel, C library or init system update: the new code runs only after a reboot. Kernels are recognised under the names each system gives them (`linux-image-…`, `kernel-default`, `linux6.12`, `linux-lts`, FreeBSD's `FreeBSD-kernel-…`) |
| `care` | the package manager itself, the boot loader or remote access is being updated: do not interrupt, and keep a second session open over ssh |
| `info` | major version changes, worth a look at the release notes |

**Security fixes** lists the pending updates that fix known
vulnerabilities, where the system has a source for that: the security
repository on Debian and Ubuntu (an update that several repositories offer
counts when one of them is the security one), the advisory data of dnf and
zypper, and `arch-audit` on Arch when it is installed. Where there is no
source, a note says so instead of printing an empty list.

**Distribution news** shows recent announcements from the distribution's
feed. Lines marked `!` announce a manual step. This is the one part of
`briefing` that uses the network (through `curl`); `--offline` skips it.
Distributions without a machine-readable feed get a note saying so, as
Debian does above. On Arch, which has one:

```console
$ yapacc briefing
Upgrade briefing — 25 package(s) pending (pacman)

Needs attention
  reboot  C library — every running program keeps the old one until restarted; reboot when convenient: glibc 2.44+r50+g1848099f063e-1 -> 2.44+r78+gfcfb867c58ab-1
  reboot  init system — reboot recommended: systemd 262.1-1 -> 262.2-1, systemd-libs 262.1-1 -> 262.2-1
  care    the package manager itself — let it finish alone; do not interrupt: pacman 7.1.0.r9.g54d9411-3 -> 7.1.0.r9.g54d9411-4
  care    remote access — if you are connected over ssh, keep a second session open until you have reconnected: openssl 3.6.5-1 -> 4.1.0-1
  info    major version changes (1): openssl 3.6.5-1 -> 4.1.0-1

Distribution news (newest first) — manual steps are announced here
    Update to php ≥ 8.5.12 and/or php-legacy ≥ 8.3.36 to prevent remote code execution  (Wed, 21 Oct 2026 13:58:42 +0000)
    Update to ca-certificates-mozilla ≥ 3.131.1-1 to prevent man-in-the-middle attacks  (Wed, 21 Oct 2026 13:52:37 +0000)
  ! Mkinitcpio >=42 requires manual intervention for TPM2-based unlocking of LUKS devices  (Tue, 22 Sep 2026 09:09:27 +0000)
  ! virtualbox-ext-vnc >= 7.2.12-2 requires manual intervention  (Tue, 21 Jul 2026 13:01:46 +0000)

…
```

**Configuration files still unmerged** lists what earlier upgrades left
for you to merge. [`yapacc configs`](housekeeping.md#configs) shows how to
compare each one.

**Disk** shows free space where packages are written.

**Safety net** says whether a snapshot tool is available and reminds you
what `undo` does and does not restore.

## It does not refresh

`briefing` reads the pending list as of your last repository refresh
(`apt-get update`, `pacman -Sy`, `dnf makecache` and so on) and says so in
a note. A read command never refreshes repositories for you, and never
lets the package manager do it behind your back: on dnf and zypper, where
the native tool would fetch metadata on its own, yapacc asks it for cached
data only. If the list looks old, refresh with your package manager, or run
`yapacc refresh`, which is gated like any other change.

On a machine that has never refreshed (a fresh image, a new container)
there is no pending list to read. yapacc says that, and names the command,
instead of reporting that nothing is pending:

```console
$ yapacc check
note: no repository metadata on disk: run `apt-get update` (yapacc does not refresh on a read command)
```

`briefing` then opens with "pending packages not known" and the same note.
On Arch, when `checkupdates` is installed but fails, the briefing says the
list below it is not current and why.

## Options

| Option | Effect |
|---|---|
| `--offline` | do not fetch the distribution's news feed |

## After the briefing

`yapacc upgrade` runs the full upgrade through the native manager, with the
plan and the gate. See [install, remove, upgrade](changes.md).
