# CLAUDE.md — stormgather

`must-gather` for stormcos: a debug-data collector run from a workstation
or a pod. Cluster state and pod logs through the API; each node's host data
through a short-lived collector pod (no ssh); built-in component collectors
(`collect.rs`; the `gather/` shell scripts were ported, stormcos_qa#68). Public, MIT. Moved here from stormcos_qa with its history
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

Current version: **0.1.0** (tag v0.1.0).

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

### Done — the move (stormcos_qa#48, 2026-10-13)

- [x] history split from stormcos_qa (22 commits), references rewritten to `stormcos_qa#N`
- [x] workspace, LICENSE, README (customer docs), CHANGELOG, deploy scripts
- [x] Cargo.lock from the build VM; sc-build + clippy
- [x] component `stormgather` (kind binaries) + golden golden-stormgather-e6c252f4a455 (stormcos#564)
- [x] v0.1.0 tag + release assets (linux x86_64/aarch64, windows x86_64)
- [ ] own test container (stormcentral test standard): today the live test is stormcos_qa's `/test must-gather`

### In progress — collectors from shell to Rust (stormcos_qa#68, 2026-10-14)

Owner (2026-10-13): "is the shell not rustable?" — stormcos ships no shell.

Design: `collect.rs` built into must-gather, the four `gather/*/status.sh` ported 1:1 (same endpoints, env overrides, summaries: python3's summariser becomes pure functions with unit tests). Node-scoped (`network`, `cadvisor`) → `nodes/<node>/collectors/<area>.txt`; cluster-scoped (`ironprom`, `stormblock-csi`) once → `collectors/<area>.txt`. `--collectors a,b` (default all), `--no-collectors`, `--timeout` per collector. HTTP to node ports through the retry helper (GATHER policy, TLS not verified as `curl -k` did). Removed: `--collectors-dir`, `gather/`, the `gather-<v>.tar.gz` release asset. Breaking → **0.2.0**.

- [ ] collect.rs + tests, main.rs wiring; gather/ removed
- [ ] README, CHANGELOG, publish-release.sh; sc-build + clippy
- [ ] v0.2.0 tag, release, golden
- [ ] stormcos_qa: pin v0.2.0; `/test must-gather` wants the collectors' files; live run
