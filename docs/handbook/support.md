# What works where

This page says what yapacc does on each package manager, and how that was
checked. The run-by-run evidence is in the
[verification log](../VERIFICATION.md); the current state of the project is
in [STATUS.md](../../STATUS.md).

On your own machine, `yapacc doctor` is the authority: it reports what
works *there*.

## The tables

`why`, `list`, `info`, `search`, `check`, `inspect`, `clean`, `doctor` and
`platform` work on all seven package managers. The gated `install`,
`remove` and `upgrade` do too. The differences are in the rest.

### What you can ask

| Manager | It ran on | `timeline` | `briefing` adds | `restart` | `configs` |
|---|---|---|---|---|---|
| pacman | Arch | yes | distribution news; security with `arch-audit` | yes | yes |
| apt | Debian 13, Ubuntu 24.04 | yes | security, kernel | yes | yes |
| dnf | Fedora 45, Rocky 9.9 | yes | security | yes | yes |
| zypper | openSUSE Leap 16.1, Tumbleweed | yes, as root | security | yes | yes |
| apk | Alpine 3.24 | yes, when apk keeps a log | pending and reboot only | yes | yes |
| xbps | Void | no: xbps keeps no log | pending and reboot only | yes | no |
| pkg | FreeBSD 14.5, 15.1 | yes | pending and reboot only | kernel; processes with `checkrestart` | no |

### What you can take back

| Manager | `undo` | Old versions come from | Snapshot before a change ran with | Privilege tool it ran with |
|---|---|---|---|---|
| pacman | yes | the cache, then the Arch Linux Archive | snapper, timeshift | sudo, doas |
| apt | yes | the repositories, the cache, then snapshot.debian.org or Launchpad | timeshift | sudo, doas |
| dnf | yes | the enabled repositories | not run | sudo |
| zypper | yes | the repositories | snapper | sudo |
| apk | no | | not run | doas |
| xbps | no | | not run | sudo |
| pkg | no | | zfs | sudo, doas |

"Not run" means the code path exists and was never exercised on that
system. It is not a claim that it works.

## How it was checked

Every system in the first row was a booted virtual machine built from the
distribution's public cloud image (Void: an image built from the official
container), with an ordinary user going through `sudo` or `doas`, and as
root. The commands were run for real: packages installed, upgraded,
removed and restored, prompts answered at a terminal, machines rebooted
into old and new kernels.

On top of that, one static binary is run in containers of seven
distributions by the test suite, and the parsers are tested against output
captured from the real tools.

What that does not cover:

- **Real hardware.** Every run was a virtual machine or a container.
- **Other architectures.** Everything ran on x86_64. The aarch64 archive
  is built, and its binaries were started under emulation; nothing more
  was run on that architecture.
- **Derivatives.** Linux Mint, Pop!_OS, Manjaro, EndeavourOS, AlmaLinux,
  RHEL and SLES were not run. They use the same package managers, so the
  commands are expected to work, and `yapacc doctor` will tell you; `undo`'s
  archive fallback knows Debian, Ubuntu and Arch only.
- **Other users.** Until this release nobody but the author had used it.

## Tiers

`yapacc platform` prints a support tier for the native package manager.

| Tier | Managers | Meaning |
|---|---|---|
| 1 | pacman, apt, dnf | the main targets: everything in the table, including `undo` |
| 2 | zypper, apk, xbps, pkg | read commands and gated changes work; some features are missing, as the table shows |
| 3 | portage, nix | detected only; changes are refused, with the reason |

## Not supported

- **OpenBSD and NetBSD.** There is no backend for `pkg_add` or `pkgin`.
- **Portage and Nix.** yapacc recognises the system and refuses to change
  it, saying why. Read commands have no backend there.
- **Image-based systems** (ostree, such as Fedora Silverblue or openSUSE
  MicroOS). They are detected and nothing more: package changes on those
  systems are not what yapacc's gate models.

## Known limits

- `undo` restores package versions, not configuration or data, and it is
  all or nothing: when one old version cannot be found the whole undo is
  refused.
- zypper and dnf have no archive to fall back on. Once a repository drops
  an old version, an upgrade that replaced it cannot be undone. On
  Tumbleweed that is a day or two.
- Old packages from snapshot.debian.org are checked against the recorded
  hash and not signature-verified. Launchpad downloads are not verified.
  The output says which applies.
- Snapshots are taken through snapper, timeshift or zfs. Plain btrfs is
  listed only. yapacc never restores a snapshot.
- The sandbox for third-party builds has no seccomp filter and no resource
  limits.
- `restart` cannot see interpreters holding replaced scripts, statically
  linked programs, or kernel live patches, and it says so every time.

[STATUS.md](../../STATUS.md) carries the full list.
