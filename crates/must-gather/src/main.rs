//! must-gather — stormcos debug-data collector (our `oc adm must-gather`),
//! run from a workstation or from a pod. No ssh: a stormcos node has none
//! for this, and its services run under stormpump, not systemd (#45).
//!
//! - **cluster** (`cluster.rs`): through the cluster API (TLS + token):
//!   discovery, health, every listed resource and custom resource, and the
//!   logs of kube-system's pods (the node's own services are mirror pods
//!   there) and of every pod in trouble.
//! - **host** (`host.rs`): per node, a short-lived pod with `hostPID`,
//!   `hostNetwork` and read-only host mounts runs `must-gather
//!   host-collect` from `--host-image`, which sends the node's stormpump and
//!   service logs, kernel log, pstore, host network, stormblock, fastetcd
//!   and Cilium agent state back through its pod log (`frame.rs`).
//! - **collectors** (`collect.rs`): per component, built in (stormcos_qa#68:
//!   they were shell scripts, and stormcos ships no shell): `network` and
//!   `cadvisor` per node, `ironprom` and `stormblock-csi` once.
//!
//! Output: `<out>/…` with `manifest.json` (every item and its status), then
//! `<out>.tar.gz` holding all of it, the manifest included (#13).

mod api;
mod cluster;
mod collect;
mod frame;
mod host;

use std::path::{Path, PathBuf};
use std::time::Duration;

use clap::{Args, Parser, Subcommand};
use serde::Serialize;
use serde_json::Value;

use api::Api;

#[derive(Parser)]
#[command(name = "must-gather", version, about, args_conflicts_with_subcommands = true)]
struct Cli {
    #[command(subcommand)]
    cmd: Option<Cmd>,
    #[command(flatten)]
    gather: GatherArgs,
}

#[derive(Subcommand)]
enum Cmd {
    /// On a node, inside the collector pod: print the host's data as a
    /// framed tar.gz on stdout (must-gather starts this itself).
    HostCollect(host::CollectArgs),
}

#[derive(Args)]
struct GatherArgs {
    /// The apiserver, `https://<node>:6443`. Empty: in-cluster (from a pod).
    #[arg(long, env = "MUST_GATHER_API", default_value = "")]
    api: String,
    /// A file holding a bearer token (default: the pod's ServiceAccount).
    #[arg(long, env = "MUST_GATHER_TOKEN_FILE")]
    token_file: Option<String>,
    /// The cluster's CA (default: the pod's ServiceAccount CA).
    #[arg(long, env = "MUST_GATHER_CA_FILE")]
    ca_file: Option<String>,
    /// Skip verifying the apiserver's certificate.
    #[arg(long)]
    insecure: bool,
    /// Output directory; `<out>.tar.gz` is written next to it.
    /// Default: `must-gather-<unix time>` in the current directory.
    #[arg(long)]
    out: Option<PathBuf>,
    /// Only these nodes (comma-separated names) for the host part; default all.
    #[arg(long, value_delimiter = ',')]
    nodes: Vec<String>,
    /// The image the per-node collector pod runs (it must hold must-gather
    /// at `--host-command`). Without it the host part is skipped.
    #[arg(long, env = "MUST_GATHER_HOST_IMAGE")]
    host_image: Option<String>,
    #[arg(long, default_value = "/must-gather")]
    host_command: String,
    /// Namespace for the collector pods. Default: a new
    /// `must-gather-<run>` namespace, deleted afterwards.
    #[arg(long)]
    host_namespace: Option<String>,
    /// How long a node's collector pod may take.
    #[arg(long, default_value_t = 300)]
    host_timeout: u64,
    /// The component collectors to run (default: all of them: network,
    /// cadvisor, ironprom, stormblock-csi).
    #[arg(long, value_delimiter = ',')]
    collectors: Vec<String>,
    /// Run no component collector.
    #[arg(long)]
    no_collectors: bool,
    /// Per-collector timeout (seconds).
    #[arg(long, default_value_t = 120)]
    timeout: u64,
}

#[derive(Serialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Ok,
    Error,
    Skipped,
}

#[derive(Serialize, Debug)]
pub struct Item {
    pub path: String,
    pub status: Status,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub detail: String,
}

impl Item {
    pub fn new(path: impl Into<String>, status: Status, detail: impl Into<String>) -> Self {
        Item { path: path.into(), status, detail: detail.into() }
    }
}

#[derive(Serialize)]
struct Manifest<'a> {
    must_gather: &'a str,
    run: &'a str,
    api: &'a str,
    started_unix: u64,
    nodes: Vec<String>,
    tarball: String,
    counts: Value,
    items: &'a [Item],
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    let r = match cli.cmd {
        Some(Cmd::HostCollect(a)) => host::host_collect(a).await,
        None => gather(cli.gather).await,
    };
    if let Err(e) = r {
        eprintln!("must-gather: {e:#}");
        std::process::exit(1);
    }
}

async fn gather(g: GatherArgs) -> anyhow::Result<()> {
    let started = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?;
    let run = format!("{:08x}", (started.as_nanos() as u64 ^ (std::process::id() as u64) << 32) as u32);
    let out = g.out.clone().unwrap_or_else(|| PathBuf::from(format!("must-gather-{}", started.as_secs())));
    if let Some(bad) = g.collectors.iter().find(|c| !collect::known(c)) {
        anyhow::bail!("--collectors: no collector named {bad} (there are {})", collect::NODE.iter().chain(collect::CLUSTER).copied().collect::<Vec<_>>().join(", "));
    }
    std::fs::create_dir_all(&out)?;
    let api = Api::new(&g.api, g.token_file.as_deref(), g.ca_file.as_deref(), g.insecure)?;
    let mut items: Vec<Item> = Vec::new();

    eprintln!("must-gather: cluster state from {}", api.base);
    let nodes = cluster::gather(&api, &out, &mut items).await;
    let node_list: Vec<(String, String)> = nodes
        .as_ref()
        .map(api::items)
        .unwrap_or(&[])
        .iter()
        .filter_map(|n| {
            let name = n["metadata"]["name"].as_str()?.to_string();
            let ip = n["status"]["addresses"].as_array()?.iter().find(|a| a["type"] == "InternalIP")?["address"].as_str().unwrap_or("").to_string();
            Some((name, ip))
        })
        .filter(|(n, _)| g.nodes.is_empty() || g.nodes.contains(n))
        .collect();

    // The host part, per node.
    match &g.host_image {
        None => items.push(Item::new("nodes/*/host", Status::Skipped, "no --host-image: the node's logs, kernel log and stormblock state were not collected")),
        Some(image) if !node_list.is_empty() => {
            let (ns, made) = match &g.host_namespace {
                Some(ns) => (ns.clone(), false),
                None => {
                    let ns = format!("must-gather-{run}");
                    let r = api.send(reqwest::Method::POST, "/api/v1/namespaces", Some(&host::namespace_spec(&ns, &run))).await?;
                    if !r.ok() {
                        anyhow::bail!("creating namespace {ns} answered {}: {}", r.code, cluster::first_line(&r.text));
                    }
                    (ns, true)
                }
            };
            let hp = host::HostPod { ns: &ns, image, command: &g.host_command, run: &run, timeout: Duration::from_secs(g.host_timeout) };
            for (node, _) in &node_list {
                eprintln!("must-gather: host data from {node}");
                let path = format!("nodes/{node}/host");
                match host::gather(&api, &hp, node, &out.join(&path)).await {
                    Ok(d) => items.push(Item::new(path, Status::Ok, d)),
                    Err(e) => items.push(Item::new(path, Status::Error, format!("{e:#}"))),
                }
            }
            if made {
                let _ = api.send(reqwest::Method::DELETE, &format!("/api/v1/namespaces/{ns}"), None).await;
            }
        }
        Some(_) => items.push(Item::new("nodes/*/host", Status::Skipped, "no nodes to collect from")),
    }

    // Component collectors: cluster-scoped once, node-scoped per node.
    if !g.no_collectors {
        let want = |a: &str| g.collectors.is_empty() || g.collectors.iter().any(|c| c == a);
        let http = collect::node_client()?;
        let timeout = Duration::from_secs(g.timeout);
        for area in collect::CLUSTER.iter().filter(|a| want(a)) {
            eprintln!("must-gather: collector {area}");
            items.push(run_collector(&api, &http, area, "", "", timeout, &out, format!("collectors/{area}.txt")).await);
        }
        for (node, ip) in &node_list {
            for area in collect::NODE.iter().filter(|a| want(a)) {
                eprintln!("must-gather: collector {area} on {node}");
                items.push(run_collector(&api, &http, area, node, ip, timeout, &out, format!("nodes/{node}/collectors/{area}.txt")).await);
            }
        }
    }

    // The manifest goes in before the tarball is made (#13).
    let tarball = format!("{}.tar.gz", out.to_string_lossy().trim_end_matches(['/', '\\']));
    let count = |s: Status| items.iter().filter(|i| i.status == s).count();
    let manifest = Manifest {
        must_gather: env!("CARGO_PKG_VERSION"),
        run: &run,
        api: &api.base,
        started_unix: started.as_secs(),
        nodes: node_list.iter().map(|(n, _)| n.clone()).collect(),
        tarball: tarball.clone(),
        counts: serde_json::json!({"ok": count(Status::Ok), "error": count(Status::Error), "skipped": count(Status::Skipped)}),
        items: &items,
    };
    std::fs::write(out.join("manifest.json"), serde_json::to_vec_pretty(&manifest)?)?;
    write_tarball(&out, &tarball)?;
    println!(
        "must-gather: {} nodes, {} ok, {} errors, {} skipped (see manifest.json) -> {tarball}",
        node_list.len(),
        count(Status::Ok),
        count(Status::Error),
        count(Status::Skipped)
    );
    Ok(())
}

/// One collector into `<out>/<path>`, bounded by `timeout`.
#[allow(clippy::too_many_arguments)]
async fn run_collector(api: &Api, http: &reqwest::Client, area: &str, node: &str, ip: &str, timeout: Duration, out: &Path, path: String) -> Item {
    let ctx = collect::Ctx { api, http: http.clone(), node, node_ip: ip };
    let (st, text) = match tokio::time::timeout(timeout, collect::run(area, &ctx)).await {
        Ok(t) => (Status::Ok, t),
        Err(_) => (Status::Error, format!("(collector {area} timed out after {}s)\n", timeout.as_secs())),
    };
    let target = out.join(&path);
    if let Some(dir) = target.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(&target, &text);
    Item::new(path, st, if st == Status::Ok { String::new() } else { cluster::first_line(&text) })
}

/// `<out>.tar.gz` holding `<out>/` under its own name (no `tar` program
/// needed: a Windows workstation has none).
fn write_tarball(out: &Path, tarball: &str) -> anyhow::Result<()> {
    let f = std::fs::File::create(tarball)?;
    let mut t = tar::Builder::new(flate2::write::GzEncoder::new(f, flate2::Compression::default()));
    let name = out.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "must-gather".into());
    t.append_dir_all(&name, out)?;
    t.into_inner()?.finish()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_manifest_is_inside_the_tarball() {
        let t = std::env::temp_dir().join(format!("mg-tar-{}", std::process::id()));
        let out = t.join("must-gather-1");
        std::fs::create_dir_all(out.join("cluster")).unwrap();
        std::fs::write(out.join("cluster/nodes.json"), "{}").unwrap();
        std::fs::write(out.join("manifest.json"), "{\"items\":[]}").unwrap();
        let tarball = format!("{}.tar.gz", out.display());
        write_tarball(&out, &tarball).unwrap();
        let mut ar = tar::Archive::new(flate2::read::GzDecoder::new(std::fs::File::open(&tarball).unwrap()));
        let names: Vec<String> = ar.entries().unwrap().map(|e| e.unwrap().path().unwrap().to_string_lossy().to_string()).collect();
        assert!(names.iter().any(|n| n == "must-gather-1/manifest.json"), "{names:?}");
        assert!(names.iter().any(|n| n == "must-gather-1/cluster/nodes.json"), "{names:?}");
        let _ = std::fs::remove_dir_all(&t);
    }

    #[test]
    fn plain_http_is_refused() {
        assert!(Api::new("http://127.0.0.1:6443", None, None, false).is_err());
        assert!(Api::new("https://127.0.0.1:6443", None, None, true).is_ok());
    }
}
