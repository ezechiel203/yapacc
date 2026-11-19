# Scripting: JSON and exit codes

yapacc is meant to be usable from scripts without parsing prose. Two things
make that possible: read commands can print JSON, and the exit code says
what kind of outcome it was.

## Exit codes

| Code | Name | Meaning |
|---|---|---|
| 0 | ok | the command did what was asked |
| 1 | failed | the operation ran and failed |
| 2 | usage | bad arguments, or a configuration file that does not parse |
| 3 | refused | a gate or a guard said no; nothing was changed |
| 4 | not found | the package, record or plan does not exist |
| 5 | integrity | a hash, signature or chain check failed |
| 6 | unavailable | something the command needs is not available: a tool or an optional component that is not installed, a file this user may not read, a feature this build lacks, or a command that does not exist for this package manager |

The message goes to standard error as `error: …`.

The codes that matter most in a script are 3 and 6. A 3 means the system is
exactly as it was, by decision. A 6 means the machine lacks something, and
the message names it.

```sh
yapacc --script upgrade --yes
case $? in
  0) echo "upgraded" ;;
  3) echo "refused, nothing changed" ;;
  6) echo "a tool is missing" ;;
  *) echo "failed" ;;
esac
```

## Never waiting for input

`--script` promises that yapacc never prompts. A command that would have
asked `[y/N]` is refused unless `--yes` or `--dryrun` is given, and a
privilege tool that would have asked for a password is called in its
non-interactive mode and fails at once.

Without `--script`, yapacc still never reads an answer from a pipe:
standard input that is not a terminal cannot confirm anything.

## JSON

`--json` is a global option and goes before the command:

```console
$ yapacc --json platform
$ yapacc --json why bash
$ yapacc --json timeline --since 2026-11-01
$ yapacc --json list --explicit
$ yapacc --json doctor
```

These commands have a JSON form on every system: `why`, `timeline`,
`briefing`, `clean`, `restart`, `configs`, `doctor`, `platform`, `list`,
`info`, `search`, `check`, `inspect owns`, `inspect files`,
`inspect orphans`, `names`, `get --explain`, `snapshot list`, `config`,
`capabilities`, and the ledger reads. `yapacc docs json` prints the complete
list, including the Arch-only ones.

A command with no JSON form prints nothing and exits with code 2
("no JSON output for this command"). It never prints prose with a success
status, so a script cannot mistake one for the other.

Fields are only ever added. Treat a key you do not know as ignorable, and
do not depend on the order of keys.

An empty answer is an empty array. In text mode the same answer is one
line in parentheses, such as "(no orphan: every package installed as a
dependency is still needed)", so that a blank screen never has to be
interpreted.

```console
$ yapacc --json platform
{
  "os": {
    "id": "debian",
    "id_like": [],
    "pretty_name": "Debian GNU/Linux 13 (trixie)",
    "version_id": "13"
  },
  "family": "debian",
  "arch": "x86_64",
  "libc": "glibc",
  "native_pkg_manager": "apt",
  "pkg_managers": [
    "apt"
  ],
  "extra_sources": [],
  "init": "systemd",
  "boot": {
    "firmware": "bios",
    "managers": [],
    "loaders": [
      "grub"
    ],
    "incomplete": true
  },
  "log_systems": [
    "journald"
  ],
  "package_logs": [
    "/var/log/dpkg.log",
    "/var/log/apt/history.log"
  ],
  "snapshot_providers": [],
  "root_fstype": "ext4",
  "container": false,
  "image_based": false,
  "support_tier": 1
}
```

What a command has to add in words (a source it could not read, "no
repository metadata on disk") goes to standard error as `note: …` lines.
Standard output holds the JSON alone, so `yapacc --json check | jq` works
on a machine that has never refreshed: the array is empty and the note
says why.

A few shapes that are useful to know:

| Command | Shape |
|---|---|
| `platform` | one object; `native_pkg_manager` is `"pacman"`, `"apt"`, `"dnf"`, `"zypper"`, `"apk"`, `"xbps"` or `"pkg"`; `package_logs_unreadable` lists the logs that exist and that this user may not read, and is left out when there is none |
| `why NAME` | one object; `native.manager`, `native.reason`, `native.required_by[]`, `native.base`, and `orphan` (`true`, `false` or `null`) |
| `timeline` | array of `{ts, source, action, name, old_version, new_version}`; `ts` is Unix seconds |
| `doctor` | array of `{name, state, detail}`; `state` is `ok`, `warn`, `fail` or `n/a` |
| `clean` | array of `{what, size_bytes, count, safety, command, detail[]}` |
| `list` | array of `{name, version, description, …}` |
| `check` | array of `{name, old_version, new_version, repo}` on every package manager; on apt `repo` holds every origin, separated by `", "`; on pacman it is `""` for a repository update and `"aur"` for an AUR update |

The shapes of `info` and `inspect orphans` differ between pacman and the
other package managers; `yapacc docs json` describes both.

## A health check in two lines

```sh
# Print the failing checks, and exit non-zero when there is one.
yapacc --json doctor | jq -r '.[] | select(.state == "fail") | "\(.name): \(.detail)"'
yapacc --json doctor | jq -e 'all(.[]; .state != "fail")' > /dev/null
```

The same list, filtered the other way: everything that is not `ok` on the
machine of the [quick start](quick-start.md).

```console
$ yapacc --json doctor | jq -r '.[] | select(.state != "ok") | [.state, .name] | @tsv'
n/a	component:yapacc-tui
n/a	component:yapacc-mcp
n/a	sandbox
n/a	ledger:user
n/a	ledger:system
n/a	tool:flatpak
n/a	tool:gpgv
n/a	tool:bwrap
```

## Shell completions

```console
$ yapacc completions bash > ~/.local/share/bash-completion/completions/yapacc
$ yapacc completions zsh  > ~/.zsh/completions/_yapacc
$ yapacc completions fish > ~/.config/fish/completions/yapacc.fish
```

The release archive and the distribution packages install them for you.
