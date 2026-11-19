# Configuration

yapacc needs no configuration. Without a file, the defaults below apply,
and they are the safe ones: nothing is snapshotted or applied on its own.

## Where the file lives

`~/.config/yapacc/config.toml`, or `$XDG_CONFIG_HOME/yapacc/config.toml`.
Another path can be given for one run with `--config PATH`. An
`XDG_CONFIG_HOME` that is empty or not an absolute path is ignored, as the
XDG specification says, and the same holds for the cache, state and data
directories: yapacc never derives one of its paths from the directory you
happen to be in.

```console
$ yapacc config
```

prints the effective values and the file they were loaded from. yapacc
never writes this file.

A file that does not parse, or a known key with an invalid value, is an
error (exit code 2). yapacc does not fall back to defaults silently,
because a protective setting that quietly stops applying is worse than a
command that refuses to run. A section or key it does not know gets one
warning line each, and `yapacc config` lists them, so a typo such as
`[safty]` does not go unnoticed.

Paths in the file must be absolute or start with `~/`.

## The keys

A complete example, [`yapacc.example.toml`](../../yapacc.example.toml),
ships with the source. The keys most people will want:

### `[safety]`

```toml
[safety]
# Snapshot before install, remove and upgrade through yapacc.
#   off               (default)
#   auto              take one if a tool is found; if it fails, refuse the change
#   auto-best-effort  take one if a tool is found; if it fails, warn and go on
snapshot = "off"

# Which tool: auto | snapper | timeshift | zfs | none
snapshot_provider = "auto"
```

See [snapshots](snapshots.md).

### `[mcp]`

```toml
[mcp]
# What an AI assistant connected through yapacc-mcp may do.
#   plan   it can read and propose; you run the command yourself (default)
#   allow  it may apply a change when it states that you approved;
#          honoured only in a file owned by root
apply = "plan"
```

See [AI assistants](mcp.md).

### `[build]`

For builds of third-party recipes (the AUR on Arch).

```toml
[build]
# require  refuse to build without a working sandbox (default)
# prefer   warn and build unconfined when the sandbox is unavailable
# off      never sandbox
sandbox = "require"

# fetch-only  network only while downloading sources (default)
# always      network for the whole build
network = "fetch-only"
```

### `[repair]`

```toml
[repair]
# Classify a failed build and suggest a fix. The one fix that changes the
# system, installing missing build dependencies, always asks first.
enabled = true
max_attempts = 3
```

### `[install]`

```toml
[install]
# The order in which `yapacc get` looks for a source.
order = ["repo", "aur", "flatpak", "recipe"]
```

### `[ledger]`

The ledger records what yapacc installed *outside* the native package
manager (AppImages, flatpaks, isolated installs). Native packages are never
recorded there; the package manager's own log is their record.

```toml
[ledger]
# Where to copy the ledger head after every change, so that tampering can be
# detected and not only accidental corruption: "journald", or a file path
# (ideally on another machine's mount).
witnesses = []
```

## Environment

| Variable | Effect |
|---|---|
| `XDG_CONFIG_HOME`, `XDG_CACHE_HOME`, `XDG_STATE_HOME`, `XDG_DATA_HOME` | where the configuration, caches, the ledger and data live |
| `LC_ALL`, `LC_MESSAGES`, `LANG` | the letters of the `[y/N]` question |
| `NO_COLOR` | honoured by the interactive interface; the command line prints no colour codes |

## Command-line options that apply everywhere

| Option | Effect |
|---|---|
| `--config PATH` | use this configuration file |
| `--json` | machine-readable output on the read commands; goes before the command |
| `--script` | never prompt; fail fast instead |
| `--noconfirm` | consent in advance, like `--yes` |
