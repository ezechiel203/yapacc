# doctor and platform

Two read-only commands that describe the machine. Run `doctor` first on any
new system, and attach both to a bug report.

## doctor

`yapacc doctor` prints one line per check: what works on this machine, what
does not, and why.

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

| State | Meaning |
|---|---|
| `ok` | it works |
| `warn` | something a feature of this system uses is missing or weaker; the line says what you lose |
| `fail` | a command will not work until this is fixed; the line says why |
| `n/a` | the check does not apply to this system, or an optional part (`yapacc-tui`, `yapacc-mcp`, flatpak, gpgv) is not installed; the line says what it would be for |

A healthy machine shows `ok` and `n/a` only, as above.

The lines for `undo`, `timeline` and `briefing` say what each can do *on
this machine*: where old versions would come from, which log is read,
whether there is a news feed. A system without a package log gets "no
timeline" here, not an empty timeline later.

`yapacc --json doctor` gives the same list as an array of
`{name, state, detail}`.

## platform

`yapacc platform` says what kind of machine this is, one line per axis.
Each axis is detected on its own; yapacc does not assume that a
distribution implies an init system or a boot loader.

```console
$ yapacc platform
distribution      Debian GNU/Linux 13 (trixie) [debian] (family: debian)
architecture      x86_64 / glibc
native packages   apt
extra sources     none detected
init              systemd
firmware          bios
boot managers     none detected
bootloaders       grub  (incomplete: boot partition not readable without root)
log systems       journald
package logs      /var/log/dpkg.log, /var/log/apt/history.log
root filesystem   ext4
snapshots         none detected
support tier      1 — main target
```

Where an axis could only be read in part, the line says so, as for the boot
loader above. A package log that is there but closed to you is marked
`(exists; not readable by this user)`.

When a machine has several package managers installed, the native one is
the one whose database owns `/usr/lib/os-release`. dnf and zypper read the
same rpm database; between them it is the one that has repositories
configured, and zypper on SUSE, dnf elsewhere, when both have. The other
one is listed on an `other managers` line.

The last line is the support tier of the native package manager.
[What works where](support.md) explains the tiers.
