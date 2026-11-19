#!/bin/sh
# void-image.sh - build a bootable Void Linux test image without root.
#
# Void publishes no cloud/VM image, so this turns the official
# ghcr.io/void-linux/void-glibc container image into one: install base-system,
# a kernel (dracut builds the initramfs), openssh, sudo, dhcpcd inside a rootless
# podman container, create user `tester` with NOPASSWD sudo and a throwaway ssh
# key, then export the container filesystem into an ext4 image with
# `mke2fs -d` (no loop device, no root). It is booted by direct kernel boot
# (`-kernel/-initrd`, see void-vm.sh), so no bootloader is needed.
#
# Output (all under $YAPACC_VMS, default /tmp/yapacc-vms):
#   images/void.raw        ext4 root filesystem (read-only base)
#   images/void.vmlinuz    kernel
#   images/void.initrd     initramfs
#   images/void.key(.pub)  ssh key of user `tester`
# Needs: podman, mke2fs, ssh-keygen, network. Run once.
set -eu
ROOT=${YAPACC_VMS:-/tmp/yapacc-vms}
IMG=$ROOT/images
W=$ROOT/void-build
IMAGE=${VOID_IMAGE:-ghcr.io/void-linux/void-glibc:latest}
mkdir -p "$IMG" "$W"
if [ -s "$IMG/void.raw" ]; then echo "void: already built ($IMG/void.raw)"; exit 0; fi
[ -f "$IMG/void.key" ] || ssh-keygen -q -t ed25519 -N '' -C yapacc-vm -f "$IMG/void.key"
cp "$IMG/void.key.pub" "$W/authorized_keys"

cat > "$W/provision.sh" <<'IN'
set -eu
xbps-install -Syu xbps
xbps-install -Syu
xbps-remove -y base-container-full 2>/dev/null || true
xbps-install -y base-system linux openssh sudo dhcpcd e2fsprogs
useradd -m -s /bin/bash -G wheel tester
mkdir -p /home/tester/.ssh
cp /w/authorized_keys /home/tester/.ssh/authorized_keys
chown -R tester:tester /home/tester/.ssh
chmod 700 /home/tester/.ssh
chmod 600 /home/tester/.ssh/authorized_keys
echo 'tester ALL=(ALL:ALL) NOPASSWD: ALL' > /etc/sudoers.d/tester
chmod 440 /etc/sudoers.d/tester
echo yapacc-void > /etc/hostname
echo '/dev/vda / ext4 rw,relatime 0 1' > /etc/fstab
for s in sshd dhcpcd; do ln -sf /etc/sv/$s /etc/runit/runsvdir/default/$s; done
xbps-reconfigure -fa
# the kernel's reconfigure hook does not build an initramfs inside a container
# the container image's xbps config has noextract patterns that strip dracut's modules
grep -l noextract /etc/xbps.d/* 2>/dev/null | xargs -r rm -f
xbps-install -fy dracut runit-void eudev kmod
kver=$(ls /lib/modules | sort -V | tail -n 1)
dracut --force --no-hostonly --add-drivers "virtio_net virtio_scsi" --force-drivers "virtio_pci virtio_blk ext4" --kver "$kver" "/boot/initramfs-$kver.img"
lsinitrd "/boot/initramfs-$kver.img" | grep -i "virtio\|ext4" | head
ls -la /boot
IN

podman rm -f yapacc-voidbuild >/dev/null 2>&1 || true
podman run --name yapacc-voidbuild -v "$W:/w:Z" "$IMAGE" sh /w/provision.sh
podman export yapacc-voidbuild -o "$W/rootfs.tar"
podman rm yapacc-voidbuild >/dev/null

rm -rf "$W/rootfs"
mkdir -p "$W/rootfs"
# unshare maps our uid to root, so ownership in the tar survives and mke2fs -d records it
podman unshare sh -c "
  set -eu
  tar -xpf '$W/rootfs.tar' -C '$W/rootfs' --exclude='./dev/*'
  cp \$(ls '$W'/rootfs/boot/vmlinuz-* | sort -V | tail -n 1) '$IMG/void.vmlinuz'
  cp \$(ls '$W'/rootfs/boot/initramfs-*.img | sort -V | tail -n 1) '$IMG/void.initrd'
  mke2fs -q -t ext4 -L void -d '$W/rootfs' '$IMG/void.raw.part' 6G
  rm -rf '$W/rootfs'
"
chmod 644 "$IMG/void.vmlinuz" "$IMG/void.initrd" || true
mv "$IMG/void.raw.part" "$IMG/void.raw"
rm -f "$W/rootfs.tar"
echo "void: built $IMG/void.raw"
