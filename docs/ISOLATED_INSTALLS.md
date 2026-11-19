# Isolated installs, source parsing, and conflict-safe binary trees

> **Second pass, 2026-11-15.** This is the design note for the keg lane
> (target binary `yapacc-keg`). Status against the code:
>
> - *Built:* extraction of deb, rpm, snap and Arch packages into managed
>   prefixes; attributed bin names (`fgn.<format>.<pkg>--<bin>` — the
>   `pkgc-…` examples below predate the rename); exposure modes `attributed`
>   and `none`; ledger records; APT and RPM source parsing; GPG verification
>   of remote repositories; rollback of a failed multi-package transaction.
> - *Not built:* dependency closure, environment activation, interactive
>   conflict prompts, `original` exposure.
> - *Changed by the review:* extraction no longer builds any shell command
>   (the RPM fallback is piped directly with `--no-absolute-filenames`;
>   archive member names are validated).
> - *Scope decision:* extraction is kept for **inspection and for
>   self-contained binaries**. It is not presented as a way to run another
>   distribution's software: a payload extracted without its dependencies
>   and without its maintainer scripts usually does not run, and making it
>   run is the parked dependency store (DESIGN_RESOLUTION §3). The supported
>   answer for foreign-distribution software is a container.
> - *Uninstall test:* attributed launchers are plain symlinks or wrappers
>   that do not call yapacc, so removing yapacc leaves them working; a keg is
>   removed with `rm -r` of its directory. This property must be kept.

## What is verified (2026-11-19)

This is the state after the pre-release review of the write path. The
security model with its limits is `docs/SECURITY.md` section 6; this is the
user's view.

| You run | Signature checked | Each package checked against |
|---|---|---|
| `install tx-apt --keyring K`, `install tx-apt-sources --keyring K` | `InRelease` (or `Release` + `Release.gpg`) | the SHA256/SHA512 and size in the signed `Packages` index |
| `install tx-rpm --keyring K` | `repomd.xml.asc` | the checksum and size in the verified `primary.xml` |
| the same with `--allow-unsigned` instead of a keyring | nothing | the hash in the unverified index (catches a damaged download, not a hostile server) |
| `install tx-apt-repo`, `install tx-rpm-repo` | nothing: a bare index has no signature | the hash in that unverified index |
| `install tx`, `install foreign` | nothing | nothing |
| `install appimage --sha256 HEX` | nothing | the SHA-256 you gave |
| `install appimage` | nothing | nothing |

Each run prints a `Verified:` or `NOT verified:` line saying which of these
applied, and a `check:` line per package.

- `--keyring` takes an OpenPGP keyring file that `gpgv` can read (binary,
  not ASCII-armored), for example
  `/usr/share/keyrings/debian-archive-keyring.gpg`. A relative path is
  resolved from the current directory.
- A signature that does not verify, an index or a package that does not
  match its hash, and text added around the signed part of an `InRelease`
  stop the install with **exit 5**. `--allow-unsigned` does not change that:
  it only accepts a repository that has no signature at all.
- A remote repository without `--keyring` or `--allow-unsigned`, a remote
  index given to `tx-apt-repo` / `tx-rpm-repo` without `--allow-unsigned`, a
  repository that only offers MD5 or SHA1, and a plain `http://` AppImage
  without `--allow-http` are refused with **exit 3**. So is a non-free
  package (deb `Section` non-free/contrib/restricted/multiverse, RPM
  `License` saying nonfree, proprietary or commercial) without `--non-free`
  or a yes at the terminal; `--yes` does not accept a licence.
- A dry run of the index verbs downloads no package: the plan comes from the
  index. A real run downloads each package once, after you consent, into a
  private temporary directory that is deleted afterwards. `install tx` with
  URLs downloads them once, before the plan, and installs those same files.
- Extraction never keeps the archive's owners, set-user-ID or set-group-ID
  bits, or device nodes, also as root, and never follows a symbolic link
  that a package ships.
- Remove a keg with `yapacc remove foreign --id ID --yes` (the id is in
  `yapacc foreign list`), also a keg built by `get --from recipe`. The
  directory is deleted only if it is one keg: never the foreign root or a
  directory holding other kegs.
- Compressed indexes need the matching tool: `gzip`, `xz` or `zstd`
  (current Fedora metadata is zstd). The lane also needs `curl` or `wget`,
  `gpgv` for `--keyring`, `ar` and `bsdtar` for deb, `bsdtar` or `rpm2cpio`
  and `cpio` for rpm.

Not verified by anything here: that the keyring is the publisher's, how old
the repository state is (`Valid-Until` is not checked), and what the
software does when you run it.

## Goals (user-requested)
- Read foreign distro package source definitions: apt `sources.list` and `sources.d/*` (`.list`, deb822 `.sources`), RPM repo definitions (`.repo` in yum/dnf repos.d), and analogous sources for other package types where applicable.
- Install packages **and all their dependencies** into a specific directory that is **not** `/bin`, `/usr`, or a standard home user directory, separate from the machine’s original packages.
- Resolve filename/name conflicts with user choice: if filenames conflict between installs, ask the user to either (a) not add the new install to `PATH`, or (b) rename the new install. In non-interactive/scripted mode (`--noconfirm`/`--script`), fail fast with an actionable message instead of prompting.
- **New names shall always be attributed to packages that are foreign to the distro** (foreign-origin binaries must be namespaced/attributed).

## Design (brainstorm, staged)

### 1) Isolated install roots (separate binary trees)
- **Managed foreign root** (default, configurable): e.g. `/var/lib/yapacc/foreign-root/<format>/<distro-or-origin>/<pkg>-<ver>-<arch>/...`
  - Never under `/bin`, `/usr`, `/sbin`, `/lib*` of host; never in standard home paths like `~/.local/bin` by default.
  - Per-install immutable-ish prefix: `rootfs/` (extracted/installed payload), `meta/` (manifest, origin, dep graph), `bin/` (exposed entrypoints), `lib/` overlays/wrappers as needed.
- **Dependency closure**: for foreign distro installs, resolve and install the full dependency set into the same isolated transaction prefix/tree (or a shared content-addressed store with per-env views) so host packages are not mutated. (Full cross-distro dependency resolution is non-trivial; staged implementation: manifest+resolution plan first, mediated/containerized execution where needed.)
- **Environment isolation**: prefer `PATH`/`LD_LIBRARY_PATH`-scoped wrappers, chroot/proot/bubblewrap/container views, or per-env activation scripts rather than global system mutation.

### 2) Naming and conflict avoidance policy
- **Foreign-attributed naming (mandatory for foreign-to-distro packages)**: every exposed executable gets a stable attributed name, e.g.
  - `pkgc-<origin>-<format>-<pkg>--<bin>` or `<bin>+<pkg>@<origin>.<format>` (exact scheme configurable), recorded in ledger/manifest.
  - Original basename is preserved inside the isolated prefix (`rootfs/.../bin/<bin>`), but PATH exposure uses attributed names by default.
- **PATH exposure modes** (per install/env):
  - `none`: no PATH exposure (use absolute paths or env activation).
  - `attributed` (default for foreign): only attributed names exposed in a managed bin dir for that isolated root/env.
  - `original` (opt-in): expose original basenames only when explicitly requested and conflict-free, or after explicit resolution.
- **Filename conflict resolution** (when an unprefixed/original exposure is requested or when two installs would claim same exposed name):
  - Detect collisions against host PATH, managed exposure dirs, and ledger-tracked exposures.
  - **Interactive**: prompt user — (1) do not add the new install to PATH, or (2) rename the new install’s exposed name (foreign-attributed rename recorded). Never silently overwrite.
  - **Non-interactive (`--noconfirm`/`--script`)**: fail fast with conflict details and required choices/flags (e.g. `--expose=none|attributed`, `--rename-bin <old>=<new>`).
- **Collision-avoidance strategies** (to minimize conflicts):
  - Per-package/per-env private `bin/` dirs + activation (env modules / `yapacc env activate <env>`), not a single global bin.
  - Content-addressed or versioned prefixes; multiple versions co-installable side-by-side.
  - Hard separation of foreign-origin namespaces from host distro packages.
  - Wrapper scripts that set library/search paths into the isolated prefix (avoids soname/loader collisions).
  - Prefer sandboxed views (bwrap/proot/chroot/distrobox/containers) for complex dependency graphs to avoid host ABI mixing.

### 3) Source definition parsing (read support)
- **APT/Debian**: parse classic one-line `sources.list` / `sources.d/*.list` (`deb [opts] uri suite components...`) and deb822 `sources.d/*.sources` stanzas (`Types:`, `URIs:`, `Suites:`, `Components:`).
- **RPM (yum/dnf)**: parse `.repo` INI files (e.g. `/etc/yum.repos.d/*.repo`) with `[id]`, `name`, `baseurl`/`mirrorlist`/`metalink`, `enabled`, `gpgcheck`.
- **Arch/other**: parse `pacman.conf` repos, and other formats (flatpak remotes, snap channels/stores, appimage catalogs) via adapters.
- Output: normalized repo/source records (id, type, uris, suites/components/arches/options, enabled, origin path) for inspection, planning, and later mediated install resolution.

### 4) Implementation status (staged)
- **Implemented now (this iteration)**: read-only parsing/reporting for apt `sources.list`/`sources.d` and rpm `.repo` definitions via CLI; foundational design documented.
- **Partially implemented (existing)**: mediated foreign extract installs to managed prefixes with manifest/ledger tracking and bin symlinks (user-scope defaults; needs migration to isolated non-home roots and mandatory foreign-attributed naming/exposure policies).
- **Not yet implemented**: full dependency closure resolution/fetch/install from parsed foreign repos, sandboxed runtime env activation, interactive TUI conflict prompts, and seamless native integration across all formats.
