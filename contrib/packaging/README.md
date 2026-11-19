# Packaging yapacc

Recipes for distribution packages, and the layout every one of them — and
the release archive — must respect. The recipes are kept beside the source;
none of them is in a distribution's repository. A release publishes them
filled in (version and checksum of the source tarball) as
`yapacc-<version>-packaging.tar.gz`.

There is deliberately no `curl | sh` installer. Install a package you built
from a recipe, or unpack a release archive you have verified (last section).

## The recipes

| Distribution | File | Build locally | What is installed where |
|---|---|---|---|
| Arch Linux | [`arch/PKGBUILD`](arch/PKGBUILD), [`arch/.SRCINFO`](arch/.SRCINFO) | `cd contrib/packaging/arch && makepkg -si` | One package `yapacc`: `/usr/bin/yapacc`, `/usr/lib/yapacc/yapacc-tui`, `/usr/lib/yapacc/yapacc-mcp`, man page, bash/zsh/fish completions, `/usr/share/licenses/yapacc/LICENSE` |
| Debian, Ubuntu | [`debian/`](debian/) | from the top of the source tree: `cp -r contrib/packaging/debian debian && dpkg-buildpackage -us -uc -b` | `yapacc`: `/usr/bin/yapacc`, man page, completions. `yapacc-tui`: `/usr/lib/yapacc/yapacc-tui`. `yapacc-mcp`: `/usr/lib/yapacc/yapacc-mcp` |
| Fedora and relatives | [`rpm/yapacc.spec`](rpm/yapacc.spec) | `rpmbuild -ba contrib/packaging/rpm/yapacc.spec` with the source tarball in `~/rpmbuild/SOURCES` (or a COPR with network access enabled) | `yapacc`: `/usr/bin/yapacc`, man page, completions, licence. `yapacc-tui`: `/usr/libexec/yapacc/yapacc-tui`. `yapacc-mcp`: `/usr/libexec/yapacc/yapacc-mcp` |
| Alpine Linux | [`alpine/APKBUILD`](alpine/APKBUILD) | `cd contrib/packaging/alpine && abuild -r` | Core only: `/usr/bin/yapacc`; man page in `yapacc-doc`; completions in `yapacc-{bash,zsh,fish}-completion` |
| Void Linux | [`void/template`](void/template) | copy to `srcpkgs/yapacc/template` in a void-packages checkout, then `./xbps-src pkg yapacc` | Core only: `/usr/bin/yapacc`, man page |
| Any Linux | release archive `yapacc-<version>-<arch>-linux-musl.tar.gz` (x86_64, aarch64) | `cargo xtask dist` ([`crates/xtask/src/dist.rs`](../../crates/xtask/src/dist.rs)); the release workflow runs the same command on a `v*` tag | static binaries; unpack with `tar -xzf … --strip-components=1 -C /usr/local` (or `~/.local`): `bin/yapacc`, `lib/yapacc/yapacc-{tui,mcp}`, `share/man/man1/yapacc.1`, `share/…` completions, `LICENSE`, `README.md` |

What applies to all of them:

- **Rust 1.89 or newer** is required (`rust-version` in `Cargo.toml`; CI
  checks that version). A distribution whose `rustc` is older needs a newer
  toolchain to build: Debian 13 ships rustc 1.85, which is why the Debian
  recipe is built on Debian testing.
- **Crates are downloaded** at the versions pinned in `Cargo.lock` unless
  they are vendored. The Arch and Alpine recipes fetch in `prepare()` and
  build offline afterwards (`--frozen`); the Debian rules build offline when
  a `vendor/` directory exists; the RPM spec needs network during `%build`.
- **Feature flags.** Every recipe builds the core and `yapacc-mcp` with
  `--no-default-features`, and `yapacc-tui` with `--features tui` added, so
  the packaged core is the small one the size budget is about.
- **No recipe enables the `alpm` feature**, Arch included. The `alpm` crate
  is GPL-3.0 and yapacc is GPL-2.0-only: a binary built with that feature
  may only be distributed under GPL-3 terms. On Arch yapacc uses pacman's
  command line; the feature remains for local builds (`Cargo.toml`).
- **Tests** run in each recipe's check step, without a skip list: a test of
  a command that exists only with pacman checks, on any other system, that
  the command refuses cleanly. A few tests reach for the network and are
  written to tolerate its absence. Each recipe says how to skip the step.
- The source tarball unpacks to `yapacc-<version>/`.

Where each recipe can go, as things are:

- **Arch**: the recipe is in the form the AUR expects. It is not in the AUR.
- **Debian**: not a candidate for the Debian archive as it is (an official
  build has no network, so every crate would have to be packaged or
  vendored, with a `debian/copyright` stanza per licence; there is no
  `debian/watch`). The `.deb` files it builds can be published as release
  assets; the release workflow does not build them.
- **Fedora**: the spec downloads crates in `%build` and calls cargo
  directly, so it fits a COPR with network access, not Fedora proper (which
  wants `%cargo_build`, packaged crates, and their licences in `License:`).
  The RPMs can be published as release assets; the workflow does not build
  them either.
- **Alpine**: conventional, except that aports expects `cargo-auditable`.
- **Void**: void-packages does not accept software without established
  users, so the template stays here for those who build it themselves.

## What has been built

Recorded in [`STATUS.md`](../../STATUS.md): on 2026-11-16 the five recipes
were built and the packages installed in clean containers (Arch, Debian
testing, Fedora, Alpine, Void).

The recipes changed on 2026-11-19 (no `alpm` feature on Arch, the core built
without `tui` on Debian, no skip lists, the real project address). After
that change:

- **Arch**: in a clean `archlinux:base-devel` container, as a non-root
  user, `makepkg` built the package from the filled recipe and the source
  tarball, `check()` included (all tests passed, none skipped);
  `makepkg --printsrcinfo` printed exactly the filled `.SRCINFO`; the
  package was installed with `pacman -U`; `yapacc --version` and
  `yapacc doctor` ran (exit 0, both components found in
  `/usr/lib/yapacc`), and `ldd /usr/bin/yapacc` names no libalpm. That
  build ended with makepkg's warning "Package contains reference to
  $srcdir": the binaries carry the path of the build directory.
- **Debian testing**: in a clean `debian:testing` container (rustc 1.98.1),
  as a non-root user, `dpkg-buildpackage -us -uc -b` built the three
  packages, tests included (all passed, none skipped); they were installed
  with `apt-get install ./…deb`; `yapacc --version` and `yapacc doctor` ran
  (exit 0, both components found in `/usr/lib/yapacc`).
- **Fedora, Alpine, Void**: not rebuilt since the change.

Install, upgrade and uninstall of the built packages are checked, in
containers, by [`contrib/test/pkg-lifecycle.sh`](../test/pkg-lifecycle.sh)
(Debian, Fedora, Alpine; see its header for what it asserts and skips).

## Filling in a release

`fill-release.sh VERSION SOURCE_TARBALL` (POSIX `sh`; `-d DIR` edits a copy
of this directory instead of the checkout) sets the version and writes the
checksum of the tarball into the Arch `PKGBUILD` and `.SRCINFO` (sha256), the
Alpine `APKBUILD` (sha512, abuild's format), the Void `template` (sha256),
the RPM spec (`Version`, `Release`, a `%changelog` entry) and the Debian
`changelog` (a new entry for a new version; the distribution, `UNRELEASED`
in the checkout, becomes `$DEB_DISTRIBUTION`, by default `unstable`). It is
idempotent, and with `SOURCE_DATE_EPOCH` set the entries it adds carry that
date. `cargo xtask dist` runs it on a copy, refuses to go on if a
placeholder is left (`SKIP`, an empty or zero checksum, `UNRELEASED`), and
packs the copy as `yapacc-<version>-packaging.tar.gz`. `shellcheck` is clean
on the script.

**Where the recipes download the source from.** The source URL in every
recipe is
`https://github.com/ezechiel203/yapacc/releases/download/v<version>/yapacc-<version>.tar.gz`:
the tarball `cargo xtask dist` builds (`git archive` of the commit,
`gzip -n`) and the release publishes as an asset next to the archives. The
checksum filled in is of that very file, so it matches what the URL serves
(GitHub's generated `/archive/` tarballs are not used: their bytes are not
guaranteed to stay the same). The URL resolves once the release of that
version is published; until then a recipe builds from a tarball placed
beside it (`git archive --format=tar --prefix=yapacc-<version>/ HEAD | gzip -n > yapacc-<version>.tar.gz`).

## The layout contract

yapacc is several small binaries. Only the core is required.

| What | Where | Required |
|---|---|---|
| `yapacc` (core) | a `bin` directory on `PATH` | yes |
| `yapacc-tui`, `yapacc-mcp` (components) | `lib/yapacc/` or `libexec/yapacc/` beside that `bin` directory | no |

The core finds a component **only** in these places, in the words of
[docs/ARCHITECTURE.md §2](../../docs/ARCHITECTURE.md): next to the running
binary, in `../lib/yapacc`, in `../libexec/yapacc`, in `/usr/lib/yapacc`,
and in `$YAPACC_LIBEXEC` — never on the whole `PATH`. So:

- `/usr/bin/yapacc` with components in `/usr/lib/yapacc/` (Arch, Debian) or
  `/usr/libexec/yapacc/` (Fedora) works;
- the release archive unpacked under any prefix works, because `bin/` and
  `lib/yapacc/` keep their relative positions;
- a component placed in `/usr/bin` next to the core also works, but a
  package should not do that: components are not commands for the user.

A package must not put a component anywhere else, and must not rename one.
A missing component is not an error of the package: `yapacc i` and
`yapacc mcp` then exit with code 6 and name what to install.

A package must also keep yapacc's promises (README, "What it promises"):
install **no** service, timer, hook, alias or configuration file. The units
in `contrib/systemd/` are for the user to install on request.

## Building the release files yourself

```sh
cargo xtask dist            # writes target/dist/
```

builds what a release publishes: the two archives (the aarch64 one when the
rust target `aarch64-unknown-linux-musl` is installed; `--require-aarch64`
makes its absence an error), the source tarball, the filled recipes and
`SHA256SUMS`. It refuses to run on a tree with uncommitted changes
(`--allow-dirty` overrides), checks that the binaries are static, that the
core is within its size budget and that no path of the build machine is
compiled in, and starts each binary (the aarch64 ones under `qemu-aarch64`
when it is installed). Two runs on the same commit with the same toolchain
give the same bytes. The release is built with the toolchain named in
`.github/workflows/release.yml`; to use the same one:
`cargo +<that version> xtask dist`.

## Verifying a release

Each release carries the archives and a `SHA256SUMS` file, and each archive
has a build provenance attestation: a signed statement that it was built by
the release workflow of this repository from the tagged commit.

```sh
# 1. The archive is the one the release lists.
sha256sum -c SHA256SUMS --ignore-missing

# 2. The archive was built by the project's release workflow.
gh attestation verify yapacc-0.1.0-x86_64-linux-musl.tar.gz --repo ezechiel203/yapacc

# 3. Unpack under a prefix; nothing outside it is touched.
tar -xzf yapacc-0.1.0-x86_64-linux-musl.tar.gz --strip-components=1 -C "$HOME/.local"
yapacc --version && yapacc doctor
```

The checksum alone protects against a damaged download, not against a
replaced release: whoever can replace the archive can replace `SHA256SUMS`
too. The attestation is what ties the archive to the source.

To remove what step 3 unpacked: `bin/yapacc`, `lib/yapacc/`,
`share/man/man1/yapacc.1`, the three completion files, `LICENSE` and
`README.md` under that prefix.

To check that the one binary really answers on every supported family, run
[`contrib/test/families.sh`](../test/families.sh) against it (needs podman
or docker).
