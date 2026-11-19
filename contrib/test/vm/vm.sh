#!/bin/sh
# vm.sh - tiny harness to boot public cloud images under QEMU/KVM with
# cloud-init, for yapacc integration tests. See README.md.
#
# All state lives under $YAPACC_VMS (default /tmp/yapacc-vms), never in the
# repository:
#   images/NAME.qcow2   cached, verified, decompressed base image (read-only)
#   run/NAME/           per-VM overlay disk, seed ISO, ssh key, pid, log
set -eu

ROOT=${YAPACC_VMS:-/tmp/yapacc-vms}
IMG_DIR=$ROOT/images
RUN_DIR=$ROOT/run
PORT_BASE=${YAPACC_VM_PORT_BASE:-2200}
VM_USER=tester
BOOT_TIMEOUT=${YAPACC_VM_BOOT_TIMEOUT:-420}

NAMES="debian13 ubuntu2404 ubuntu2604 fedora rocky9 opensuse-tumbleweed opensuse-leap arch alpine void freebsd14 freebsd14-zfs freebsd15 freebsd15-zfs"

die() { echo "vm.sh: $*" >&2; exit 1; }

# ---------------------------------------------------------------------------
# Image table. Sets: URL CK COMP UEFI KIND
#   URL  full URL, or "DIR/|REGEX" (newest href in the index matching REGEX)
#   CK   checksum file: full URL, "DIR/|REGEX", or "+SUFFIX" (URL + SUFFIX)
#   COMP none | xz
#   UEFI default for --uefi (0/1)
#   KIND cloud-init flavour: ci (cloud-init) | alpine | nuage (FreeBSD nuageinit)
# ---------------------------------------------------------------------------
lookup() {
    UEFI=0 COMP=none KIND=ci CK=
    case $1 in
    debian13)
        URL=https://cloud.debian.org/images/cloud/trixie/latest/debian-13-generic-amd64.qcow2
        CK=https://cloud.debian.org/images/cloud/trixie/latest/SHA512SUMS ;;
    ubuntu2404)
        URL=https://cloud-images.ubuntu.com/noble/current/noble-server-cloudimg-amd64.img
        CK=https://cloud-images.ubuntu.com/noble/current/SHA256SUMS ;;
    ubuntu2604)
        URL=https://cloud-images.ubuntu.com/resolute/current/resolute-server-cloudimg-amd64.img
        CK=https://cloud-images.ubuntu.com/resolute/current/SHA256SUMS ;;
    fedora)
        URL='https://download.fedoraproject.org/pub/fedora/linux/releases/45/Cloud/x86_64/images/|Fedora-Cloud-Base-Generic-45-[0-9.]*\.x86_64\.qcow2'
        CK='https://download.fedoraproject.org/pub/fedora/linux/releases/45/Cloud/x86_64/images/|Fedora-Cloud-45-[0-9.]*-x86_64-CHECKSUM' ;;
    rocky9)
        URL=https://dl.rockylinux.org/pub/rocky/9/images/x86_64/Rocky-9-GenericCloud-Base.latest.x86_64.qcow2
        CK=+.CHECKSUM ;;
    opensuse-tumbleweed)
        URL=https://download.opensuse.org/tumbleweed/appliances/openSUSE-Tumbleweed-Minimal-VM.x86_64-Cloud.qcow2
        CK=+.sha256 ;;
    opensuse-leap)
        URL=https://download.opensuse.org/distribution/leap/16.1/appliances/Leap-16.1-Minimal-VM.x86_64-Cloud.qcow2
        CK=+.sha256 ;;
    arch)
        URL=https://geo.mirror.pkgbuild.com/images/latest/Arch-Linux-x86_64-cloudimg.qcow2
        CK=+.SHA256 ;;
    alpine)
        KIND=alpine
        URL='https://dl-cdn.alpinelinux.org/alpine/latest-stable/releases/cloud/|generic_alpine-[0-9.]*-x86_64-bios-cloudinit-r[0-9]*\.qcow2'
        CK=+.sha512 ;;
    freebsd14)
        URL=https://download.freebsd.org/releases/VM-IMAGES/14.5-RELEASE/amd64/Latest/FreeBSD-14.5-RELEASE-amd64-BASIC-CLOUDINIT-ufs.qcow2.xz
        CK=https://download.freebsd.org/releases/VM-IMAGES/14.5-RELEASE/amd64/Latest/CHECKSUM.SHA256
        COMP=xz KIND=nuage ;;
    freebsd14-zfs)
        URL=https://download.freebsd.org/releases/VM-IMAGES/14.5-RELEASE/amd64/Latest/FreeBSD-14.5-RELEASE-amd64-BASIC-CLOUDINIT-zfs.qcow2.xz
        CK=https://download.freebsd.org/releases/VM-IMAGES/14.5-RELEASE/amd64/Latest/CHECKSUM.SHA256
        COMP=xz KIND=nuage ;;
    freebsd15)
        URL=https://download.freebsd.org/releases/VM-IMAGES/15.1-RELEASE/amd64/Latest/FreeBSD-15.1-RELEASE-amd64-BASIC-CLOUDINIT-ufs.qcow2.xz
        CK=https://download.freebsd.org/releases/VM-IMAGES/15.1-RELEASE/amd64/Latest/CHECKSUM.SHA256
        COMP=xz KIND=nuage ;;
    freebsd15-zfs)
        URL=https://download.freebsd.org/releases/VM-IMAGES/15.1-RELEASE/amd64/Latest/FreeBSD-15.1-RELEASE-amd64-BASIC-CLOUDINIT-zfs.qcow2.xz
        CK=https://download.freebsd.org/releases/VM-IMAGES/15.1-RELEASE/amd64/Latest/CHECKSUM.SHA256
        COMP=xz KIND=nuage ;;
    void)
        die "void: Voidlinux publishes no cloud-init/ready-made VM image (only live ISOs and ROOTFS tarballs); build one with void-image.sh and boot it with void-vm.sh (see README.md)" ;;
    *) die "unknown image '$1' (known: $NAMES)" ;;
    esac
}

default_port() {
    i=0
    for n in $NAMES; do
        [ "$n" = "$1" ] && { echo $((PORT_BASE + i)); return; }
        i=$((i + 1))
    done
}

# Resolve "DIR/|REGEX" to a URL by scraping the directory index.
resolve() {
    case $1 in
    *'|'*)
        dir=${1%%|*}; re=${1#*|}
        f=$(curl -fsSL --retry 3 "$dir" | grep -oE "href=\"$re\"" |
            sed 's/^href="//; s/"$//' | sort -V | tail -n 1)
        [ -n "$f" ] || die "nothing matching $re at $dir"
        echo "$dir$f" ;;
    *) echo "$1" ;;
    esac
}

sum_of() { # file algo-bits
    case $2 in
    64) sha256sum "$1" | cut -d' ' -f1 ;;
    128) sha512sum "$1" | cut -d' ' -f1 ;;
    esac
}

verify() { # file ckurl base
    f=$1 ck=$2 base=$3
    tmp=$(mktemp)
    curl -fsSL --retry 3 "$ck" -o "$tmp" || { rm -f "$tmp"; die "cannot fetch checksum $ck"; }
    want=$(grep -F "$base" "$tmp" | grep -oE '[0-9a-f]{64,128}' | head -n 1 || true)
    if [ -z "$want" ] && [ "$(grep -cE '^[0-9a-f]{64,128}( |$)' "$tmp")" = 1 ]; then
        want=$(grep -oE '^[0-9a-f]{64,128}' "$tmp")
    fi
    rm -f "$tmp"
    [ -n "$want" ] || die "no checksum for $base in $ck"
    got=$(sum_of "$f" ${#want})
    [ "$got" = "$want" ] || die "checksum MISMATCH for $base (want $want, got $got)"
    echo "checksum ok ($base)"
}

cmd_fetch() {
    name=${1:?usage: vm.sh fetch NAME}
    lookup "$name"
    mkdir -p "$IMG_DIR"
    out=$IMG_DIR/$name.qcow2
    if [ -s "$out" ]; then echo "$name: cached at $out"; return 0; fi
    url=$(resolve "$URL")
    base=${url##*/}
    echo "$name: downloading $url"
    dl=$IMG_DIR/$name.dl.part
    curl -fLsS --retry 3 -C - -o "$dl" "$url"
    if [ -n "$CK" ]; then
        case $CK in +*) ck=$url${CK#+} ;; *) ck=$(resolve "$CK") ;; esac
        verify "$dl" "$ck" "$base"
    else
        echo "$name: no published checksum, skipping verification"
    fi
    case $COMP in
    xz) xz -dc "$dl" > "$out.part" && mv "$out.part" "$out"; rm -f "$dl" ;;
    *) mv "$dl" "$out" ;;
    esac
    echo "$url" > "$IMG_DIR/$name.src"
    echo "$name: ready $out"
}

# ---------------------------------------------------------------------------
vm_dir() { echo "$RUN_DIR/$1"; }

is_running() {
    [ -f "$1/pid" ] && kill -0 "$(cat "$1/pid")" 2>/dev/null
}

ssh_opts() { # dir
    echo "-i $1/key -p $(cat "$1/port") -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -o LogLevel=ERROR -o IdentitiesOnly=yes -o BatchMode=yes -o ConnectTimeout=5"
}

write_seed() { # dir name kind
    d=$1 name=$2 kind=$3
    pub=$(cat "$d/key.pub")
    {
        echo "instance-id: yapacc-$name-$$"
        echo "local-hostname: yapacc-$name"
    } > "$d/meta-data"
    case $kind in
    nuage)
        # FreeBSD nuageinit: the image has no sudo, so it is installed through
        # `packages:` (needs working guest DNS, see the unshare wrapper in
        # cmd_up). nuageinit does this before sshd starts, so first boot is
        # slow (the image also freebsd-updates itself and reboots once).
        cat > "$d/user-data" <<UD
#cloud-config
users:
  - name: $VM_USER
    gecos: yapacc tester
    groups: wheel
    homedir: /home/$VM_USER
    shell: /bin/sh
    sudo: ALL=(ALL) NOPASSWD:ALL
    ssh_authorized_keys:
      - $pub
packages:
  - sudo
UD
        ;;
    alpine)
        # Alpine: a `!`-locked account cannot ssh in (a '*' passwd keeps it
        # unusable for password login but not locked), and
        # the image ships doas, not sudo, so install sudo via apk.
        cat > "$d/user-data" <<UD
#cloud-config
users:
  - name: $VM_USER
    sudo: ALL=(ALL) NOPASSWD:ALL
    lock_passwd: false
    passwd: '*'
    shell: /bin/sh
    ssh_authorized_keys:
      - $pub
ssh_pwauth: false
package_update: true
packages:
  - sudo
UD
        ;;
    *)
        cat > "$d/user-data" <<UD
#cloud-config
users:
  - name: $VM_USER
    sudo: ALL=(ALL) NOPASSWD:ALL
    lock_passwd: true
    ssh_authorized_keys:
      - $pub
ssh_pwauth: false
UD
        ;;
    esac
    cloud-localds "$d/seed.iso" "$d/user-data" "$d/meta-data"
}

cmd_up() {
    name=${1:?usage: vm.sh up NAME [--port N] [--mem MiB] [--cpus N] [--disk-gb N] [--extra-disk-gb N] [--uefi]}
    shift
    lookup "$name"
    port=$(default_port "$name") mem=4096 cpus=2 disk=20 extra=${YAPACC_VM_EXTRA_DISK_GB:-0}
    uefi=$UEFI
    while [ $# -gt 0 ]; do
        case $1 in
        --port) port=$2; shift 2 ;;
        --mem) mem=$2; shift 2 ;;
        --cpus) cpus=$2; shift 2 ;;
        --disk-gb) disk=$2; shift 2 ;;
        --extra-disk-gb) extra=$2; shift 2 ;;
        --uefi) uefi=1; shift ;;
        *) die "up: unknown option $1" ;;
        esac
    done
    [ "$mem" -le 4096 ] || die "up: --mem above 4096 MiB is not allowed by the lane rules"
    [ "$cpus" -le 4 ] || die "up: --cpus above 4 is not allowed by the lane rules"
    d=$(vm_dir "$name")
    if is_running "$d"; then die "$name is already running (port $(cat "$d/port"))"; fi
    cmd_fetch "$name"
    base=$IMG_DIR/$name.qcow2
    rm -rf "$d"; mkdir -p "$d"
    echo "$port" > "$d/port"
    ssh-keygen -q -t ed25519 -N '' -C yapacc-vm -f "$d/key"
    write_seed "$d" "$name" "$KIND"
    vsize=$(qemu-img info -f qcow2 --output=json "$base" | sed -n 's/.*"virtual-size": *\([0-9]*\).*/\1/p' | head -n 1)
    want=$((disk * 1073741824))
    [ "$want" -ge "$vsize" ] || want=$vsize
    qemu-img create -q -f qcow2 -F qcow2 -b "$base" "$d/disk.qcow2" "$want"
    if [ "$extra" -gt 0 ]; then
        qemu-img create -q -f qcow2 "$d/extra.qcow2" "${extra}G"
    fi
    fw=
    if [ "$uefi" = 1 ]; then
        code=/usr/share/edk2/x64/OVMF_CODE.4m.fd
        vars=/usr/share/edk2/x64/OVMF_VARS.4m.fd
        [ -r "$code" ] && [ -r "$vars" ] || die "OVMF firmware not found under /usr/share/edk2/x64"
        cp "$vars" "$d/OVMF_VARS.fd"
        fw="-drive if=pflash,format=raw,readonly=on,file=$code -drive if=pflash,format=raw,file=$d/OVMF_VARS.fd"
    fi
    # slirp forwards guest DNS to the FIRST nameserver of the host's
    # /etc/resolv.conf; if that is a dead loopback resolver, guests get no
    # DNS. Run qemu in a private mount namespace with a clean resolv.conf.
    set -- qemu-system-x86_64 -name "yapacc-$name" -machine accel=kvm -cpu host \
        -smp "$cpus" -m "$mem" -display none -serial "file:$d/console.log" \
        -daemonize -pidfile "$d/pid" \
        -drive "file=$d/disk.qcow2,if=virtio,format=qcow2" \
        -drive "file=$d/seed.iso,media=cdrom,format=raw,readonly=on" \
        -netdev "user,id=n0,hostfwd=tcp:127.0.0.1:$port-:22" \
        -device virtio-net-pci,netdev=n0 \
        -object rng-random,id=rng0,filename=/dev/urandom -device virtio-rng-pci,rng=rng0
    [ "$extra" -gt 0 ] && set -- "$@" -drive "file=$d/extra.qcow2,if=virtio,format=qcow2"
    # shellcheck disable=SC2086  # $fw is a deliberate word list
    [ -z "$fw" ] || set -- "$@" $fw
    # shellcheck disable=SC2086
    if [ "${YAPACC_VM_DNS:-1.1.1.1}" != host ] && unshare -rm true 2>/dev/null; then
        printf 'nameserver %s\n' "${YAPACC_VM_DNS:-1.1.1.1}" > "$d/resolv.conf"
        # shellcheck disable=SC2016  # expansion happens in the inner sh
        set -- unshare -rm sh -c 'mount --bind "$1" /etc/resolv.conf && shift && exec "$@"' sh "$d/resolv.conf" "$@"
    else
        echo "$name: warning: guest DNS follows host resolv.conf (no user namespaces, or YAPACC_VM_DNS=host)" >&2
    fi
    "$@"
    echo "$name: booting on 127.0.0.1:$port (console log: $d/console.log)"
    # shellcheck disable=SC2046,SC2086
    set -- $(ssh_opts "$d")
    t=0
    until ssh "$@" "$VM_USER@127.0.0.1" true 2>/dev/null; do
        is_running "$d" || die "$name: qemu exited early; see $d/console.log"
        t=$((t + 3)); [ "$t" -le "$BOOT_TIMEOUT" ] || die "$name: ssh did not answer within ${BOOT_TIMEOUT}s"
        sleep 3
    done
    # Wait for passwordless sudo to work.
    until ssh "$@" "$VM_USER@127.0.0.1" 'sudo -n true' 2>/dev/null; do
        t=$((t + 3)); [ "$t" -le "$BOOT_TIMEOUT" ] || die "$name: passwordless sudo not available within ${BOOT_TIMEOUT}s"
        sleep 3
    done
    echo "$name: up (ssh as $VM_USER, NOPASSWD sudo ok)"
}

cmd_ssh() {
    name=${1:?usage: vm.sh ssh NAME [-t] -- CMD...}
    shift
    tty=
    case ${1:-} in -t | -tt) tty=$1; shift ;; esac # -tt: pty even when stdin is a pipe (scripted answers)
    [ "${1:-}" = -- ] && shift
    d=$(vm_dir "$name")
    is_running "$d" || die "$name is not running"
    # shellcheck disable=SC2046,SC2086
    set -- $(ssh_opts "$d") $tty "$VM_USER@127.0.0.1" "$@"
    exec ssh "$@"
}

cmd_scp() {
    name=${1:?usage: vm.sh scp NAME SRC DST   (remote paths: vm:/path)}
    src=${2:?missing SRC}; dst=${3:?missing DST}
    d=$(vm_dir "$name")
    is_running "$d" || die "$name is not running"
    case $src in vm:*) src=$VM_USER@127.0.0.1:${src#vm:} ;; esac
    case $dst in vm:*) dst=$VM_USER@127.0.0.1:${dst#vm:} ;; esac
    exec scp -r -i "$d/key" -P "$(cat "$d/port")" -o StrictHostKeyChecking=no \
        -o UserKnownHostsFile=/dev/null -o LogLevel=ERROR -o IdentitiesOnly=yes \
        -o BatchMode=yes "$src" "$dst"
}

cmd_down() {
    name=${1:?usage: vm.sh down NAME}
    d=$(vm_dir "$name")
    if is_running "$d"; then
        pid=$(cat "$d/pid")
        # Ask the guest to power off cleanly, then fall back to killing qemu.
        # shellcheck disable=SC2046,SC2086
        ssh $(ssh_opts "$d") "$VM_USER@127.0.0.1" 'sudo -n poweroff || sudo -n shutdown -p now' >/dev/null 2>&1 || true
        t=0
        while kill -0 "$pid" 2>/dev/null && [ $t -lt 30 ]; do sleep 1; t=$((t + 1)); done
        kill -0 "$pid" 2>/dev/null && { kill "$pid"; sleep 1; }
        kill -0 "$pid" 2>/dev/null && kill -9 "$pid"
        echo "$name: stopped"
    else
        echo "$name: not running"
    fi
}

cmd_clean() {
    name=${1:?usage: vm.sh clean NAME}
    d=$(vm_dir "$name")
    if is_running "$d"; then cmd_down "$name"; fi
    rm -rf "$d"
    echo "$name: overlay disk, seed and key removed (cached base image kept)"
}

usage() {
    cat <<U
usage: vm.sh fetch NAME
       vm.sh up NAME [--port N] [--mem MiB] [--cpus N] [--disk-gb N] [--extra-disk-gb N] [--uefi]
       vm.sh ssh NAME [-t] -- CMD...
       vm.sh scp NAME SRC DST        (remote side written as vm:/path)
       vm.sh down NAME
       vm.sh clean NAME
       vm.sh list
images: $NAMES
env: YAPACC_VMS (default /tmp/yapacc-vms), YAPACC_VM_PORT_BASE (default 2200)
U
}

cmd=${1:-}
[ $# -gt 0 ] && shift
case $cmd in
fetch) cmd_fetch "$@" ;;
up) cmd_up "$@" ;;
ssh) cmd_ssh "$@" ;;
scp) cmd_scp "$@" ;;
down) cmd_down "$@" ;;
clean) cmd_clean "$@" ;;
list) for n in $NAMES; do echo "$n port=$(default_port "$n")"; done ;;
-h|--help|help|'') usage ;;
*) usage >&2; exit 2 ;;
esac
