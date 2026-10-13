#!/usr/bin/env bash
# Publish a release's binaries as GitHub release assets (stormcos_qa#48):
#   deploy/publish-release.sh [vX.Y.Z]      (default: the tag at HEAD)
#
# Run from a checkout on the stormcentral VM, after the tag is pushed. It
#  1. checks out the tag in a worktree under tmp/ (sc-build builds a
#     checkout's HEAD, and needs the directory named stormgather);
#  2. builds it with `sc-build deploy/build-release.sh` for $TARGETS, the
#     targets the build box builds today; the binaries and SHA256SUMS
#     leave the job's drive as one tar (SC_BUILD_OUT / SC_BUILD_OUT_TO);
#  3. checks every binary against SHA256SUMS here;
#  4. creates the GitHub release on the tag (notes: its CHANGELOG section)
#     if there is none, and uploads the binaries and SHA256SUMS, replacing
#     assets of the same name, so it can be run again.
# The upload uses this machine's `gh` login: no GitHub runner, no GitHub
# secret (owner, storminstall#9). Nothing is built here: the build is on
# the build box. See README.md (Release).
set -euo pipefail
cd "$(dirname "$0")/.."
root=$PWD
# macOS (stormcentral#387) joins this list when the build box builds it.
TARGETS=${TARGETS:-"x86_64-unknown-linux-musl aarch64-unknown-linux-musl x86_64-pc-windows-gnu"}

tag=${1:-$(git describe --tags --exact-match HEAD 2>/dev/null || true)}
[[ $tag =~ ^v[0-9]+\.[0-9]+\.[0-9]+$ ]] || { echo "publish-release: give a vX.Y.Z tag (HEAD has none)" >&2; exit 2; }
git fetch -q --tags origin
git ls-remote --exit-code --tags origin "refs/tags/$tag" >/dev/null || { echo "publish-release: $tag is not on origin; push it first" >&2; exit 2; }
version=${tag#v}
repo=$(gh repo view --json nameWithOwner -q .nameWithOwner)
tagged=$(git show "$tag:Cargo.toml" | sed -n 's/^version = "\(.*\)"/\1/p' | head -1)
[[ $tagged == "$version" ]] || { echo "publish-release: $tag's Cargo.toml says $tagged" >&2; exit 2; }

mkdir -p tmp
export TMPDIR=$root/tmp
work=$root/tmp/release-$tag
rm -rf "$work"
git worktree prune
git worktree add -q --detach "$work/stormgather" "$tag"
trap 'git -C "$root" worktree remove --force "$work/stormgather" 2>/dev/null || true' EXIT

echo "== building $tag ($TARGETS) on the build box"
(cd "$work/stormgather" &&
  SC_BUILD_OUT=release.tar SC_BUILD_OUT_TO="$work/release.tar" \
  sc-build "TARGETS='$TARGETS' deploy/build-release.sh && (cd dist && tar -cf ../release.tar must-gather-* gather-* SHA256SUMS)")

mkdir "$work/dist"
tar -C "$work/dist" -xf "$work/release.tar"
cd "$work/dist"
sha256sum -c SHA256SUMS
assets=(must-gather-"$version"-* gather-"$version".tar.gz)
expect=$(wc -w <<<"$TARGETS")
[[ ${#assets[@]} -eq $((expect + 1)) ]] || { echo "publish-release: $expect targets asked for, ${#assets[@]} binaries came back" >&2; exit 1; }

# The release notes: the tag's section of CHANGELOG.md.
notes=$work/notes.md
git -C "$root" show "$tag:CHANGELOG.md" |
  awk -v h="## [$tag]" 'index($0, h) == 1 { on = 1; next } on && /^## \[/ { exit } on' > "$notes"
printf '\nBinaries: one per OS/arch, checked against `SHA256SUMS` (`sha256sum -c SHA256SUMS --ignore-missing`). Not signed yet. `gather-<version>.tar.gz` holds the collector scripts (`--collectors-dir`).\n' >> "$notes"

if gh release view "$tag" -R "$repo" >/dev/null 2>&1; then
  echo "== $tag has a release; uploading its assets"
else
  gh release create "$tag" -R "$repo" --verify-tag --title "stormgather $tag" --notes-file "$notes"
fi
gh release upload "$tag" -R "$repo" "${assets[@]}" SHA256SUMS --clobber
gh release view "$tag" -R "$repo" --json url,assets -q '.url, (.assets[] | "  \(.name)  \(.size) bytes")'
