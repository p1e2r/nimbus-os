#!/usr/bin/env bash
# Build Nimbus's own packages into a pacman repository at ./localrepo.
# Run as root inside an Arch Linux container.
#
# If NIMBUS_GPG_KEY holds an ASCII-armored secret key, every package and
# the repo database are signed with it.
set -euo pipefail

repo_root=$(cd "$(dirname "$0")/.." && pwd)
localrepo="$repo_root/localrepo"
packages=(nimbus-release nimbus-shell nimbus-rdp calamares)

pacman -Syu --noconfirm --needed base-devel git sudo

# makepkg refuses to run as root.
if ! id builder &>/dev/null; then
    useradd -m builder
fi
echo 'builder ALL=(ALL) NOPASSWD: ALL' > /etc/sudoers.d/builder
chown -R builder:builder "$repo_root"
sudo -u builder git config --global --add safe.directory '*'

sign_args=()
repo_sign_args=()
if [ -n "${NIMBUS_GPG_KEY:-}" ]; then
    echo "Signing enabled"
    printf '%s\n' "$NIMBUS_GPG_KEY" | sudo -u builder gpg --batch --import
    fpr=$(sudo -u builder gpg --list-secret-keys --with-colons | awk -F: '/^fpr/{print $10; exit}')
    sign_args=(--sign --key "$fpr")
    repo_sign_args=(--sign --key "$fpr")
else
    echo "::warning::NIMBUS_GPG_KEY is not set; packages will be unsigned"
fi

rm -rf "$localrepo"
mkdir -p "$localrepo"
chown builder:builder "$localrepo"

for pkg in "${packages[@]}"; do
    echo "::group::Building $pkg"
    sudo -u builder bash -c "cd '$repo_root/packaging/$pkg' && rm -f ./*.pkg.tar.zst ./*.pkg.tar.zst.sig && makepkg --syncdeps --noconfirm --force --cleanbuild ${sign_args[*]}"
    cp "$repo_root/packaging/$pkg"/*.pkg.tar.zst* "$localrepo/"
    echo "::endgroup::"
done

cd "$localrepo"
sudo -u builder repo-add "${repo_sign_args[@]}" nimbus.db.tar.gz ./*.pkg.tar.zst
ls -la "$localrepo"
