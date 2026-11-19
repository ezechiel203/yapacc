# Fixtures

Real output captured from real tools. Tests parse these files; nobody edits
them by hand. To refresh, run the capture script again and commit the result
as its own change.

## `native/<family>/`

Output of each distribution's package tools, captured inside throwaway
containers by `contrib/fixtures/capture-native.sh` on 2026-11-15
(Debian 13, Ubuntu 26.10, Fedora 45, Arch). File format:

```
# cmd: <the exact command that was run>
<raw output, unmodified>
# exit: <exit status>
```

Lines starting with `# ` at the top and the final `# exit:` line are not
part of the tool's output. A test helper strips them.

One file is not captured: `native/fedora/updates-synthetic.txt` was written
by hand because the image had no pending upgrades. It says so in its header.

There are four families here: `arch`, `debian`, `ubuntu` and `fedora`.
There are no fixtures for zypper, apk, xbps or FreeBSD pkg. The parser tests
for those four use text written in the test itself, and the zypp history
sample is written from the documented format, not captured
(`crates/octo-head/src/pkglog.rs`). yapacc itself was run against the real
tools of all four, on virtual machines or in containers; see
[docs/VERIFICATION.md](../docs/VERIFICATION.md).

## `taxonomy/`

- `corpus.tsv`: 3,678 lines of `name<TAB>description`: every fifth package
  of the Arch repositories plus the Debian and Ubuntu search fixtures.
- `golden.tsv`: `name<TAB>category` for each corpus line, in the same
  order, as classified by `src/taxonomy.rs` on 2026-11-15. It is the
  reference for any later change to the classifier: it records what the
  rules answered on that day, not what is correct. No test reads these two
  files today.
