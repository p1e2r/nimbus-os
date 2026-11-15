#!/usr/bin/env bash
# Smoke-test what the installer does after copying the live system:
# unpack the ISO's root image, run Nimbus's cleanup/finalize steps in it,
# install a package through pacman (keyring + DNS + repos) and build the
# initramfs. Run as root in a privileged Arch Linux container.
set -euo pipefail

iso=${1:?usage: test-install.sh ISO}
here=$(cd "$(dirname "$0")" && pwd)
work=${WORK_DIR:-/tmp/nimbus-test}
root="$work/target"

pacman -Sy --noconfirm --needed squashfs-tools libarchive psmisc procps-ng >/dev/null

rm -rf "$work"; mkdir -p "$work"
bsdtar -xf "$iso" -C "$work" arch/x86_64/airootfs.sfs
unsquashfs -q -d "$root" "$work/arch/x86_64/airootfs.sfs" >/dev/null
rm -f "$work/arch/x86_64/airootfs.sfs"

cleanup() {
    umount -R "$root/proc" "$root/sys" "$root/dev" "$root/run" 2>/dev/null || true
}
trap cleanup EXIT
# Same mounts as Calamares' mount module.
mount -t proc proc "$root/proc"
mount -t sysfs sys "$root/sys"
mount --bind /dev "$root/dev"
mount -t tmpfs tmpfs "$root/run"

fail() { echo "::error::$*"; exit 1; }

echo "::group::cleanup"
NIMBUS_LIVE_LIST="$root/usr/share/nimbus/live-only-files" \
    bash "$here/airootfs/usr/bin/nimbus-calamares-cleanup" "$root"
echo "::endgroup::"

[ -f "$root/boot/vmlinuz-linux" ] || fail "kernel was not restored to /boot"
[ ! -e "$root/etc/calamares" ] || fail "/etc/calamares still present"
[ ! -e "$root/etc/systemd/system/etc-pacman.d-gnupg.mount" ] || fail "live gnupg tmpfs mount left behind"
[ ! -e "$root/etc/mkinitcpio.conf.d/archiso.conf" ] || fail "archiso mkinitcpio config left behind"
[ -s "$root/etc/passwd" ] && grep -q '^root:' "$root/etc/passwd" || fail "/etc/passwd damaged"
grep -q '^NAME="Nimbus"' "$root/etc/os-release" || fail "os-release is not Nimbus"
[ -s "$root/etc/resolv.conf" ] || fail "no DNS config for the chroot"
pgrep -f "gpg-agent --homedir /etc/pacman.d/gnupg" >/dev/null && fail "gpg-agent still running"

echo "::group::pacman inside the target (what the desktop step does)"
chroot "$root" pacman -Sy --noconfirm
chroot "$root" pacman -S --noconfirm --needed nano
chroot "$root" pacman -Si nimbus-shell >/dev/null || fail "[nimbus] repo not usable"
echo "::endgroup::"

echo "::group::initramfs"
chroot "$root" mkinitcpio -p linux
[ -f "$root/boot/initramfs-linux.img" ] || fail "initramfs not built"
echo "::endgroup::"

echo "::group::finalize"
echo 'tester:x:1000:1000::/home/tester:/bin/bash' >> "$root/etc/passwd"
bash "$here/airootfs/usr/bin/nimbus-calamares-finalize" "$root"
chroot "$root" pacman -Q calamares >/dev/null 2>&1 && fail "calamares still installed"
grep -q '^Session=/usr/share/wayland-sessions/nimbus.desktop' "$root/var/lib/sddm/state.conf" || fail "SDDM default session not set"
echo "::endgroup::"

cleanup
trap - EXIT
# If anything still holds the target, unmounting it after install would fail.
if fuser -m "$root" 2>/dev/null; then fail "processes still using the target"; fi
echo "Install smoke test passed"
