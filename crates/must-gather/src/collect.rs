//! Component collectors, built in (stormcos_qa#68: stormcos ships no shell).
//!
//! These were `gather/<area>/status.sh` scripts run with `sh` where
//! must-gather runs; each is now a function with the same reads, the same
//! environment overrides and the same summaries (what python3 used to print
//! is [`summ_status`], [`summ_pods`], …, with unit tests). Every read is
//! fetched from where must-gather runs: the cluster API through [`Api`],
//! a node's own ports (cadvisor, stormd, the Cilium agent's metrics,
//! ironprom) straight over HTTP, retried (GATHER policy), TLS not verified
//! (as `curl -k` did: those ports serve no cluster-issued certificate).
//!
//! - **node-scoped** (`nodes/<node>/collectors/<area>.txt`): `network`
//!   (stormcos_qa#22), `cadvisor`;
//! - **cluster-scoped** (`collectors/<area>.txt`, once): `ironprom`,
//!   `stormblock-csi`.
//!
//! A read that fails is written into the text (`(fetch failed: …)`), never
//! fatal, as the scripts did: a collector reports what it could see.

use std::fmt::Write as _;
use std::time::Duration;

use retry::Policy;
use retry::http::{self, Repeat};
use serde_json::Value;

use crate::api::Api;

/// Every area, in the order they run.
pub const NODE: &[&str] = &["network", "cadvisor"];
pub const CLUSTER: &[&str] = &["ironprom", "stormblock-csi"];

pub fn known(area: &str) -> bool {
    NODE.contains(&area) || CLUSTER.contains(&area)
}

/// What a collector reads with.
pub struct Ctx<'a> {
    pub api: &'a Api,
    /// For a node's own ports: no client certificate, TLS not verified.
    pub http: reqwest::Client,
    pub node: &'a str,
    pub node_ip: &'a str,
}

/// A client for node ports, as `curl -k --max-time 20` was.
pub fn node_client() -> anyhow::Result<reqwest::Client> {
    Ok(reqwest::Client::builder().timeout(Duration::from_secs(20)).danger_accept_invalid_certs(true).build()?)
}

fn env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

/// Run one area. Node-scoped areas read `ctx.node`/`ctx.node_ip`.
pub async fn run(area: &str, ctx: &Ctx<'_>) -> String {
    match area {
        "network" => network(ctx).await,
        "cadvisor" => cadvisor(ctx).await,
        "ironprom" => ironprom(ctx).await,
        "stormblock-csi" => stormblock_csi(ctx).await,
        other => format!("(no collector named {other})\n"),
    }
}

/// A cluster API read: the body as it came, or why there is none.
async fn api(ctx: &Ctx<'_>, path: &str) -> String {
    match ctx.api.get(path).await {
        Ok(g) => g.text,
        Err(e) => format!("(fetch failed: {}{path}: {e:#})", ctx.api.base),
    }
}

/// A node-port read (curl without `-f`: a 4xx/5xx body is printed too).
async fn get(ctx: &Ctx<'_>, url: &str, token: Option<&str>) -> String {
    let r = http::send(&Policy::GATHER, &format!("GET {url}"), Repeat::Safe, || {
        let r = ctx.http.get(url);
        match token {
            Some(t) => r.bearer_auth(t),
            None => r,
        }
    })
    .await;
    match r {
        Ok(a) => String::from_utf8_lossy(&a.body).into_owned(),
        Err(e) => format!("(fetch failed: {url}: {e:#})"),
    }
}

fn section(out: &mut String, title: &str) {
    let _ = writeln!(out, "{title}");
}

fn body(out: &mut String, text: &str) {
    out.push_str(text.trim_end());
    out.push('\n');
}

/// The first `n` bytes, on a char boundary (`head -c`).
pub fn head(s: &str, n: usize) -> &str {
    if s.len() <= n {
        return s;
    }
    let mut i = n;
    while !s.is_char_boundary(i) {
        i -= 1;
    }
    &s[..i]
}

/// Lines starting with one of `prefixes`, at most `max` (`grep -E '^(…)' | head`).
pub fn lines_starting(text: &str, prefixes: &[&str], max: usize) -> String {
    text.lines().filter(|l| prefixes.iter().any(|p| l.starts_with(p))).take(max).collect::<Vec<_>>().join("\n")
}

/// Parse an API answer; a `Status` (an error) or non-JSON comes back as the
/// line to print instead.
fn parse(raw: &str) -> Result<Value, String> {
    let v: Value = serde_json::from_str(raw).map_err(|_| head(raw, 4000).to_string())?;
    if v["kind"] == "Status" {
        return Err(format!("({} {})", v["code"].as_u64().map(|c| c.to_string()).unwrap_or_default(), v["message"].as_str().unwrap_or("")));
    }
    Ok(v)
}

fn items(v: &Value) -> Vec<&Value> {
    match v["items"].as_array() {
        Some(a) => a.iter().collect(),
        None => vec![v],
    }
}

fn name(o: &Value) -> &str {
    o["metadata"]["name"].as_str().unwrap_or("?")
}

/// Each object's name, `spec.ipam` if any, and its `.status`.
pub fn summ_status(raw: &str) -> String {
    let v = match parse(raw) {
        Ok(v) => v,
        Err(line) => return line,
    };
    let mut out = String::new();
    for o in items(&v) {
        let _ = writeln!(out, "-- {}", name(o));
        if let Some(ipam) = o["spec"].get("ipam").filter(|i| !i.is_null()) {
            let _ = writeln!(out, "spec.ipam: {ipam}");
        }
        let st = if o["status"].is_null() { Value::Object(Default::default()) } else { o["status"].clone() };
        let _ = writeln!(out, "{}", serde_json::to_string_pretty(&st).unwrap_or_default());
    }
    out.trim_end().to_string()
}

/// Per pod: name, node, phase, ready, restarts; each waiting or last
/// terminated container's reason and message.
pub fn summ_pods(raw: &str) -> String {
    let v = match parse(raw) {
        Ok(v) => v,
        Err(line) => return line,
    };
    let mut out = String::new();
    for p in items(&v) {
        let st = &p["status"];
        let empty = Vec::new();
        let inits = st["initContainerStatuses"].as_array().unwrap_or(&empty);
        let mains = st["containerStatuses"].as_array().unwrap_or(&empty);
        let ready = !mains.is_empty() && mains.iter().all(|c| c["ready"] == true);
        let restarts: u64 = inits.iter().chain(mains).map(|c| c["restartCount"].as_u64().unwrap_or(0)).sum();
        let _ = writeln!(
            out,
            "{} node={} phase={} ready={} restarts={restarts}",
            name(p),
            p["spec"]["nodeName"].as_str().unwrap_or(""),
            st["phase"].as_str().unwrap_or(""),
            if ready { "True" } else { "False" }
        );
        for c in inits.iter().chain(mains) {
            let w = if c["state"]["waiting"].is_object() { &c["state"]["waiting"] } else { &c["lastState"]["terminated"] };
            if w.is_object() {
                let msg = w["message"].as_str().unwrap_or("");
                let _ = writeln!(out, "   {}: {} {}", c["name"].as_str().unwrap_or(""), w["reason"].as_str().unwrap_or(""), head(msg, 300));
            }
        }
    }
    out.trim_end().to_string()
}

/// CiliumEndpoints whose `status.networking.node` is `ip`.
pub fn summ_on_node(raw: &str, ip: &str) -> String {
    let v = match parse(raw) {
        Ok(v) => v,
        Err(line) => return line,
    };
    let all = items(&v);
    let mine: Vec<&&Value> = all.iter().filter(|o| o["status"]["networking"]["node"].as_str() == Some(ip)).collect();
    let mut out = format!("{} of {} endpoints on {ip}\n", mine.len(), all.len());
    for o in mine {
        let st = &o["status"];
        let addrs: Vec<&str> = st["networking"]["addressing"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|a| a["ipv4"].as_str().filter(|s| !s.is_empty()).or_else(|| a["ipv6"].as_str()).unwrap_or(""))
            .collect();
        let id = match &st["id"] {
            Value::Null => String::new(),
            Value::String(s) => s.clone(),
            other => other.to_string(),
        };
        let _ = writeln!(
            out,
            "  {}/{} id={id} state={} {}",
            o["metadata"]["namespace"].as_str().unwrap_or(""),
            name(o),
            st["state"].as_str().unwrap_or(""),
            addrs.join(",")
        );
    }
    out.trim_end().to_string()
}

/// A ConfigMap's data, the values of secret-looking keys replaced.
pub fn redact(raw: &str) -> String {
    const WORDS: &[&str] = &["token", "secret", "password", "passwd", "credential", "private", "apikey", "api-key", "api_key"];
    let v = match parse(raw) {
        Ok(v) => v,
        Err(line) => return line,
    };
    let mut keys: Vec<(&String, &Value)> = v["data"].as_object().map(|m| m.iter().collect()).unwrap_or_default();
    keys.sort_by(|a, b| a.0.cmp(b.0));
    keys.iter()
        .map(|(k, val)| {
            let secret = WORDS.iter().any(|w| k.to_lowercase().contains(w));
            format!("{k}: {}", if secret { "<redacted>" } else { val.as_str().unwrap_or("") })
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// List paths of the cilium.io CRDs for LB IP pools and BGP, from the CRD list.
pub fn cilium_crds(raw: &str) -> Vec<String> {
    let Ok(v) = parse(raw) else { return vec![] };
    let mut out = Vec::new();
    for o in items(&v) {
        let s = &o["spec"];
        let plural = s["names"]["plural"].as_str().unwrap_or("");
        if s["group"] != "cilium.io" || !(plural.contains("loadbalancerippool") || plural.contains("bgp")) {
            continue;
        }
        let vs = s["versions"].as_array().cloned().unwrap_or_default();
        let ver = vs
            .iter()
            .find(|x| x["storage"] == true)
            .or(vs.first())
            .and_then(|x| x["name"].as_str())
            .unwrap_or("v2")
            .to_string();
        out.push(format!("/apis/cilium.io/{ver}/{plural}"));
    }
    out
}

/// The top-level keys of a JSON object, sorted, one per line (cadvisor's
/// container names); not an object: the first 4000 bytes.
pub fn keys(raw: &str) -> String {
    match serde_json::from_str::<Value>(raw) {
        Ok(Value::Object(m)) => {
            let mut k: Vec<&String> = m.keys().collect();
            k.sort();
            k.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("\n")
        }
        _ => head(raw, 4000).to_string(),
    }
}

/// The address of the first pod (an ironprom one first) that has one.
pub fn pod_ip(raw: &str) -> Option<String> {
    let v = parse(raw).ok()?;
    let all = items(&v);
    let ip = |o: &&Value| o["status"]["podIP"].as_str().filter(|s| !s.is_empty()).map(str::to_string);
    all.iter().filter(|o| name(o).contains("ironprom")).find_map(ip).or_else(|| all.iter().find_map(ip))
}

// ---- the collectors ----

/// network (stormcos_qa#22): the pod network's state for one node: the
/// Network CR, the Cilium and network-operator workloads, cilium-config, this
/// node's agent pod, CiliumNode and endpoints, LB pools and BGP objects, and
/// the agent's metrics. Everything else is in the bundle already:
/// `cluster/crs/networks.network.storm.io.json`, `cluster/crs/*.cilium.io.json`,
/// `logs/kube-system/cilium-*`, `nodes/<node>/host/cilium/` (the agent's own
/// API: `status.json` is what `cilium status --verbose` shows) and
/// `nodes/<node>/host/net/`. Overrides: `CILIUM_NAMESPACE` (kube-system),
/// `CILIUM_METRICS` (`http://<node ip>:9962/metrics`; `none` skips).
/// Owner: glennswest/network-operator (Cilium itself: stormcos-cilium).
async fn network(ctx: &Ctx<'_>) -> String {
    let ns = env("CILIUM_NAMESPACE").unwrap_or_else(|| "kube-system".into());
    let node = ctx.node;
    let metrics = env("CILIUM_METRICS").unwrap_or_else(|| format!("http://{}:9962/metrics", ctx.node_ip));
    let mut out = String::new();

    section(&mut out, "== Network CR (networks.network.storm.io) ==");
    body(&mut out, &summ_status(&api(ctx, "/apis/network.storm.io/v1/networks").await));

    section(&mut out, &format!("\n== workloads ({ns}) =="));
    for (group, kind, wname) in [
        ("apps/v1", "daemonsets", "cilium"),
        ("apps/v1", "deployments", "cilium-operator"),
        ("apps/v1", "daemonsets", "cilium-envoy"),
        ("apps/v1", "deployments", "network-operator"),
    ] {
        section(&mut out, &format!("-- {group}/{kind}/{wname}"));
        body(&mut out, &summ_status(&api(ctx, &format!("/apis/{group}/namespaces/{ns}/{kind}/{wname}")).await));
    }

    section(&mut out, &format!("\n== ConfigMap {ns}/cilium-config (secret-looking keys redacted) =="));
    body(&mut out, &redact(&api(ctx, &format!("/api/v1/namespaces/{ns}/configmaps/cilium-config")).await));

    section(&mut out, &format!("\n== Cilium agent pod on {node} =="));
    let sel = format!("labelSelector={}&fieldSelector={}", crate::api::enc("k8s-app=cilium"), crate::api::enc(&format!("spec.nodeName={node}")));
    body(&mut out, &summ_pods(&api(ctx, &format!("/api/v1/namespaces/{ns}/pods?{sel}")).await));
    body(&mut out, &format!("(its log: logs/{ns}/<pod>/; the agent's own status: nodes/{node}/host/cilium/status.json)"));
    section(&mut out, "\n== operator pods ==");
    for s in ["io.cilium/app=operator", "app.kubernetes.io/name=network-operator"] {
        body(&mut out, &summ_pods(&api(ctx, &format!("/api/v1/namespaces/{ns}/pods?labelSelector={}", crate::api::enc(s))).await));
    }

    section(&mut out, &format!("\n== CiliumNode {node} =="));
    body(&mut out, &summ_status(&api(ctx, &format!("/apis/cilium.io/v2/ciliumnodes/{node}")).await));
    section(&mut out, &format!("\n== CiliumEndpoints on {} ==", ctx.node_ip));
    body(&mut out, &summ_on_node(&api(ctx, "/apis/cilium.io/v2/ciliumendpoints").await, ctx.node_ip));

    section(&mut out, "\n== LB IP pools and BGP objects ==");
    let paths = cilium_crds(&api(ctx, "/apis/apiextensions.k8s.io/v1/customresourcedefinitions").await);
    if paths.is_empty() {
        body(&mut out, "(no CiliumLoadBalancerIPPool / CiliumBGP* CRDs)");
    }
    for p in paths {
        section(&mut out, &format!("-- {p}"));
        body(&mut out, &summ_status(&api(ctx, &p).await));
    }

    if metrics != "none" {
        section(&mut out, &format!("\n== agent metrics {metrics} (health, IPAM, endpoints, errors) =="));
        let m = get(ctx, &metrics, None).await;
        const KEEP: &[&str] = &[
            "cilium_version",
            "cilium_unreachable_nodes",
            "cilium_unreachable_health_endpoints",
            "cilium_endpoint_state",
            "cilium_endpoint_regenerations_total",
            "cilium_ip_addresses",
            "cilium_ipam_",
            "cilium_errors_warnings_total",
            "cilium_controllers_failing",
            "cilium_policy_import_errors",
            "cilium_datapath_errors_total",
            "cilium_bpf_map_pressure",
            "cilium_k8s_client_api_calls_total",
            "(fetch failed",
        ];
        body(&mut out, &lines_starting(&m, KEEP, 80));
    }
    out
}

/// cadvisor: health, version, machine info, the containers cadvisor knows,
/// recent events, its self-metrics, and stormd's view of the process and its
/// log. cadvisor listens on `<node>:9096` and its stormd on `:9196`.
/// Overrides: `CADVISOR_ENDPOINT` (`http://<node ip>:9096`), `CADVISOR_TOKEN`,
/// `CADVISOR_STORMD` (`http://<node ip>:9196`; `none` skips), `STORMD_TOKEN`.
/// The node's cgroup tree and block devices are in the host bundle
/// (`nodes/<node>/host/cgroup/`, `host/kernel/block-devices.txt`).
/// Owner: glennswest/cadvisor.
async fn cadvisor(ctx: &Ctx<'_>) -> String {
    let cad = env("CADVISOR_ENDPOINT").unwrap_or_else(|| format!("http://{}:9096", ctx.node_ip));
    let sd = env("CADVISOR_STORMD").unwrap_or_else(|| format!("http://{}:9196", ctx.node_ip));
    let tok = env("CADVISOR_TOKEN");
    let tok = tok.as_deref();
    let mut out = String::new();

    section(&mut out, &format!("== cadvisor {cad} =="));
    for p in ["/healthz", "/api/v2.0/version", "/api/v2.0/attributes", "/api/v2.0/machine"] {
        section(&mut out, &format!("-- {p}"));
        body(&mut out, &get(ctx, &format!("{cad}{p}"), tok).await);
    }
    section(&mut out, "-- containers (/api/v2.0/spec?recursive=true keys)");
    body(&mut out, &keys(&get(ctx, &format!("{cad}/api/v2.0/spec/?recursive=true"), tok).await));
    section(&mut out, "-- /api/v2.0/events (last 100)");
    body(&mut out, &get(ctx, &format!("{cad}/api/v2.0/events/?all_events=true&max_events=100"), tok).await);
    section(&mut out, "-- /metrics: self and machine families, series count");
    let m = get(ctx, &format!("{cad}/metrics"), tok).await;
    body(&mut out, &lines_starting(&m, &["cadvisor_version_info", "machine_"], 40));
    body(&mut out, &format!("container_* series: {}", m.lines().filter(|l| l.starts_with("container_")).count()));

    if sd != "none" {
        let stok = env("STORMD_TOKEN");
        section(&mut out, &format!("== stormd {sd} =="));
        section(&mut out, "-- /healthz");
        body(&mut out, &get(ctx, &format!("{sd}/healthz"), None).await);
        section(&mut out, "-- /metrics (process=cadvisor)");
        let m = get(ctx, &format!("{sd}/metrics"), None).await;
        body(&mut out, &m.lines().filter(|l| l.contains("process=\"cadvisor\"")).collect::<Vec<_>>().join("\n"));
        section(&mut out, "-- /api/v1/processes/cadvisor");
        body(&mut out, &get(ctx, &format!("{sd}/api/v1/processes/cadvisor"), stok.as_deref()).await);
        section(&mut out, "-- /api/v1/logs/cadvisor (tail 300)");
        body(&mut out, &get(ctx, &format!("{sd}/api/v1/logs/cadvisor?tail=300"), stok.as_deref()).await);
    }
    out
}

/// ironprom: its pods and StatefulSet through the cluster API, then the live
/// TSDB, targets and buildinfo from its HTTP API when the pod's address is
/// reachable from where must-gather runs. Override: `IRONPROM_NAMESPACE`
/// (monitoring). Owner: glennswest/ironprom.
async fn ironprom(ctx: &Ctx<'_>) -> String {
    let ns = env("IRONPROM_NAMESPACE").unwrap_or_else(|| "monitoring".into());
    let mut out = String::new();
    section(&mut out, &format!("== ironprom pods ({ns}) =="));
    let pods = api(ctx, &format!("/api/v1/namespaces/{ns}/pods")).await;
    body(&mut out, &summ_pods(&pods));
    section(&mut out, &format!("== ironprom statefulsets ({ns}) =="));
    body(&mut out, &summ_status(&api(ctx, &format!("/apis/apps/v1/namespaces/{ns}/statefulsets")).await));

    let Some(ip) = pod_ip(&pods) else {
        body(&mut out, "(no ironprom pod address)");
        return out;
    };
    let ep = format!("http://{ip}:9090");
    section(&mut out, &format!("== ironprom endpoint: {ep} =="));
    for (title, p) in [
        ("buildinfo", "/api/v1/status/buildinfo"),
        ("status/tsdb", "/api/v1/status/tsdb"),
        ("status/runtimeinfo", "/api/v1/status/runtimeinfo"),
        ("targets", "/api/v1/targets"),
        ("rules", "/api/v1/rules"),
    ] {
        section(&mut out, &format!("== {title} =="));
        body(&mut out, &get(ctx, &format!("{ep}{p}"), None).await);
    }
    section(&mut out, "== self-metrics (ironprom_*) ==");
    body(&mut out, &lines_starting(&get(ctx, &format!("{ep}/metrics"), None).await, &["ironprom_", "(fetch failed"], usize::MAX));
    out
}

/// stormblock-csi: driver/operator pods, wandering CRs, volume policies,
/// leases and capacity objects through the cluster API: the CR and pod state
/// a failover post-mortem needs. The node's nvme/ublk devices are in the host
/// bundle (`host/kernel/`). Owner: glennswest/stormblock-csi.
async fn stormblock_csi(ctx: &Ctx<'_>) -> String {
    let mut out = String::new();
    section(&mut out, "== stormblock-system pods ==");
    body(&mut out, &summ_pods(&api(ctx, "/api/v1/namespaces/stormblock-system/pods").await));
    for (title, path, n) in [
        ("wanderingvolumes (all ns)", "/apis/stormblock.io/v1alpha1/wanderingvolumes", 4000),
        ("volumepolicies", "/apis/stormblock.io/v1alpha1/volumepolicies", 2000),
        ("tiebreak lease", "/apis/coordination.k8s.io/v1/namespaces/stormblock-system/leases/stormblock-tiebreak", 4000),
        ("node leases", "/apis/coordination.k8s.io/v1/namespaces/kube-node-lease/leases", 2000),
        ("csistoragecapacities", "/apis/storage.k8s.io/v1/namespaces/stormblock-system/csistoragecapacities", 2000),
    ] {
        section(&mut out, &format!("\n== {title} =="));
        body(&mut out, head(&api(ctx, path).await, n));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn status_names_each_object_with_ipam_and_status() {
        let raw = json!({"items": [{"metadata": {"name": "default"}, "spec": {"ipam": {"mode": "cluster-pool"}}, "status": {"applied": true}}]}).to_string();
        let s = summ_status(&raw);
        assert!(s.starts_with("-- default\nspec.ipam: {\"mode\":\"cluster-pool\"}\n"), "{s}");
        assert!(s.contains("\"applied\": true"), "{s}");
        assert_eq!(summ_status(&json!({"kind": "Status", "code": 404, "message": "not found"}).to_string()), "(404 not found)");
        assert_eq!(summ_status("(fetch failed: x)"), "(fetch failed: x)");
    }

    #[test]
    fn pods_say_ready_restarts_and_why_a_container_waits() {
        let raw = json!({"items": [{"metadata": {"name": "cilium-x"}, "spec": {"nodeName": "n1"},
            "status": {"phase": "Running", "containerStatuses": [
                {"name": "agent", "ready": false, "restartCount": 3, "state": {"waiting": {"reason": "CrashLoopBackOff", "message": "boom"}}}]}}]})
        .to_string();
        assert_eq!(summ_pods(&raw), "cilium-x node=n1 phase=Running ready=False restarts=3\n   agent: CrashLoopBackOff boom");
    }

    #[test]
    fn endpoints_on_this_node_only() {
        let raw = json!({"items": [
            {"metadata": {"name": "a", "namespace": "ns"}, "status": {"id": 7, "state": "ready", "networking": {"node": "10.0.0.1", "addressing": [{"ipv4": "10.1.0.5"}]}}},
            {"metadata": {"name": "b", "namespace": "ns"}, "status": {"networking": {"node": "10.0.0.2"}}}]})
        .to_string();
        assert_eq!(summ_on_node(&raw, "10.0.0.1"), "1 of 2 endpoints on 10.0.0.1\n  ns/a id=7 state=ready 10.1.0.5");
    }

    #[test]
    fn secret_looking_keys_are_redacted() {
        let raw = json!({"data": {"cluster-name": "default", "etcd-api-key": "x", "kvstore-password": "y"}}).to_string();
        assert_eq!(redact(&raw), "cluster-name: default\netcd-api-key: <redacted>\nkvstore-password: <redacted>");
    }

    #[test]
    fn lb_pool_and_bgp_crds_by_storage_version() {
        let raw = json!({"items": [
            {"spec": {"group": "cilium.io", "names": {"plural": "ciliumloadbalancerippools"}, "versions": [{"name": "v2alpha1"}, {"name": "v2", "storage": true}]}},
            {"spec": {"group": "cilium.io", "names": {"plural": "ciliumbgpclusterconfigs"}, "versions": [{"name": "v2alpha1"}]}},
            {"spec": {"group": "cilium.io", "names": {"plural": "ciliumnodes"}, "versions": [{"name": "v2"}]}},
            {"spec": {"group": "other.io", "names": {"plural": "bgppeers"}, "versions": [{"name": "v1"}]}}]})
        .to_string();
        assert_eq!(cilium_crds(&raw), vec!["/apis/cilium.io/v2/ciliumloadbalancerippools", "/apis/cilium.io/v2alpha1/ciliumbgpclusterconfigs"]);
    }

    #[test]
    fn keys_and_pod_ip() {
        assert_eq!(keys(r#"{"/b": 1, "/a": 2}"#), "/a\n/b");
        assert_eq!(keys("not json"), "not json");
        let pods = json!({"items": [{"metadata": {"name": "other"}, "status": {"podIP": "10.0.0.9"}},
            {"metadata": {"name": "ironprom-0"}, "status": {"podIP": "10.0.0.7"}}]})
        .to_string();
        assert_eq!(pod_ip(&pods).as_deref(), Some("10.0.0.7"));
        assert_eq!(pod_ip(r#"{"items": []}"#), None);
    }

    #[test]
    fn lines_and_head() {
        assert_eq!(lines_starting("machine_a 1\nx 2\ncadvisor_version_info 3\n", &["cadvisor_version_info", "machine_"], 40), "machine_a 1\ncadvisor_version_info 3");
        assert_eq!(head("héllo", 2), "h");
        assert!(known("network") && known("stormblock-csi") && !known("fastetcd"));
    }
}
