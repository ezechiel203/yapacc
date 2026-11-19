#!/bin/sh
# void-vm.sh - boot / use / remove the Void test image built by void-image.sh.
#
#   void-vm.sh up | ssh [-t] -- CMD... | scp SRC DST (remote is written vm:/path) | clean
#
# Direct kernel boot of a qcow2 overlay on images/void.raw, user-mode
# networking, ssh on 127.0.0.1:$YAPACC_VM_PORT_BASE+9 (same numbering as
# vm.sh list). Same DNS workaround as vm.sh (see README.md).
set -eu
ROOT=${YAPACC_VMS:-/tmp/yapacc-vms}
IMG=$ROOT/images
RUN=$ROOT/run/void
PORT=$(( ${YAPACC_VM_PORT_BASE:-2200} + 9 ))
MEM=${YAPACC_VM_MEM:-2048}
SSHO="-i $IMG/void.key -p $PORT -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -o LogLevel=ERROR -o ConnectTimeout=10"
die() { echo "void-vm.sh: $*" >&2; exit 1; }

up() {
    [ -s "$IMG/void.raw" ] || die "no image: run void-image.sh first"
    mkdir -p "$RUN"
    if [ -f "$RUN/pid" ] && kill -0 "$(cat "$RUN/pid")" 2>/dev/null; then die "already running"; fi
    qemu-img create -q -f qcow2 -b "$IMG/void.raw" -F raw "$RUN/disk.qcow2" 12G
    dns=${YAPACC_VM_DNS:-1.1.1.1}
    echo "nameserver $dns" > "$RUN/resolv.conf"
    unshare -rm sh -c "mount --bind '$RUN/resolv.conf' /etc/resolv.conf && exec qemu-system-x86_64 \
        -name yapacc-void -machine accel=kvm -cpu host -smp 2 -m $MEM -display none \
        -serial file:$RUN/console.log -daemonize -pidfile $RUN/pid \
        -kernel $IMG/void.vmlinuz -initrd $IMG/void.initrd \
        -append 'root=/dev/vda rw console=ttyS0 net.ifnames=0 ${YAPACC_VOID_APPEND:-}' \
        -drive file=$RUN/disk.qcow2,if=virtio,format=qcow2 \
        -netdev user,id=n0,hostfwd=tcp:127.0.0.1:$PORT-:22 -device virtio-net-pci,netdev=n0 \
        -object rng-random,id=rng0,filename=/dev/urandom -device virtio-rng-pci,rng=rng0"
    i=0
    while [ $i -lt 120 ]; do
        # shellcheck disable=SC2086
        if ssh $SSHO tester@127.0.0.1 'sudo -n true' 2>/dev/null; then
            echo "void: up on 127.0.0.1:$PORT (user tester, NOPASSWD sudo)"; return 0
        fi
        i=$((i + 1)); sleep 3
    done
    die "ssh did not come up; see $RUN/console.log"
}

case ${1:-} in
up) up ;;
ssh) shift; t=; [ "${1:-}" = -t ] && { t=-t; shift; }; [ "${1:-}" = -- ] && shift
     # shellcheck disable=SC2086
     exec ssh $t $SSHO tester@127.0.0.1 "$@" ;;
scp) # shellcheck disable=SC2086
     src=$2 dst=$3; case $dst in vm:*) dst=tester@127.0.0.1:${dst#vm:} ;; esac
     exec scp -q -i "$IMG/void.key" -P "$PORT" -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -o LogLevel=ERROR "$src" "$dst" ;;
clean) if [ -f "$RUN/pid" ]; then
           # shellcheck disable=SC2086
           ssh $SSHO tester@127.0.0.1 'sudo poweroff' 2>/dev/null || true
           sleep 3; kill "$(cat "$RUN/pid")" 2>/dev/null || true
       fi
       rm -rf "$RUN"; echo "void: stopped, overlay removed" ;;
*) die "usage: void-vm.sh up|ssh|scp|clean" ;;
esac
