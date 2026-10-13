# Changelog

## [Unreleased]

## [v0.1.0] — 2026-10-13

### Added
- must-gather in its own repo, with its history from glennswest/stormcos_qa (stormcos_qa#48): the `must-gather` binary, the `stormgather-retry` helper, the `gather/` collector scripts
- Release binaries (Linux x86_64/aarch64 static musl, Windows x86_64), `gather-<version>.tar.gz`, `SHA256SUMS`: `deploy/build-release.sh`, `deploy/publish-release.sh`
- Golden `stormgather` (kind binaries, `bin/must-gather`)

### Log
- **chore:** stormgather created from glennswest/stormcos_qa with its history (stormcos_qa#48; owner: own repo, public, MIT, named `stormgather`): `crates/must-gather` (binary `must-gather`), `crates/retry` (now `stormgather-retry`), `gather/`. Issue references in the moved commits read `stormcos_qa#N`
- **feat:** `deploy/build-release.sh` (static binaries per target + `gather-<version>.tar.gz` + `SHA256SUMS`) and `deploy/publish-release.sh` (GitHub release assets from the build box), after storminstall and sc
- **build:** Windows x86_64 joins the published targets (the build box links it)
- **build:** `.cargo/config.toml` links static musl with rust-lld (aarch64 had no cross linker); `parse_response` allowed dead on non-unix (the Windows build denied it); `Cargo.lock` pruned to this workspace on the build box
- **docs:** README for customers: download, flags, what is collected, what is never copied, collector scripts
