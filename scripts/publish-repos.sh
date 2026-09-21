#!/usr/bin/env bash
# Publish the website plus APT and RPM repositories to the gh-pages branch.
#
#   scripts/publish-repos.sh            # rebuild from every release
#   scripts/publish-repos.sh v0.1.5-alpha   # ...and make sure this tag is included
#
# Needs: gpg with the signing key below, apt-ftparchive (apt-utils),
# createrepo_c (createrepo-c), gh.
# The private key never leaves this machine; only the public key is published.
set -euo pipefail

repo="its-banana-coder/noida"
key_id="${NOIDA_GPG_KEY:-noida@its-banana-coder.github.io}"
root="$(cd "$(dirname "$0")/.." && pwd)"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

command -v apt-ftparchive >/dev/null || { echo "install apt-utils first: sudo apt install apt-utils"; exit 1; }
command -v createrepo_c >/dev/null || { echo "install createrepo-c first: sudo apt install createrepo-c"; exit 1; }
gpg --list-secret-keys "$key_id" >/dev/null || { echo "no signing key for $key_id"; exit 1; }

# Debian and rpm spell the same machines differently.
deb_arches="amd64 arm64"
rpm_arches="x86_64 aarch64"

echo "==> collecting packages from releases"
mkdir -p "$work/site/apt/pool/main/n/noida"
for a in $deb_arches; do mkdir -p "$work/site/apt/dists/stable/main/binary-$a"; done
for a in $rpm_arches; do mkdir -p "$work/site/rpm/$a"; done
for tag in $(gh api "repos/$repo/releases?per_page=50" --jq '.[].tag_name'); do
  version="${tag#v}"
  rm -rf "$work/dl"
  patterns=()
  for a in $deb_arches; do patterns+=(-p "noida_$a.deb"); done
  for a in $rpm_arches; do patterns+=(-p "noida.$a.rpm"); done
  if gh release download "$tag" -R "$repo" "${patterns[@]}" -D "$work/dl" 2>/dev/null; then
    for a in $deb_arches; do
      [ -f "$work/dl/noida_$a.deb" ] &&
        mv "$work/dl/noida_$a.deb" "$work/site/apt/pool/main/n/noida/noida_${version}_$a.deb"
    done
    for a in $rpm_arches; do
      [ -f "$work/dl/noida.$a.rpm" ] &&
        mv "$work/dl/noida.$a.rpm" "$work/site/rpm/$a/noida-${version}.$a.rpm"
    done
    echo "    $tag"
  fi
done
ls "$work/site/apt/pool/main/n/noida/"*.deb >/dev/null 2>&1 || { echo "no .deb assets found"; exit 1; }

echo "==> building repository metadata"
cd "$work/site/apt"
for a in $deb_arches; do
  # Each architecture's index must list only its own packages.
  dpkg-scanpackages --multiversion --arch "$a" pool /dev/null > "dists/stable/main/binary-$a/Packages" 2>/dev/null
  gzip -9kf "dists/stable/main/binary-$a/Packages"
done
cat > "$work/apt-release.conf" <<EOF
APT::FTPArchive::Release::Origin "NOIDA";
APT::FTPArchive::Release::Label "NOIDA";
APT::FTPArchive::Release::Suite "stable";
APT::FTPArchive::Release::Codename "stable";
APT::FTPArchive::Release::Architectures "$deb_arches";
APT::FTPArchive::Release::Components "main";
APT::FTPArchive::Release::Description "NOIDA: a terminal IDE built around Claude Code and Codex";
EOF
apt-ftparchive -c "$work/apt-release.conf" release dists/stable > dists/stable/Release

echo "==> signing"
gpg --batch --yes --local-user "$key_id" --detach-sign --armor -o dists/stable/Release.gpg dists/stable/Release
gpg --batch --yes --local-user "$key_id" --clearsign -o dists/stable/InRelease dists/stable/Release
gpg --export --armor "$key_id" > key.asc
gpg --export "$key_id" > key.gpg

echo "==> building the rpm repository"
cd "$work/site/rpm"
found=""
for a in $rpm_arches; do
  ls "$a"/*.rpm >/dev/null 2>&1 || { rmdir "$a" 2>/dev/null; continue; }
  createrepo_c --quiet "$a"
  # dnf verifies repomd.xml's signature when repo_gpgcheck=1.
  gpg --batch --yes --local-user "$key_id" --detach-sign --armor "$a/repodata/repomd.xml"
  found="$found $a"
done
if [ -n "$found" ]; then
  echo "   arches:$found"
  gpg --export --armor "$key_id" > key.asc
  # $basearch lets one repo file serve every architecture.
  {
    echo "[noida]"
    echo "name=NOIDA"
    echo "baseurl=https://its-banana-coder.github.io/noida/rpm/\$basearch"
    echo "enabled=1"
    echo "repo_gpgcheck=1"
    echo "gpgcheck=0"
    echo "gpgkey=https://its-banana-coder.github.io/noida/rpm/key.asc"
  } > noida.repo
else
  echo "    no .rpm assets found, skipping"
fi

echo "==> assembling the site"
cp -r "$root/docs/." "$work/site/"
touch "$work/site/.nojekyll"

echo "==> publishing to gh-pages"
cd "$work/site"
git init -q
git add -A
git -c user.email=noreply@github.com -c user.name="NOIDA release" commit -qm "Site, apt and rpm repositories ($(date -u +%F))"
git push -q --force "https://github.com/$repo.git" HEAD:gh-pages
echo "done: https://its-banana-coder.github.io/noida/ (apt + rpm)"
