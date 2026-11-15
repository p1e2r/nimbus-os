#!/usr/bin/env bash
# Build the Nimbus ISO from Arch's "releng" profile plus the Nimbus overlay.
# Run as root inside a privileged Arch Linux container, after
# build-packages.sh has filled ./localrepo.
set -euo pipefail

here=$(cd "$(dirname "$0")" && pwd)
repo_root=$(dirname "$here")
localrepo="$repo_root/localrepo"
work=${WORK_DIR:-/tmp/nimbus-work}
out=${OUT_DIR:-$repo_root/out}
profile="$work/profile"

pacman -Syu --noconfirm --needed archiso

[ -f "$localrepo/nimbus.db.tar.gz" ] || { echo "Run build-packages.sh first" >&2; exit 1; }

rm -rf "$work"
mkdir -p "$work" "$out"
cp -r /usr/share/archiso/configs/releng "$profile"

# --- Files from releng that must not end up on an installed system -------
list_files() { (cd "$1" && find . \( -type f -o -type l \) | sed 's|^\.||' | sort); }
releng_files=$(list_files "$profile/airootfs")
overlay_files=$(list_files "$here/airootfs")
# Never delete the account databases.
keep='^/etc/(passwd|shadow|group|gshadow)$'

# --- Live system tweaks to releng ---------------------------------------
# SDDM logs the live user in; drop releng's root autologin on tty1.
rm -rf "$profile/airootfs/etc/systemd/system/getty@tty1.service.d"
# NetworkManager handles networking instead of networkd + iwd.
# (systemd-resolved stays: the live /etc/resolv.conf points at it.)
find "$profile/airootfs/etc/systemd/system" \
    \( -name 'systemd-networkd*' -o -name 'iwd.service' \) \
    -prune -exec rm -rf {} +
rm -rf "$profile/airootfs/etc/systemd/network"

# --- Nimbus overlay ----------------------------------------------------
cp -a "$here/airootfs/." "$profile/airootfs/"
mkdir -p "$profile/airootfs/usr/share/nimbus"
{
    grep -v '^#' "$here/live-only-files"
    comm -23 <(echo "$releng_files") <(echo "$overlay_files") | grep -Ev "$keep" || true
} > "$profile/airootfs/usr/share/nimbus/live-only-files"

# Packages (deduplicated, comments dropped)
cat "$profile/packages.x86_64" "$here/packages.x86_64" \
    | sed 's/#.*//; s/[[:space:]]*$//' | grep -v '^$' | sort -u > "$work/packages"
mv "$work/packages" "$profile/packages.x86_64"

# Build-time pacman.conf: releng's plus the local Nimbus repo. The plain
# "labwc" session would sit confusingly next to "Nimbus" on the login screen.
sed -i '/^\[options\]/a NoExtract = usr/share/wayland-sessions/labwc.desktop' "$profile/pacman.conf"
cat >> "$profile/pacman.conf" <<EOF

[nimbus]
SigLevel = Optional TrustAll
Server = file://$localrepo
EOF

# ISO identity
sed -i \
    -e 's/^iso_name=.*/iso_name="nimbus"/' \
    -e 's/^iso_label=.*/iso_label="NIMBUS_$(date --date="@${SOURCE_DATE_EPOCH:-$(date +%s)}" +%Y%m)"/' \
    -e 's|^iso_publisher=.*|iso_publisher="Nimbus <https://github.com/p1e2r/nimbus-os>"|' \
    -e 's|^iso_application=.*|iso_application="Nimbus Live/Install Medium"|' \
    "$profile/profiledef.sh"
cat >> "$profile/profiledef.sh" <<'EOF'
file_permissions+=(
  ["/usr/bin/nimbus-live-setup"]="0:0:755"
  ["/usr/bin/nimbus-install"]="0:0:755"
  ["/usr/bin/nimbus-live-welcome"]="0:0:755"
  ["/usr/bin/nimbus-calamares-cleanup"]="0:0:755"
  ["/usr/bin/nimbus-calamares-finalize"]="0:0:755"
)
EOF

# Boot menu names
{ grep -rl "Arch Linux" "$profile"/{grub,syslinux,efiboot} 2>/dev/null || true; } | while read -r f; do
    sed -i -e 's/Arch Linux install medium/Nimbus live medium/g' \
           -e 's/Arch Linux/Nimbus/g' "$f"
done

echo "::group::Profile"
cat "$profile/profiledef.sh"
echo "--- live-only files on install: $(wc -l < "$profile/airootfs/usr/share/nimbus/live-only-files")"
echo "::endgroup::"

mkarchiso -v -w "$work/build" -o "$out" "$profile"

cd "$out"
for iso in *.iso; do
    sha256sum "$iso" > "$iso.sha256"
done
ls -la "$out"
