# Contributing to yapacc

Thank you for looking. yapacc is a small project with firm rules about what
it will and will not do; reading this page first will save you time.

## Before you write code

- **Read the rules.** [docs/STRATEGY.md](docs/STRATEGY.md) lists what yapacc
  is, what it must never do, and what has been cut on purpose. A change
  that adds a daemon, a copy of the package database, or a way around the
  gate will not be merged however well it is written.
- **Open an issue for anything larger than a fix.** Agreeing on the shape
  first is cheaper than rewriting a pull request.
- **Bug reports** need the output of `yapacc platform` and `yapacc doctor`,
  the exact command, what you expected and what happened.
- **Security problems** go through [SECURITY.md](SECURITY.md), not the
  issue tracker.

## Building and testing

You need Rust 1.89 or newer.

```console
$ cargo build
$ cargo test --workspace --all-targets
$ cargo fmt --all
$ cargo clippy --workspace --all-targets -- -D warnings
```

Before you open a pull request, run what CI runs:

```console
$ cargo xtask ci
```

That runs four jobs: lints and tests of the portable build, the static
musl build with its size budget (the core must stay under 6 MiB), the tests
in an Arch Linux container, and one static binary in containers of seven
distributions. The last two need `podman` or `docker`;
`cargo xtask ci --no-containers` skips them, and `cargo xtask ci portable`
runs one job.

### Testing on real systems

Parsers are tested against output captured from the real tools, under
`fixtures/`. Nobody edits those files by hand: they are regenerated with
`contrib/fixtures/capture-native.sh`, in containers.

Behaviour that depends on a booted system (prompts on a terminal, `sudo`
and `doas`, reboots, snapshots) is checked on virtual machines built by
[`contrib/test/vm/vm.sh`](contrib/test/vm/README.md) from the
distributions' public cloud images. If your change touches the write path
for a package manager, say in the pull request which system you ran it on
and what you observed. "Not run on a real system" is an acceptable
statement; a claim that was not checked is not.

## What a change must respect

- **No shell strings built from data.** Commands are argument vectors.
  Package names are validated and passed after `--`.
- **Every change to the system is gated**, shows the native manager's own
  simulation first, and does nothing under `--dryrun`.
- **Exit codes mean something:** 0 ok, 1 failed, 2 usage, 3 refused,
  4 not found, 5 integrity, 6 tool unavailable. A refusal is always 3.
- **Read commands write nothing** and contact nobody, unless the command's
  help text says otherwise.
- **Messages are plain, exact sentences.** When something was not done or
  could not be checked, the output says so.
- **Parse with `LC_ALL=C`**, and never assume the tool is installed: a
  missing tool is exit code 6 with its name.
- **Tests for every fix.** A parser fix comes with the input that broke it.

## Documentation

User documentation is the handbook in [`docs/handbook`](docs/handbook/README.md):
plain Markdown, one page per topic, listed in `SUMMARY.md`. The website is
generated from it:

```console
$ cargo xtask site
$ python3 -m http.server --directory target/site 8000
```

The build checks every internal link and fails on a broken one.

Documentation states only what is true of the code today, and says what was
not verified. Terminal examples are real output, shortened where marked
with `…`. If your change alters behaviour, update the handbook page, the
built-in topic (`src/docs.rs`) and the manual page
(`contrib/man/yapacc.1`) in the same pull request, and add a line to
[CHANGELOG.md](CHANGELOG.md).

## Licence of contributions

yapacc is licensed under the GNU General Public License, version 2 only.
By submitting a contribution you agree that it is licensed under the same
terms, and you confirm that you have the right to submit it. Sign your
commits off (`git commit -s`) to state this, as in the
[Developer Certificate of Origin](https://developercertificate.org/).

Dependencies must be compatible with GPL-2.0-only. `cargo deny check
licenses` runs in CI and decides.
