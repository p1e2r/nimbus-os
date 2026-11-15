#!/usr/bin/env bash
# Build Nimbus's own packages (nimbus-shell, calamares) into a local pacman
# repository at ./localrepo. Run as root inside an Arch Linux container.
set -euo pipefail

repo_root=$(cd "$(dirname "$0")/.." && pwd)
localrepo="$repo_root/localrepo"
packages=(nimbus-shell calamares)

pacman -Syu --noconfirm --needed base-devel git sudo

# makepkg refuses to run as root.
if ! id builder &>/dev/null; then
    useradd -m builder
fi
echo 'builder ALL=(ALL) NOPASSWD: ALL' > /etc/sudoers.d/builder
chown -R builder:builder "$repo_root"

mkdir -p "$localrepo"
for pkg in "${packages[@]}"; do
    echo "::group::Building $pkg"
    sudo -u builder bash -c "cd '$repo_root/packaging/$pkg' && makepkg --syncdeps --noconfirm --force --cleanbuild"
    cp "$repo_root/packaging/$pkg"/*.pkg.tar.zst "$localrepo/"
    echo "::endgroup::"
done

cd "$localrepo"
rm -f nimbus.db* nimbus.files*
repo-add nimbus.db.tar.gz ./*.pkg.tar.zst
ls -la "$localrepo"
