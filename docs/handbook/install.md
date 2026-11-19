# Install

yapacc is one static binary. There are three ways to get it: a release
archive, a build from source, or a distribution package you build from the
recipe kept beside the source.

There is deliberately no `curl | sh` installer. A tool whose job is to show
you what will happen before it happens should not start by asking you to
run something you have not read.

## Release archive (any Linux, x86_64 or aarch64)

Each release publishes one archive per architecture and a `SHA256SUMS`
file. The binaries are linked statically against musl: they need no shared
library and run on any Linux distribution.

```console
$ V=0.1.0; A=$(uname -m)
$ curl -fsSLO https://github.com/ezechiel203/yapacc/releases/download/v$V/yapacc-$V-$A-linux-musl.tar.gz
$ curl -fsSLO https://github.com/ezechiel203/yapacc/releases/download/v$V/SHA256SUMS
$ sha256sum -c --ignore-missing SHA256SUMS
yapacc-0.1.0-x86_64-linux-musl.tar.gz: OK
```

Unpack it into a prefix. System-wide:

```console
$ sudo tar -xzf yapacc-$V-$A-linux-musl.tar.gz --strip-components=1 -C /usr/local
```

Or for yourself only, without root:

```console
$ mkdir -p ~/.local
$ tar -xzf yapacc-$V-$A-linux-musl.tar.gz --strip-components=1 -C ~/.local
```

Make sure `~/.local/bin` is on your `PATH` in that case. Then:

```console
$ yapacc --version
$ yapacc doctor
```

### What is in the archive

| Path under the prefix | What |
|---|---|
| `bin/yapacc` | the command line |
| `lib/yapacc/yapacc-tui` | the optional interactive interface (`yapacc i`) |
| `lib/yapacc/yapacc-mcp` | the optional server for AI assistants (`yapacc mcp`) |
| `share/man/man1/yapacc.1` | the manual page |
| `share/bash-completion/`, `share/zsh/`, `share/fish/` | shell completions |
| `LICENSE`, `README.md` | the licence and the readme |

Only `bin/yapacc` is required. Delete the other two binaries if you do not
want them; `yapacc doctor` then reports them as optional components that
are not installed.

### Checking where the archive was built

The release workflow attaches a build provenance attestation to every
archive. With the GitHub command line tool:

```console
$ gh attestation verify yapacc-$V-$A-linux-musl.tar.gz --repo ezechiel203/yapacc
```

The checksum proves the file is the one that was published. The attestation
proves which workflow run, from which commit, built it.

## From source

You need Rust 1.89 or newer and a C linker.

```console
$ git clone https://github.com/ezechiel203/yapacc.git
$ cd yapacc
$ cargo build --release
$ ./target/release/yapacc doctor
```

That builds the three binaries. For the core alone, which is all the
commands in this handbook need:

```console
$ cargo build --release --no-default-features --bin yapacc
```

For a static binary you can copy to another machine:

```console
$ rustup target add x86_64-unknown-linux-musl
$ cargo build --release --no-default-features --target x86_64-unknown-linux-musl --bin yapacc
$ ls -lh target/x86_64-unknown-linux-musl/release/yapacc
```

### FreeBSD

Build on FreeBSD itself, with the Rust from packages:

```console
$ pkg install rust git
$ git clone https://github.com/ezechiel203/yapacc.git && cd yapacc
$ cargo build --release --no-default-features --bin yapacc
```

This was done on FreeBSD 15.1 and takes about two minutes. There is no
pre-built FreeBSD binary.

### Build features

| Feature | Default | What it adds |
|---|---|---|
| `tui` | on | builds `yapacc-tui` |
| `alpm` | off | links libalpm on Arch for transaction analysis; see the note below |

> [!NOTE]
> The `alpm` feature uses a Rust crate published under GPL-3.0, while
> yapacc is GPL-2.0-only. It is there for local builds on Arch. No release
> archive or packaging recipe enables it, and everything in this handbook
> works without it.

## Distribution packages

Recipes for five distributions live in
[`contrib/packaging`](../../contrib/packaging/README.md). They are not in
any distribution's repository yet; you build the package yourself and
install it with your package manager, which then owns the files.

| Distribution | Recipe | Build |
|---|---|---|
| Arch Linux | `arch/PKGBUILD` | `makepkg -si` |
| Debian, Ubuntu | `debian/` | `dpkg-buildpackage -us -uc -b` |
| Fedora and relatives | `rpm/yapacc.spec` | `rpmbuild -ba` |
| Alpine Linux | `alpine/APKBUILD` | `abuild -r` |
| Void Linux | `void/template` | `./xbps-src pkg yapacc` |

The packaging README has the details for each, including which Rust
version the distribution needs.

## After installing

Nothing has to be configured. yapacc works with no configuration file, and
the read commands need no setup at all. Start with the
[quick start](quick-start.md).

Shell completions are installed by the archive and the packages. For a
source build:

```console
$ yapacc completions bash > ~/.local/share/bash-completion/completions/yapacc
```

## Uninstall

yapacc installs no service, no hook and no timer, so removing it is
removing its files.

- **Archive:** delete `bin/yapacc`, `lib/yapacc/`, the manual page and the
  three completion files from the prefix you unpacked into.
- **Package:** remove it with your package manager.

Files yapacc may have created under your home directory, all safe to
delete:

| Path | What |
|---|---|
| `~/.config/yapacc/` | your configuration file, if you wrote one; yapacc never writes it |
| `~/.cache/yapacc/` | build caches, and downloads of an `undo` while it runs |
| `~/.local/state/yapacc/` | the ledger of installs made *outside* the package manager |
| `~/.local/share/yapacc/` | AppImages, recipes and isolated installs, if you used those commands |

Native packages installed or upgraded through yapacc belong to your package
manager and are not affected.
