# Troubleshooting and questions

## Questions

### Do I have to stop using apt, pacman or dnf?

No. Keep using them. yapacc reads what they record and is never confused
by a change you made directly. Its own `install`, `remove` and `upgrade`
are a convenience with a uniform gate, not a requirement.

### Does it keep its own database of my packages?

No. Every answer is read from the package manager at the moment you ask.
The only thing yapacc records is what it installed *outside* the package
manager (an AppImage, an isolated install), because nothing else would
remember those.

### Does anything run in the background?

No. There is no daemon, no timer and no package-manager hook. A timer for
update checks exists as an option you have to install yourself with
`yapacc monitor daemon-install`.

### Can `undo` break my system?

It changes packages, so it deserves the same care as any downgrade. It is
built to refuse when it cannot do the whole job: it will not apply half an
undo, will not remove the running kernel, and shows the native manager's
own simulation before asking. What it cannot do is restore configuration
or data that the upgrade changed. For that, take a
[snapshot](snapshots.md) before big upgrades.

### How far back can I undo?

One transaction: the last one. And only if the old versions can still be
found. On pacman, apt on Debian and apt on Ubuntu there is a distribution
archive to fall back on. On dnf and zypper there is not.

### Why is there no `curl | sh` installer?

Because the point of the tool is that you see what will happen first. The
[install page](install.md) is six lines, and you can read every one.

### Is it safe to give an AI assistant access?

In the default configuration the assistant can read and propose, and
cannot apply. See [AI assistants](mcp.md) for what changes if you allow
more.

### What does "tier 1" in `yapacc platform` mean?

It is the support level of your package manager; see
[what works where](support.md#tiers).

### Why GPL-2.0-only?

It is the licence the author chose. yapacc runs your package manager as a
separate program and links none of its code.

## Troubleshooting

### `error: … is not on PATH` (exit code 6)

A tool that command needs is missing, and the message names it. Common
ones: `curl` (for `names`, the news feed and `undo` downloads), `sudo` or
`doas` (for any change as an ordinary user).

`sudo yapacc …` cannot find the binary on openSUSE when it is installed in
`/usr/local/bin`: openSUSE's `sudo` does not search there. Use the full
path or install into `/usr/bin`.

### `timeline` or `undo` says it cannot read the log

Some distributions keep the package log readable by root only (openSUSE's
`/var/log/zypp/history`). Run the command with `sudo`.

### `timeline` is empty

- On Void: xbps keeps no log, so there is nothing to show.
- On Alpine: `/var/log/apk.log` is written by apk-tools 3 only.
- After a log rotation: rotated and compressed logs are read, but a
  distribution that deletes old logs deletes that history.

### `undo` is refused: "in no cache, repository or archive"

The exact old version of some package can no longer be found. On rolling
distributions without an archive (Tumbleweed) this happens a day or two
after an upgrade. A filesystem snapshot is the remedy next time; there is
no remedy afterwards.

### `--script` fails with "Authorization required" or "a password is required"

`--script` never prompts, so `sudo` or `doas` is called in its
non-interactive mode. Either run as root, or allow that one command
without a password in `sudoers` or `doas.conf`. yapacc tells you whether
the tool refused for lack of a password or for lack of a rule.

### The command stopped with exit code 3

That is a refusal, and nothing was changed. Either you answered no, or the
command could not ask (`--script`, a pipe, no terminal) and you gave
neither `--yes` nor `--dryrun`. See [the gate](gate.md).

### `briefing` shows no security section

It depends on the system having a source for that: the security repository
on Debian and Ubuntu, advisory data on dnf and zypper, `arch-audit` on
Arch. Where there is none, a `note:` line says so.

### `restart` lists nothing, but I just upgraded

As an ordinary user you can only examine your own processes. The last
lines say how many could not be examined. Run it with `sudo`.

### The `[y/N]` question is in another language

It follows your locale (`LC_MESSAGES`). English `y` and `yes` are accepted
in every locale.

## Reporting a problem

Open an issue at
[github.com/ezechiel203/yapacc](https://github.com/ezechiel203/yapacc/issues)
with the output of these two commands and the exact command that
misbehaved:

```console
$ yapacc platform
$ yapacc doctor
```

`yapacc timeline --html > timeline.html` adds the recent package history
as one self-contained file, if the problem appeared after an upgrade. Look
through it first: it lists your installed package names.

For a security problem, do not open a public issue. See
[safety and privacy](safety.md#reporting-a-vulnerability).
