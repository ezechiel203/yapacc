# yapacc security model

Status: 2026-11-15. One section per protection: what it does, what it does
not do, and where it is tested. If a sentence here cannot be backed by a
test or a line of code, it is a bug in this document.

## Who we defend against

| # | Adversary | In scope |
|---|---|---|
| A1 | A hostile build recipe (PKGBUILD from the AUR or generated) | yes |
| A2 | Hostile text reaching a command line: build output, adapter answers, MCP arguments, archive member names | yes |
| A3 | An AI assistant that is wrong, manipulated or over-eager | yes |
| A4 | A compromised or misbehaving community adapter | partly |
| A5 | Someone with the user's own file access rewriting yapacc's records | detect only, and only with a witness |
| A6 | Root on the machine | no; nothing on a machine defends against its own root |
| A7 | A compromised distribution repository or signing key | no; that is the distribution's trust root |

## 1. Build sandbox (`src/sandbox.rs`)

`makepkg` sources the PKGBUILD as bash, so every build step — including
printing `.SRCINFO` and regenerating checksums — is code execution. All of
them run through the same confinement.

What a build gets:

- the host filesystem read-only;
- one writable directory of the host, the build directory;
- a throwaway `$HOME` at `/tmp/.yapacc-home`, shared by the two phases of
  one build and deleted afterwards;
- a cleared environment (no `SSH_AUTH_SOCK`, no tokens);
- its own pid, ipc, uts, cgroup and user namespaces; no controlling terminal;
  killed when yapacc exits;
- network only during the fetch phase. Builds run as `makepkg --nobuild`
  (download, verify, extract, `prepare()`) with network, then
  `makepkg --noextract` (`build()`, `package()`) without. `--build-net`
  shares the network for the whole build and says so.

Hidden (replaced by an empty tmpfs inside the sandbox):

| Path | What is there on a desktop |
|---|---|
| `/home`, `/root`, the invoking user's home if it is elsewhere | personal files, ssh and gpg keys, browser profiles |
| `/run`, all of it (and `/var/run` where that is a directory of its own) | `/run/user/UID` (session bus, ssh-agent and gpg-agent sockets), `/run/dbus` (system bus), `/run/media` (removable disks mounted by udisks2), `docker.sock`, podman and containerd sockets, every other daemon socket |
| `/mnt`, `/media` | manually mounted and removable disks |
| `/tmp`, `/var/tmp` | other programs' temporary files, the X11 socket directory |

One thing under `/run` comes back, read-only, and only while the network is
shared: the file `/etc/resolv.conf` links to, when it links into `/run`
(`/run/systemd/resolve/stub-resolv.conf` with systemd-resolved,
`/run/NetworkManager/resolv.conf`, `/run/resolvconf/resolv.conf`). Without it
the fetch phase could not resolve names. The systemd-resolved and nscd
sockets are not brought back; glibc falls back to the `dns` source.

Not hidden:

- every other file the invoking user can read: `/etc`, `/usr`, `/var`
  (except `/var/tmp`), `/opt`, `/srv`, `/boot`, and any filesystem mounted
  somewhere else (`/data`, a second disk under `/srv`...). Keep personal
  data out of such places or make it unreadable to the build user;
- while the network is shared (fetch phase, or the whole build with
  `--build-net`): everything listening on localhost or on an abstract unix
  socket — a local database, a container engine's TCP port, an X server
  that accepts abstract-socket connections — and the local network;
- the build directory's own contents, which the fetch phase can upload;
- CPU, memory and disk: there is no limit.

The throwaway home is not inside the build directory. The build directory
is untrusted input (a cloned repository can contain symbolic links), so
yapacc writes nothing through it before bubblewrap starts. The home is a new
mode-0700 directory with a 128-bit random name, created non-recursively
under `~/.cache/yapacc/sandbox-homes` (`$XDG_CACHE_HOME`; `$XDG_RUNTIME_DIR`
as a fallback), and only where that directory and its parent belong to the
user and are not writable by anyone else. It is seeded with the user's
*public* GnuPG keyring (`pubring.kbx`, `pubring.gpg`, `trustdb.gpg`) so
`validpgpkeys` checks work; files are created with `O_CREAT|O_EXCL`, which
never follows a link. Homes left by a killed run are removed by the next
build. A directory named `.yapacc-home` inside a build directory is ignored.

Policy: `[build] sandbox = "require"` is the default — no working bubblewrap
means no build, with an explanation. `prefer` warns and builds unconfined;
`off` and `--no-sandbox` are explicit. Root must name `--build-user`; the
build then runs as that user *inside* the same sandbox. In that case the
home is created by root under `/var/cache/yapacc/sandbox-homes`, handed to
the build user, and not seeded: root copies nothing into a directory another
user controls, and the build user gets no keyring.

Does not protect against: kernel or bubblewrap escapes; what "Not hidden"
lists above. There is no seccomp filter yet (the `rua` helper has one;
adopting it is on the roadmap). A recipe that downloads in `build()` fails
until the user passes `--build-net`. On a host whose `nsswitch.conf` offers
no `dns` or `files` source after `resolve`, names do not resolve in the
fetch phase.

Tested: `tests/build_sandbox.rs` builds a deliberately hostile PKGBUILD and
asserts it cannot read a canary file in the real home, sees no inherited
secret, cannot write outside its directory, finds nothing under `/run`
(`/run/user`, `/run/media`, `/run/dbus`, container sockets), has no network
interface in `build()`, keeps `$HOME` from `prepare()` to `build()`, and
still produces a package through fakeroot. The same file checks that names
resolve in the fetch phase and that links planted as
`.yapacc-home/.gnupg` and `.yapacc-home/.gnupg/pubring.kbx` in a build
directory leave their targets untouched. `src/sandbox.rs` tests the home
(mode, public files only, refused where others can write, removed after
use), the `/etc/resolv.conf` link handling, and the sweep of stale homes.

Not tested on a real machine: the root with `--build-user` path, and a host
where `/etc/resolv.conf` links into `/run` (the handling of that link is
tested with a file placed under the real `/run`).

## 2. Consent for inferred dependencies (`src/repair.rs`)

When a build fails, the classifier may conclude packages are missing. Their
names come from the output of a recipe the user reviewed, not from a plan
the user approved. Therefore: names are validated, checked against the
official repositories, shown, and installed only on a yes at the terminal or
with `--repair-deps` given up front. Non-interactive runs report and stop.
`--yes` on the outer command does not cover this.

## 3. Hostile text (`util::valid_pkg_name`, argument vectors)

No yapacc code path builds a shell command from data. Remaining `bash`
invocations run fixed helper scripts or a health check the user typed.
Package names from build output, adapters and MCP arguments must match the
package-name grammar (cannot start with `-`), and are passed after `--`.
Container names and image references are validated and passed as separate
arguments. RPM payloads are extracted with `bsdtar`, or `rpm2cpio | cpio
--no-absolute-filenames` wired pipe to pipe. `ar` members are taken only if
they are plain file names.

Tested: `pkg_names_are_strict`, `hostile_names_never_reach_a_package_manager`,
`option_like_names_and_unknown_lanes_are_rejected`.

Does not protect against: a native package manager's own parsing bugs; a
malicious archive that exploits `bsdtar` or `unsquashfs` themselves.

## 4. AI interface (`src/mcp.rs`)

By default an assistant can read and can *stage*, never apply. The install
tool returns the plan and the exact command for the human to run in their
own terminal, where the normal gate asks them. `confirm=true` is an
assertion made by the model and is not consent. A machine owner can opt into
direct application with `[mcp] apply = "allow"`. The server sets
`YAPACC_VIA=mcp` for such runs; the core prints "requested via mcp" before
applying such a change, but does not record it in the native log, which
records them like any other change. Tools carry `readOnlyHint` / `destructiveHint`
annotations; clients are told by the spec not to trust annotations from an
untrusted server, so they are a courtesy, not a control.

`yapacc-mcp --config PATH` (an existing regular file, checked at startup) is
forwarded to every core run as the two argv entries `--config PATH`, never
through a shell, so the core and the server read the same configuration.

Tested: `a_model_cannot_approve_its_own_install`,
`config_is_forwarded_as_two_argv_entries_before_the_command`.

Does not protect against: a human who approves without reading; an
assistant that misreports what a read tool returned.

## 5. Install ledger (`src/ledger.rs`)

What it is: an append-only journal where each event carries the SHA-256 of
its predecessor, a cache rebuilt from it, and a kernel file lock around
changes.

What the chain alone gives: detection of accidental corruption and of naive
edits. **Not tamper evidence** — anyone who can write the file can rewrite
all of it and recompute every hash. `yapacc ledger verify` says so in its
output whenever nothing independent is available to compare against.

What makes tampering detectable: a copy of the head the rewriter cannot also
rewrite.

- `[ledger] witnesses = ["/path/on/other/storage", "journald"]` — after each
  change the head is appended to each witness. `journald` uses the system
  journal, whose entries the logging user cannot edit or delete afterwards.
- `yapacc ledger head` prints `SEQ:HASH`; keep it anywhere and pass it back
  with `ledger verify --anchor`.

`verify` then fails with exit 5 and says REWRITTEN or TRUNCATED when the
journal no longer contains a witnessed head.

Crash safety: the journal append is the commit point. If the process dies
before the cache is rewritten, readers rebuild the answer in memory and the
next writer rebuilds the cache; an interrupted final line is discarded.
Pre-v2 events hashed with djb2 fail verification unless `--accept-legacy`.

Tested: `crash_between_journal_and_cache_is_recovered`,
`interrupted_append_is_discarded_not_fatal`,
`witness_detects_a_fully_recomputed_rewrite_and_truncation`,
`reads_never_create_a_ledger`.

Does not protect against: root (A6), who can rewrite the system journal too
— use a witness on another machine; loss of the witness; anything about
native packages, which the ledger does not record.

## 6. Foreign installs: repositories, package files, kegs (`src/foreign_extract.rs`, `src/appimage.rs`)

Status: 2026-11-19 (pre-release review of the write path).

What is verified, per install verb:

| Verb | Signature | Package index | Each package file |
|---|---|---|---|
| `install tx-apt`, `install tx-apt-sources` with `--keyring` | `InRelease`, or `Release` + `Release.gpg`, with `gpgv` | SHA256/SHA512 and size from the signed text | SHA256/SHA512 and size from its `Packages` stanza |
| `install tx-rpm` with `--keyring` | `repomd.xml` + `repomd.xml.asc` with `gpgv` | SHA256/SHA512 and size from `repomd.xml` | the `<checksum>` and size of its `primary.xml` entry |
| the three verbs above with `--allow-unsigned` and no keyring | none | the hash in the unverified release file | the hash in the unverified index |
| `install tx-apt-repo`, `install tx-rpm-repo` (an index given directly) | none: nothing signs a bare index | none | the hash in that unverified index |
| `install tx`, `install foreign` (files or URLs given directly) | none | none | none |
| `install appimage` | none | none | the SHA-256 given with `--sha256`; none without it |

Every one of these prints which row applied: a line starting `Verified:` or
`NOT verified:`, and per package a `check:` line with the hash or `NONE`.
A keg's ledger record keeps it as `source_check`.

The rules behind the table:

- **Only signed text is read.** An `InRelease` is verified with
  `gpgv --output` and the parser is given that output, never the file. A
  file with anything but blank lines before or after its one clearsigned
  message is refused before `gpgv` runs (`gpgv` itself accepts such a file).
- **A failed verification is fatal.** A bad signature, a key that is not in
  the keyring, an index or a package that does not match its hash: exit 5,
  whatever `--allow-unsigned` says. `--allow-unsigned` accepts a repository
  that offers no signature; it never accepts one whose signature is wrong.
- **MD5 and SHA1 are not checks.** The strongest hash listed wins (SHA512,
  then SHA256) whatever order the file lists them in. A release file that
  lists the index with MD5/SHA1 only is refused (exit 3). A package whose
  index entry has no SHA256/SHA512 is refused (exit 3) unless
  `--allow-unsigned` is given, and is then listed as `NOT checked at all`.
- **A package is downloaded once**, after consent, into a private temporary
  directory (random name from `/dev/urandom`, mode 0700, created
  non-recursively, an existing path is an error), compared with its index
  hash before anything reads it, and deleted on a mismatch. The plan of the
  index verbs is made from the index and downloads nothing.
- **The unverified-repository refusal is in the fetch layer** as well as in
  the install commands: a remote repository without `--keyring` or
  `--allow-unsigned`, and for `tx-apt-repo` / `tx-rpm-repo` a remote index
  or package location without `--allow-unsigned`, is refused (exit 3) before
  anything is fetched. A dry run is exempt: it installs nothing.
- **Extraction.** Payloads are extracted without owners or special modes
  (`bsdtar --no-same-owner --no-same-permissions`, `cpio
  --no-preserve-owner`), then every entry is given to the installing user,
  set-user-ID and set-group-ID bits are cleared and device nodes deleted,
  also when yapacc runs as root. Symbolic links in a payload are recorded,
  never followed; launchers are made only for files inside the keg.
- **Keg directories.** A package or format name becomes one path component
  (`..`, `.` and a leading dot cannot occur). Before a keg directory from the
  ledger is deleted it must be an absolute path without `.`/`..`, a real
  directory, not the foreign root or a directory holding other kegs, and
  hold the keg manifest or a `rootfs` directory.
- **Non-free.** A deb `Section` in `non-free*`, `contrib`, `restricted` or
  `multiverse`, or an RPM `License` saying nonfree, proprietary or
  commercial, needs `--non-free` or a yes at the terminal. `--yes` does not
  answer it; a no, or a run that cannot ask, exits 3.
- **AppImage.** A plain `http://` download is refused (exit 3) unless
  `--allow-http` is given, and an `https://` download may not be redirected
  to `http://`. With `--sha256` a mismatch deletes the file and exits 5.

Does not protect against:

- a keyring that is not the publisher's: yapacc verifies against the file
  it is given and has no trust root of its own (A7);
- replay of an old repository state: `Valid-Until` and `Date` in a Release
  file are not checked, so an outdated but validly signed repository is
  accepted;
- what a package or an AppImage does when it is run, and dependencies
  outside the resolved set, which are reported and not fetched;
- anything about a file given directly to `install tx`, `install foreign`,
  or to `install appimage` without `--sha256`: no signature and no hash is
  checked, and the output says so;
- `update appimage` and `update foreign`: they take no hash, so the new file
  is not verified (`update appimage` prints that; it refuses `http://`);
- a non-free package filed under a free section or with another wording:
  the gate is a match on words, not a licence analysis; the RPM `License` of
  a single file is only read when the `rpm` tool is installed;
- a ledger rewritten to name another keg (section 5), the same user, or root;
- bugs in `gpgv`, `curl`, `bsdtar`, `ar`, `cpio`, `unsquashfs` themselves.

Known gaps: the pacman-only `plan tx-*` verbs report an integrity failure
with exit 1, not 5 (they install nothing); `update external` does not
dispatch recipe kegs (`pkg-extract`) yet, `update foreign --id` does.

Tested (`src/foreign_extract.rs`, `src/appimage.rs`, `src/cli/install.rs`):
`a_signed_repository_resolves_with_a_hash_per_package`,
`an_unsigned_block_before_inrelease_is_refused_whatever_the_flags`,
`text_outside_the_signed_message_is_refused`,
`a_bad_signature_is_fatal_even_with_allow_unsigned`,
`a_tampered_package_is_an_integrity_error_and_is_not_installed`,
`a_tampered_packages_index_is_an_integrity_error`,
`strongest_hash_in_release_wins_whatever_the_order`,
`a_release_with_only_md5_or_sha1_is_refused`,
`a_bare_remote_index_is_refused_without_allow_unsigned`,
`unverified_remote_repo_is_refused_before_any_fetch`,
`private_dir_is_new_random_mode_0700_and_removed_on_drop`,
`extracted_files_lose_setuid_setgid_and_foreign_owners`,
`collect_files_records_a_directory_symlink_as_one_entry`,
`only_a_keg_directory_is_ever_deleted`,
`a_deb_installs_as_a_keg_without_setuid_or_followed_links_and_is_removed`,
`a_declined_or_unanswerable_licence_is_a_refusal`,
`an_integrity_failure_keeps_exit_code_5`,
`plain_http_is_refused_before_any_download_unless_allowed`,
`a_given_sha256_is_checked_and_a_mismatch_deletes_the_file`. The signature
tests use a throwaway key whose public keyring is embedded in the test and
are skipped when `gpgv` is not installed. Run for real on 2026-11-19 in
rootless podman containers (debian:stable against deb.debian.org with the
Debian archive keyring and against a tampered local copy; fedora:latest
against a local `createrepo_c` repository signed with a throwaway key).

## 7. Adapters

Adapters are separate processes that answer resolution questions as JSON and
never install. A hostile adapter can mislead the resolver; the decision
table shows which adapter answered. Not yet done: timeouts enforced by a
supervisor, sandboxing of the adapter process, signed adapter packages.
Until then, treat an installed adapter as code you run as yourself.

## 9. `undo` archives (2026-11-16)

`undo` may fetch old package files when the cache and repositories lack
them. Arch Linux Archive packages are signature-checked; Debian packages from
snapshot.debian.org are checked by hash only, and Launchpad downloads are not
verified at all. It refuses the whole undo if any version is missing, shows
the plan, and goes through the same gate as any change. Even `--dryrun` may
download files into `~/.cache/yapacc/undo`. `names` sends the package name to
repology.org.

## 8. What changed on 2026-11-15

The first review found five places where the documentation claimed more than
the code did. All five are fixed and tested: the sandbox that left the home
directory readable and the network open; autorepair installing packages
without asking; the ledger described as tamper-evident; a shell-interpolated
RPM extraction; an MCP confirmation the model could give itself. A sixth
(shell-interpolated container creation) was found while fixing the others.
