# stormgather

`must-gather` for stormcos: one command that collects what is needed to
debug a stormcos cluster (the API's view of it, every node's host data,
and component collectors) into one tarball, the way `oc adm must-gather`
does for OpenShift. Public, MIT.

The repo is `stormgather` (stormcos projects are `storm<name>`); the
binary is `must-gather`, the name people already know.

## Get it

**A laptop binary per release**, one static file per platform, attached to
each [GitHub release](https://github.com/glennswest/stormgather/releases)
with a `SHA256SUMS`:

```bash
v=0.2.0
curl -LO https://github.com/glennswest/stormgather/releases/download/v$v/must-gather-$v-x86_64-unknown-linux-musl
curl -LO https://github.com/glennswest/stormgather/releases/download/v$v/SHA256SUMS
sha256sum -c SHA256SUMS --ignore-missing
chmod +x must-gather-$v-x86_64-unknown-linux-musl
```

Linux x86_64 and aarch64 (static musl) and Windows x86_64
(`must-gather-<v>-x86_64-pc-windows-gnu.exe`). macOS follows when the build
box has a macOS SDK (stormcentral#387). The component collectors are built
in: nothing else to download, and nothing needs a shell (stormcos_qa#68).

**A golden for the nodes**: `stormcentral component build stormgather`
seals `golden-stormgather-<sha12>` (kind `binaries`, `bin/must-gather`,
x86_64 musl) on forge, with its commit behind it, for a release or an image
to take. The per-node collector pod runs whatever image `--host-image`
names; today that is stormcos_qa's test image, which carries the binary.

## Use

stormcos's debug-data collector, run from a workstation (the usual
case) or from a pod. **No ssh**: a stormcos node has none for this (ssh lands
in the `fedora` container, not on the host), and its services run under
stormpump, not systemd (stormcos_qa#45).

```
must-gather --api https://<node>:6443 --token-file <f> [--ca-file <ca> | --insecure] \
            [--host-image <image>] [--out <dir>] [--nodes <n1,n2>] [--collectors <a,b> | --no-collectors]
```

| Flag | Default | Effect |
|---|---|---|
| `--api <url>` (`MUST_GATHER_API`) | in-cluster | The apiserver. `http://` is refused: rustkube serves TLS (stormcos_qa#11). Empty: the pod's own (`KUBERNETES_SERVICE_HOST`). |
| `--token-file <f>` (`MUST_GATHER_TOKEN_FILE`) | the pod's ServiceAccount token | Bearer token, re-read on every request. |
| `--ca-file <f>` (`MUST_GATHER_CA_FILE`) | the pod's ServiceAccount CA | The cluster's CA. |
| `--insecure` | off | Do not verify the apiserver's certificate. |
| `--out <dir>` | `must-gather-<unix time>` | Output directory; `<out>.tar.gz` is written next to it. |
| `--nodes <a,b>` | every node | Nodes for the host part and the node-scoped collectors. |
| `--host-image <ref>` (`MUST_GATHER_HOST_IMAGE`) | — | The image the per-node collector pod runs; it must hold must-gather at `--host-command` (`/must-gather`). Without it the host part is skipped and the manifest says so. The `stormgather` golden holds it at `bin/must-gather`; stormcos_qa's test image also carries it at `/must-gather`. |
| `--host-namespace <ns>` | a new `must-gather-<run>` | Where the collector pods run. A created namespace is labelled `pod-security.kubernetes.io/enforce=privileged` and deleted afterwards. |
| `--host-timeout <s>` | `300` | How long a node's collector pod may take. |
| `--collectors <a,b>` | all | Component collectors to run: `network`, `cadvisor`, `ironprom`, `stormblock-csi`. An unknown name is refused. |
| `--no-collectors` | off | Run none. |
| `--timeout <s>` | `120` | Each collector's timeout. |

**What it collects:**

- `cluster/`, through the API: `version`, `healthz`, `livez`, `readyz`,
  `api`, `apis`; every object of nodes, namespaces, pods, events, services,
  endpoints, PVs, PVCs, service accounts, deployments, daemonsets,
  statefulsets, replicasets, jobs, storage classes, volume attachments, CSI
  drivers and nodes, leases, network policies, CRDs and priority classes
  (paged, one List per file); and every custom resource of every CRD in
  `cluster/crs/<plural>.<group>.json` (VMs, VMIs, CloudImages, placements,
  stormblock's, …). **Never Secrets or ConfigMaps.**
- `logs/<ns>/<pod>/<container>[.previous].log`: every kube-system pod (the
  node's own services, fastetcd, the apiserver, rustkube-node and the rest,
  are mirror pods `kube-system/<service>-<node>` there) and every pod that
  is not Running and Ready, or has restarted (the previous log too); last
  5,000 lines, at most 4 MiB each. These pass through rustkube-node's log
  filter, which cuts the first three words of a line with 3+ spaces
  (rustkube-node#136); the host bundle has the same logs unfiltered.
- `nodes/<node>/host/`, per node: a pod pinned with `spec.nodeName`, with
  `hostPID`, `hostNetwork` and read-only mounts of `/run/stormpump`,
  `/sys/fs/cgroup`, `/l`, `/var/log/pods`, `/dev`, `/sys/fs/pstore`,
  `/etc/stormpump`, `/state/config`, stormblock's engine token and
  `/var/run/cilium` (the Cilium agent's API socket), runs
  `must-gather host-collect`. Its bundle: the host's `/proc` (version,
  cmdline, mountinfo, meminfo, pressure, …) and `/proc/net`; the kernel log
  (`/dev/kmsg`, read without consuming it), pstore, `/dev` and sysfs
  listings, the block-device map, the cgroup tree; stormpump's
  `assets.json`, `runs.tsv` and logs; stormd's service logs (`/l`); every
  pod log file as written; `/etc/stormpump` and `stormcos.toml` with
  secret-looking values redacted, and a listing of `/state/config`;
  stormblock's engine API (health, volumes, slabs, drives, arrays, pallets,
  exports, rebuilds, debug, metrics) and fastetcd's metrics (`:2381`);
  the Cilium agent's API over its socket, `cilium/status.json` (`/v1/healthz`,
  what `cilium status --verbose` shows: there is no exec on a node), `config`,
  `endpoints`, `cluster-nodes` and `maps` (stormcos_qa#22);
  `errors.txt` for anything it could not read. The kubelet has no exec and
  the apiserver no node proxy, so the bundle comes back through the pod's
  log as base64 lines between `mg:begin:<bytes>:<sha256>` and `mg:end`
  (no spaces, so #136 cannot touch it), checked on arrival. **Secrets stay
  on the node**: keys, tokens, pull secrets and `authorized_keys` are listed
  with their size, never copied.
- `nodes/<node>/collectors/<area>.txt` (`network`, `cadvisor`) and
  `collectors/<area>.txt` (`ironprom`, `stormblock-csi`): the component collectors.
- `manifest.json`: every item with `ok`, `error` (and why) or `skipped`, and
  the counts. It is written before the tarball, so it is in it (stormcos_qa#13).

**Component collectors** are built into must-gather (`collect.rs`; they
were `gather/<area>/*.sh` shell scripts until 0.2.0, stormcos_qa#68). They
run where must-gather runs (not on the node), read the cluster API with the
same credentials, and read a node's own ports over HTTP (retried, TLS not
verified). A read that fails is written into the text, never fatal.

| Collector | Scope | Collects |
|---|---|---|
| `network` | per node | the pod network for this node (stormcos_qa#22), through the API: the `Network` CR's status (conditions, `applied*`), the status of `cilium`, `cilium-operator`, `cilium-envoy` (if present) and `network-operator` in `$CILIUM_NAMESPACE` (`kube-system`), `ConfigMap cilium-config` (secret-looking keys redacted; the only ConfigMap must-gather reads), this node's agent pod and the operator pods (phase, ready, restarts, why waiting), the `CiliumNode` (`spec.ipam`, status), the `CiliumEndpoint`s on this node, every `CiliumLoadBalancerIPPool` / `CiliumBGP*` object; then the agent's metrics (`$CILIUM_METRICS`, default `http://<node>:9962/metrics`, `none` skips: unreachable nodes, endpoint states, IPAM, errors). The agent's own status is in the host bundle (`host/cilium/`), its logs under `logs/kube-system/`. Owner: glennswest/network-operator |
| `cadvisor` | per node | cadvisor's `/healthz`, version, attributes, machine, container names, last 100 events, its version/machine metrics and `container_*` series count on `$CADVISOR_ENDPOINT` (default `http://<node>:9096`, optional `CADVISOR_TOKEN`); stormd's `/healthz`, `process="cadvisor"` metrics, `/api/v1/processes/cadvisor` and the last 300 log lines on `$CADVISOR_STORMD` (`:9196`, `none` skips, optional `STORMD_TOKEN`). The cgroup tree and disk map are in the host bundle. Owner: glennswest/cadvisor |
| `ironprom` | once | through the API: ironprom's pods and StatefulSets in `$IRONPROM_NAMESPACE` (`monitoring`); then, if the pod address is reachable from here, buildinfo, `status/tsdb`, `status/runtimeinfo`, targets, rules and `ironprom_*` self-metrics. Owner: glennswest/ironprom |
| `stormblock-csi` | once | through the API: pods in `stormblock-system`, `wanderingvolumes` and `volumepolicies`, the `stormblock-tiebreak` and node leases, `csistoragecapacities`. Owner: glennswest/stormblock-csi |

The old ssh-based `fastetcd`, `kernel` and `stormblock` scripts are gone:
the host bundle has what they read (fastetcd's metrics and logs, ublk and
io_uring state, stormblock's engine).

Collector scripts get `QA_*` variables: the names they had in stormcos_qa,
kept so existing scripts keep working.

## Build and test

```bash
git push
sc-build                              # cargo build && cargo test, on a fresh build VM
sc-build 'cargo clippy --locked --all-targets -- -D warnings'
```

Nothing builds on the stormcentral VM. `cargo test` runs must-gather's and
the retry helper's unit tests (fake apiserver, fake Cilium socket, the
base64 frame). The live test is stormcos_qa's `/test must-gather` suite on
a test machine (`stormcentral test run stormcos_qa must-gather`): it runs
must-gather in-cluster against the machine and checks the bundle (the
manifest, the cluster lists, each node's host bundle, no secret copied, no
collector pod left).

## Release

`deploy/build-release.sh` builds one static binary per target and
`SHA256SUMS`; `deploy/publish-release.sh vX.Y.Z` builds the tag on the
build box (`sc-build`, the binaries come back as one tar) and uploads them
to the tag's GitHub release with this machine's `gh` login (no runners, no
secrets). The recipe is storminstall's and sc's.

## Layout

| Path | Job |
|---|---|
| `crates/must-gather/src/` | `main.rs` CLI, manifest, tarball; `api.rs` TLS client (paged lists); `cluster.rs` API collectors + pod logs; `collect.rs` component collectors; `host.rs` per-node collector pod + `host-collect`; `frame.rs` the base64 frame through a pod log |
| `crates/retry/src/` | the retry helper (`stormgather-retry`, lib `retry`): `with_backoff`/`probe`, policies, `Infra`; `http.rs` HTTP classification. Also used by stormcos_qa's tests |
| `deploy/` | release build and publish |

## History

must-gather grew in glennswest/stormcos_qa and moved here with its history
(stormcos_qa#48, 2026-10-13). Issue references in commits before the move
are written `stormcos_qa#N`.
