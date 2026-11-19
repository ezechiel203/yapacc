# Real-VM test harness

`vm.sh` boots public cloud images under QEMU/KVM with cloud-init so yapacc can
be tested on real distros and BSDs. Everything lives under `$YAPACC_VMS`
(default `/tmp/yapacc-vms`), never in the repo: `images/` (cached, checksum
verified, decompressed base images), `run/NAME/` (overlay disk, seed ISO,
throwaway ssh key, pid, `console.log`).

Needs: `qemu-system-x86_64`, `qemu-img`, `cloud-localds`, `curl`, `xz`, `ssh`,
KVM; `edk2-ovmf` only for `--uefi`.

## Usage

```
vm.sh fetch NAME                       # download + verify checksum, cache
vm.sh up NAME [--port N] [--mem MiB] [--cpus N] [--disk-gb N] [--extra-disk-gb N] [--uefi]
vm.sh ssh NAME [-t|-tt] -- CMD...   # -t/-tt allocate a pty (prompt tests)
vm.sh scp NAME SRC DST                 # remote side is written vm:/path
vm.sh down NAME                        # poweroff, then kill if needed
vm.sh clean NAME                       # down + delete overlay, seed, key
vm.sh list                             # names and default ssh ports
```

* `up` makes a qcow2 overlay on the cached base (default 20 GiB, 4096 MiB,
  2 vCPUs; mem and cpus are capped at 4096 / 4), generates an ed25519 key,
  seeds a user `tester` with NOPASSWD sudo, boots headless with KVM and waits
  until ssh **and** `sudo -n true` work (`YAPACC_VM_BOOT_TIMEOUT`, default 420 s).
* Networking is user-mode with `hostfwd` to 127.0.0.1 only. Default port is
  `YAPACC_VM_PORT_BASE` (default 2200) plus the image's index in `vm.sh list`,
  so lanes only need a different `YAPACC_VM_PORT_BASE`.
* Guest DNS: slirp forwards to the *first* nameserver in the host's
  `/etc/resolv.conf`; a dead loopback resolver there leaves guests without DNS
  (FreeBSD then hangs at boot in `pkg`). `up` therefore starts qemu in a private
  mount namespace (`unshare -rm`) whose `/etc/resolv.conf` is `1.1.1.1`
  (`YAPACC_VM_DNS=addr` to change, `=host` to disable).
* Seed content contains only a generated key labelled `yapacc-vm` and the
  hostname `yapacc-NAME`; nothing about the host user.
* Always `vm.sh clean NAME` when done. Max 2 VMs at once per lane.

## Images

All were booted through the harness (ssh + NOPASSWD sudo + DNS verified) on
2026-11-17 unless noted. Booted is not tested: `ubuntu2604`, for one, was
booted and yapacc was never run on it. What ran on which system is in
[docs/VERIFICATION.md](../../../docs/VERIFICATION.md). Default boot is BIOS;
`--uefi` was verified on debian13 (via `/sys/firmware/efi`) and is expected
to work for every image except where noted.

| NAME | Image | Checksum | Notes |
|---|---|---|---|
| `debian13` | `cloud.debian.org/images/cloud/trixie/latest/debian-13-generic-amd64.qcow2` | SHA512SUMS | Image user `debian`. BIOS or UEFI. Boots in ~15 s. |
| `ubuntu2404` | `cloud-images.ubuntu.com/noble/current/noble-server-cloudimg-amd64.img` | SHA256SUMS | Image user `ubuntu`. BIOS or UEFI. |
| `ubuntu2604` | `.../resolute/current/resolute-server-cloudimg-amd64.img` (26.04 LTS "resolute"; `stonking` is the next dev series) | SHA256SUMS | Image user `ubuntu`. |
| `fedora` | Fedora 45 `Cloud-Base-Generic-45-*.x86_64.qcow2` (newest matching file in the 45 images index; the version is pinned in `lookup()`, bump when 46 is GA) | `Fedora-Cloud-45-*-CHECKSUM` (clearsigned, parsed by name) | Image user `fedora`. BIOS or UEFI (`Generic`; the `UEFI-UKI` variant is UEFI-only and not used). dnf5. |
| `rocky9` | `dl.rockylinux.org/pub/rocky/9/images/x86_64/Rocky-9-GenericCloud-Base.latest.x86_64.qcow2` | `.CHECKSUM` | Image user `rocky`. dnf 4. Rocky 9.9. |
| `opensuse-tumbleweed` | `download.opensuse.org/tumbleweed/appliances/openSUSE-Tumbleweed-Minimal-VM.x86_64-Cloud.qcow2` | `.sha256` | Image user `opensuse` (cloud-init). `Minimal-VM` + `Cloud` is the cloud-init flavour; the `kvm-and-xen` ones use ignition/combustion. Rolling: cached copy is a snapshot. |
| `opensuse-leap` | `download.opensuse.org/distribution/leap/16.1/appliances/Leap-16.1-Minimal-VM.x86_64-Cloud.qcow2` | `.sha256` | Same flavour as above. Leap 15.6 also has a `Minimal-VM...-Cloud.qcow2` if the older zypper is wanted. |
| `arch` | `geo.mirror.pkgbuild.com/images/latest/Arch-Linux-x86_64-cloudimg.qcow2` | `.SHA256` | Image user `arch`. Use `cloudimg` (cloud-init), not `basic`. Rolling snapshot. |
| `alpine` | `dl-cdn.alpinelinux.org/alpine/latest-stable/releases/cloud/generic_alpine-*-x86_64-bios-cloudinit-r0.qcow2` (newest 3.24.x resolved from the index) | `.sha512` | **Real cloud-init, not tiny-cloud** (the `-cloudinit-` images; tiny-cloud is the `-tiny-` variant). Image user `alpine`. BIOS image; the `uefi-cloudinit` file exists if `--uefi` is wanted (change `lookup()`). Quirks, handled by `vm.sh`: a `!`-locked account cannot ssh (seed sets `lock_passwd: false` + `passwd: '*'`), no sudo in the image (seed installs it with apk). musl + busybox + OpenRC. |
| `void` | none published; built locally from `ghcr.io/void-linux/void-glibc` | - | Void publishes no cloud-init or ready-made VM image (only live ISOs and ROOTFS tarballs), so `vm.sh fetch void` and `vm.sh up void` only exit with an explanation. Use the two scripts next to `vm.sh` instead, see "Void" below. glibc, xbps, runit. |
| `freebsd14` | `download.freebsd.org/releases/VM-IMAGES/14.5-RELEASE/amd64/Latest/FreeBSD-14.5-RELEASE-amd64-BASIC-CLOUDINIT-ufs.qcow2.xz` | `CHECKSUM.SHA256` | Image user `freebsd`. BIOS (the image is GPT + efiesp, UEFI also works). UFS (the `-zfs` file exists next to it). |
| `freebsd14-zfs` | same, `-zfs.qcow2.xz` | `CHECKSUM.SHA256` | ZFS root (`zroot/ROOT/default`). First boot was slower than the 420 s `up` timeout with 2 GiB RAM; the VM still came up afterwards. `--extra-disk-gb N` adds a second virtio disk (`extra.qcow2`). |
| `freebsd15` | same, `15.1-RELEASE` | `CHECKSUM.SHA256` | Extra, for pkg(8) on 15. |
| `freebsd15-zfs` | same, `15.1-RELEASE` `-zfs.qcow2.xz` | `CHECKSUM.SHA256` | ZFS root (`zroot/ROOT/default`), pkgbase-style `FreeBSD-*` packages. Booted with 4 vCPUs, 4 GiB, 30 GiB disk through the harness (`up` finished inside 900 s). |

### Void (built locally, not through `vm.sh`)

```
void-image.sh                          # build the image once
void-vm.sh up                          # boot it, wait for ssh + sudo
void-vm.sh ssh [-t] -- CMD...
void-vm.sh scp SRC DST                 # remote side is written vm:/path
void-vm.sh clean                       # poweroff, delete the overlay
```

* `void-image.sh` needs rootless `podman`, `mke2fs`, `ssh-keygen` and the
  network. Inside a `void-glibc` container it installs `base-system`, a
  kernel, openssh, sudo and dhcpcd, builds an initramfs with dracut, creates
  `tester` with NOPASSWD sudo and a throwaway ssh key, and exports the
  filesystem into a 6 GiB ext4 image with `mke2fs -d` (no loop device, no
  root). Output under `$YAPACC_VMS/images/`: `void.raw`, `void.vmlinuz`,
  `void.initrd`, `void.key`. It does nothing when `void.raw` already exists.
* `void-vm.sh up` boots a qcow2 overlay on `void.raw` by direct kernel boot
  (`-kernel`/`-initrd`, so the image has no bootloader), BIOS only, 2 vCPUs,
  `YAPACC_VM_MEM` MiB (default 2048), ssh on 127.0.0.1 port
  `YAPACC_VM_PORT_BASE` + 9, with the same DNS workaround as `vm.sh`.
* There is no `down`: `clean` powers the VM off and removes the overlay. The
  built image stays in `images/`.
* The image was built and booted on 2026-11-17 and yapacc was run on it
  (see VERIFICATION.md). For xbps tests that need no boot, the
  `ghcr.io/void-linux/void-glibc` container image is the cheap alternative.

### FreeBSD quirks (nuageinit, not cloud-init)

The `BASIC-CLOUDINIT` images use FreeBSD's own `nuageinit(7)`: it reads the
`cidata` seed, supports a subset of `#cloud-config` (users, packages,
write_files, runcmd...) and does **not** create intermediate directories for
`write_files`. The image has no sudo, so the seed uses `packages: [sudo]` plus
`users[].sudo`. nuageinit processes `packages` before sshd starts, so:

* first boot takes about 2 minutes and the image patches itself with
  `freebsd-update` (the guest reports `14.5-RELEASE-p2`);
* it needs working guest DNS (see the resolv.conf note above); with a broken
  resolver pkg hangs forever and sshd never starts.

Packages: `sudo -n pkg ...`; shell for `tester` is `/bin/sh`.

### Other cloud-init notes

* The seed is a `cidata` ISO attached as a CD-ROM; all Linux images find it
  through the NoCloud datasource.
* Overlays are qcow2 with the cached image as read-only backing file, so many
  VMs of one image are cheap. `clean` deletes only the overlay.
* Console output goes to `run/NAME/console.log` (check it first when `up`
  times out).

`YAPACC_VM_EXTRA_DISK_GB=N vm.sh up NAME` also attaches an empty N GiB virtio
disk (`extra.qcow2` in the VM directory, removed by `clean`), e.g. for a btrfs
volume where the image's root is XFS (openSUSE cloud images).
