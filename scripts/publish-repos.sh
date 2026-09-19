#!/usr/bin/env bash
# Publish the website plus APT and RPM repositories to the gh-pages branch.
#
#   scripts/publish-repos.sh            # rebuild from every release
#   scripts/publish-repos.sh v0.1.3-alpha   # ...and make sure this tag is included
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

echo "==> collecting packages from releases"
mkdir -p "$work/site/apt/pool/main/n/noida" "$work/site/apt/dists/stable/main/binary-amd64" "$work/site/rpm/x86_64"
for tag in $(gh api "repos/$repo/releases?per_page=50" --jq '.[].tag_name'); do
  version="${tag#v}"
  rm -rf "$work/dl"
  if gh release download "$tag" -R "$repo" -p 'noida_amd64.deb' -p 'noida.x86_64.rpm' -D "$work/dl" 2>/dev/null; then
    [ -f "$work/dl/noida_amd64.deb" ] &&
      mv "$work/dl/noida_amd64.deb" "$work/site/apt/pool/main/n/noida/noida_${version}_amd64.deb"
    [ -f "$work/dl/noida.x86_64.rpm" ] &&
      mv "$work/dl/noida.x86_64.rpm" "$work/site/rpm/x86_64/noida-${version}.x86_64.rpm"
    echo "    $tag"
  fi
done
ls "$work/site/apt/pool/main/n/noida/"*.deb >/dev/null 2>&1 || { echo "no .deb assets found"; exit 1; }

echo "==> building repository metadata"
cd "$work/site/apt"
dpkg-scanpackages --multiversion pool /dev/null > dists/stable/main/binary-amd64/Packages 2>/dev/null
gzip -9kf dists/stable/main/binary-amd64/Packages
cat > "$work/apt-release.conf" <<EOF
APT::FTPArchive::Release::Origin "NOIDA";
APT::FTPArchive::Release::Label "NOIDA";
APT::FTPArchive::Release::Suite "stable";
APT::FTPArchive::Release::Codename "stable";
APT::FTPArchive::Release::Architectures "amd64";
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
if ls x86_64/*.rpm >/dev/null 2>&1; then
  createrepo_c --quiet x86_64
  # dnf verifies repomd.xml's signature when repo_gpgcheck=1.
  gpg --batch --yes --local-user "$key_id" --detach-sign --armor x86_64/repodata/repomd.xml
  gpg --export --armor "$key_id" > key.asc
  {
    echo "[noida]"
    echo "name=NOIDA"
    echo "baseurl=https://its-banana-coder.github.io/noida/rpm/x86_64"
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
