#!/usr/bin/env bash
# Publish the website plus an APT repository to the gh-pages branch.
#
#   scripts/publish-repos.sh            # rebuild from every release
#   scripts/publish-repos.sh v0.1.2-alpha   # ...and make sure this tag is included
#
# Needs: gpg with the signing key below, apt-ftparchive (apt-utils), gh.
# The private key never leaves this machine; only the public key is published.
set -euo pipefail

repo="its-banana-coder/noida"
key_id="${NOIDA_GPG_KEY:-noida@its-banana-coder.github.io}"
root="$(cd "$(dirname "$0")/.." && pwd)"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

command -v apt-ftparchive >/dev/null || { echo "install apt-utils first: sudo apt install apt-utils"; exit 1; }
gpg --list-secret-keys "$key_id" >/dev/null || { echo "no signing key for $key_id"; exit 1; }

echo "==> collecting .deb packages from releases"
mkdir -p "$work/site/apt/pool/main/n/noida" "$work/site/apt/dists/stable/main/binary-amd64"
for tag in $(gh api "repos/$repo/releases?per_page=50" --jq '.[].tag_name'); do
  version="${tag#v}"
  rm -rf "$work/dl"
  if gh release download "$tag" -R "$repo" -p 'noida_amd64.deb' -D "$work/dl" 2>/dev/null; then
    mv "$work/dl/noida_amd64.deb" "$work/site/apt/pool/main/n/noida/noida_${version}_amd64.deb"
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

echo "==> assembling the site"
cp -r "$root/docs/." "$work/site/"
touch "$work/site/.nojekyll"

echo "==> publishing to gh-pages"
cd "$work/site"
git init -q
git add -A
git -c user.email=noreply@github.com -c user.name="NOIDA release" commit -qm "Site and apt repository ($(date -u +%F))"
git push -q --force "https://github.com/$repo.git" HEAD:gh-pages
echo "done: https://its-banana-coder.github.io/noida/apt"
