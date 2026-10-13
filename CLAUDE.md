# CLAUDE.md — stormgather

`must-gather` for stormcos: a debug-data collector run from a workstation
or a pod. Cluster state and pod logs through the API; each node's host data
through a short-lived collector pod (no ssh); component collector scripts
(`gather/`). Public, MIT. Moved here from stormcos_qa with its history
(stormcos_qa#48, 2026-10-13).

Read the cross-project rules in `../CLAUDE.md` first: build with
`sc-build` after pushing, never on the stormcentral VM, never as root.

## Build and test

```bash
git push
sc-build                                                     # cargo build && cargo test
sc-build 'cargo clippy --locked --all-targets -- -D warnings'
```

Live test: stormcos_qa's `/test must-gather` (`stormcentral test run
stormcos_qa must-gather --tag <machine>`). stormcos_qa builds must-gather
from this repo at the commit its `test/stormgather.rev` names.

## Version locations

| File | Field |
|---|---|
| `Cargo.toml` | `[workspace.package] version` (both crates inherit it) |
| `CHANGELOG.md` | latest release heading |
| git tag | `vX.Y.Z` |

Current version: **0.1.0** (no tag cut yet).

## Shipping

- **Golden** `stormgather` (kind `binaries`, `bin/must-gather`, x86_64
  musl): `stormcentral component build stormgather --url http://stormcentral.g8.lo`
  once an issue's work is pushed and sc-build passes.
- **Laptop binaries**: tag `vX.Y.Z`, push the tag, then
  `deploy/publish-release.sh vX.Y.Z` from a checkout named `stormgather`.
  Linux x86_64/aarch64 and Windows x86_64; macOS waits on stormcentral#387.

## Users

- stormcos_qa: `crates/qa-test` depends on `stormgather-retry` (git, pinned
  rev); its test image builds `must-gather` from here at `test/stormgather.rev`.

## Work plan

### In progress — the move (stormcos_qa#48, 2026-10-13)

- [x] history split from stormcos_qa (22 commits), references rewritten to `stormcos_qa#N`
- [x] workspace, LICENSE, README (customer docs), CHANGELOG, deploy scripts
- [ ] Cargo.lock from the build VM; sc-build + clippy
- [ ] component `stormgather` (kind binaries) + first golden
- [ ] v0.1.0 tag + release assets (linux x86_64/aarch64)
- [ ] own test container (stormcentral test standard): today the live test is stormcos_qa's `/test must-gather`
