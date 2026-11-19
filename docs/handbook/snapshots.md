# Snapshots

[`undo`](undo.md) restores package versions. A filesystem snapshot restores
everything else: configuration files, data that package scripts rewrote,
and the packages too. yapacc does not implement snapshots. It drives the
snapshot tool the machine already has.

## Which tools

| Tool | List | Create | Notes |
|---|---|---|---|
| snapper | yes | yes | the usual tool on openSUSE; works on any btrfs layout snapper is configured for |
| timeshift | yes | yes | rsync mode and btrfs mode |
| zfs | yes | yes | snapshots the dataset of the root filesystem |
| plain btrfs | yes | no | subvolumes are listed; yapacc does not create plain btrfs snapshots |

`yapacc platform` shows which of these it detected, on the `snapshots`
line.

## The commands

```console
$ yapacc snapshot list                         # read-only
$ yapacc snapshot create --description "before the big upgrade"
```

`snapshot create` is a change, so it goes through [the gate](gate.md):
`--dryrun` shows the command, `--yes` skips the question, and yapacc calls
`sudo` or `doas` as it does for any change. `snapshot list` is a read
command and elevates nothing: where the tool only answers root, it says so
(exit code 6), and you run `sudo yapacc snapshot list`.

On a machine without a snapshot tool, both say what was looked for:

```console
$ yapacc snapshot list
error: no snapshot provider is usable on this machine. Looked for: snapper with a root config (/etc/snapper/configs/root), timeshift, zfs with `/` on ZFS, btrfs with `/` on btrfs.
Filesystem of `/`: ext4.
`yapacc undo` still works without one, within its limits: it restores package versions only, not configuration files or data.
```

## A snapshot before every change

Set this in the [configuration](configuration.md) and yapacc takes a
snapshot before `install`, `remove` and `upgrade`, after you have consented
and before the native command runs.

```toml
[safety]
# off (default) | auto | auto-best-effort
snapshot = "auto"
# auto (default) | snapper | timeshift | zfs | none
snapshot_provider = "auto"
```

| Value | When a tool is found | When the snapshot fails |
|---|---|---|
| `off` | nothing | n/a |
| `auto` | a snapshot is taken first | the change is refused (exit code 3) |
| `auto-best-effort` | a snapshot is taken first | a warning is printed and the change goes on |

The plan names the tool and the command before you are asked, and a dry
run never takes a snapshot; it only says one would be taken.

### When no snapshot can be taken

The plan tells you before the question, and a dry run tells you the same:

| Situation | `auto` | `auto-best-effort` |
|---|---|---|
| `snapshot_provider = "auto"` and the machine has no snapshot tool | the change runs without a snapshot; the plan says "No snapshot will be taken: … there is no snapshot tool on this machine" | the same |
| a named provider that cannot work here | refused (exit code 3), with the reason and the three ways out | a warning in the plan; the change runs without a snapshot |
| the snapshot command fails | refused (exit code 3); nothing was changed | a warning; the change goes on |

The reason for a named provider is one of: the tool is not installed,
snapper has no root config (`snapper -c root create-config /` sets one
up), `/` is not on ZFS, or the provider is plain btrfs, which yapacc lists
and does not create.

The first row is deliberate. `auto` for the provider means "use what is
there", and on a machine with nothing there, refusing every change would
make the setting unusable in a configuration shared between machines. If
you want a change refused whenever no snapshot is possible, name the
provider.

> [!WARNING]
> If timeshift was never set up, its first `--create` picks a snapshot
> device on its own and writes its configuration. Set timeshift up yourself
> before letting anything call it. yapacc prints this warning in the plan
> when it applies.

## Restoring

yapacc creates and lists snapshots. It does not restore them. Rolling a
root filesystem back is done with the snapshot tool itself, often from a
rescue or boot environment:

- snapper: `snapper rollback`, or boot a snapshot from the boot menu on
  openSUSE;
- timeshift: `timeshift --restore`;
- zfs: `zfs rollback`, or boot environments with `bectl` on FreeBSD.

This is deliberate. A rollback of the running root is the kind of operation
where a companion tool should step aside.

## What was verified

Creating and listing were run against the real tools on virtual machines:
snapper on openSUSE Leap 16.1 and Tumbleweed and on Arch, timeshift 24.06
in rsync mode on Debian 13 and 26.11 in btrfs mode on Arch, and zfs on
FreeBSD 14.5 and 15.1 with a ZFS root. Not verified: a restore of any
kind (yapacc does not do one), and snapper on a distribution other than
those three.
