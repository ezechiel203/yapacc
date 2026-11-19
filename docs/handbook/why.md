# yapacc why

`yapacc why NAME` says why a package is installed, what still needs it,
where it came from and how it was verified. It is read-only, and it answers
for any package, whether or not yapacc had anything to do with installing
it.

```console
$ yapacc why libcurl4t64
libcurl4t64 8.14.1-2+deb13u5  — native package (apt)
  what         easy-to-use client-side URL transfer library (OpenSSL flavour)
  why          pulled in as a dependency
  because      libcurl4t64 ← curl  (you installed curl)
  needed by    curl
  from         mirror+file:/etc/apt/mirrors/debian.list trixie/main amd64 Packages
  installed    2026-09-14 04:34 UTC (last install or upgrade)
  verified     repository signature (apt verifies the signed Release index)
  packager     Debian Curl Maintainers <team+curl@tracker.debian.org>
  ledger       no yapacc record (installed outside yapacc, or by a native lane) — the facts above come straight from the package manager
```

## Reading the answer

| Line | What it tells you |
|---|---|
| `what` | the package's own one-line description |
| `why` | the reason it is installed; the wordings are listed below |
| `because` | the chain back to a package you installed yourself; several lines when several of your packages lead to it |
| `needed by` | the installed packages that depend on it directly |
| `from` | the repository it came from |
| `installed` | when it was last installed or upgraded |
| `verified` | how the package manager checked it |
| `packager` | who built the package, as the package records it |
| `ledger` | whether yapacc itself has a record of installing it |

The `why` line is one of these:

| It says | Meaning |
|---|---|
| you (or a tool acting for you) installed it explicitly | the package manager has it marked as asked for |
| pulled in as a dependency | something you installed needs it; `because` shows the chain |
| part of the base system | the package manager protects it (`Essential` on apt), or the distribution gives it priority `required` |
| installed as a dependency, but nothing needs it any more (orphan) | `yapacc inspect orphans` lists it, and `yapacc remove orphans` would remove it |
| installed as a dependency; nothing depends on it, but apt keeps it | another package recommends or suggests it, so `apt-get autoremove` leaves it alone |
| *manager* does not record an install reason here | the package manager has no such record for this package |

```console
$ yapacc why perl-base
perl-base 5.40.1-6+deb13u1  — native package (apt)
  what         minimal Perl system
  why          part of the base system (the package manager protects it)
  needed by    libtext-charwidth-perl
  from         mirror+file:/etc/apt/mirrors/debian.list trixie/main amd64 Packages
  installed    2026-09-14 04:32 UTC (last install or upgrade)
  verified     repository signature (apt verifies the signed Release index)
  packager     Niko Tyni <ntyni@debian.org>
  ledger       no yapacc record (installed outside yapacc, or by a native lane) — the facts above come straight from the package manager
```

The `from` line is never left empty. When the manager cannot say, the line
says that: on apt, a package whose installed version has since been
replaced in the index reads "the installed version is no longer in the
repository index (a newer one is, from …)"; on zypper the line names the
repository that offers the package now, because the zypp history does not
record where the installed version came from.

When part of the answer could not be checked, the line starts with `note:`
and says what was missing. yapacc does not guess.

Not every package manager records every fact, and a line that has no
source is left out: apk records no install date, and FreeBSD's pkg has no
`verified` line.

On a long `needed by` list the terminal shows the first few names and a
count. `--json` lists all of them.

## More than native packages

`why` prints one block for each place that knows the name: the native
package, a flatpak with a matching name, and anything yapacc installed
itself outside the package manager (an AppImage, for example).

## Exit codes

| Code | Meaning |
|---|---|
| 0 | found, answer printed |
| 2 | the name is not a valid package name |
| 4 | not installed anywhere yapacc can see |

## In a script

```console
$ yapacc --json why zlib1g
{
  "query": "zlib1g",
  "native": {
    "manager": "apt",
    "name": "zlib1g",
    "version": "1:1.3.dfsg+really1.3.1-1+b1",
    "description": "compression library - runtime",
    "reason": "explicit",
    "required_by": [
      "bind9-libs",
      "curl",
      "dpkg",
      …
    ],
    "origin": "mirror+file:/etc/apt/mirrors/debian.list trixie/main amd64 Packages",
    "foreign": false,
    "essential": true,
    "base": "priority required",
    "installed": "2026-11-19 13:10 UTC (last install or upgrade)",
    "verified": "repository signature (apt verifies the signed Release index)",
    "packager": "Mark Brown <broonie@debian.org>"
  },
  "chains": [],
  "orphan": null,
  "flatpak": [],
  "ledger": [],
  "notes": []
}
```

`reason` is `explicit`, `dependency` or `unknown`. `orphan` is `true` or
`false` as the package manager's own orphan list says, `null` when that
was not asked or is not known, and `native.base` is
`"essential"`, `"priority required"` or empty. `--json` goes before the
command. See [scripting](scripting.md).

## Related

- `yapacc inspect owns /usr/bin/env` finds the package that owns a file.
- `yapacc inspect files NAME` lists the files of a package.
- `yapacc inspect orphans` lists dependencies nothing needs any more;
  [`yapacc clean`](housekeeping.md) reports how much space they take.
