//! `pob agent <harness>`: the harness in the default container (docs/agent.md,
//! "The default container"), or with `--no-container` in this terminal.
//!
//! Started from here, on every platform:
//!
//! ```text
//! agent container (internal network, no route out)
//!   HTTPS_PROXY=pob-proxy:3128 ──▶ proxy container ──▶ the harness's hosts
//!   http://pob-proxy:8731/mcp  ──▶   (route)      ──▶ the host's MCP endpoint
//! ```
//!
//! The route reaches the host's endpoint (`127.0.0.1:8731`) in a way per
//! platform ([`Route`]): on Linux a Unix socket `pob agent` forwards from,
//! mounted into the proxy alone; on macOS gvproxy's `host.containers.internal`,
//! which lands on the Mac's loopback; on Windows, WSL's NAT mode, a port
//! `pob agent` forwards from on the WSL adapter's address only.
//!
//! This file holds what is decided without running anything (the
//! arguments, the configs, the platform's route), so it is tested; `run`
//! does the rest.

use std::net::IpAddr;

/// The images' build context, shipped in `pob`: the Containerfile, the
/// allowlists and the proxy's source
pub const CONTEXT: &[(&str, &str)] = &[
    (
        "Containerfile",
        include_str!("../../../container/Containerfile"),
    ),
    (
        "allow/claude.txt",
        include_str!("../../../container/allow/claude.txt"),
    ),
    (
        "allow/codex.txt",
        include_str!("../../../container/allow/codex.txt"),
    ),
    ("proxy/Cargo.toml", include_str!("../../proxy/Cargo.toml")),
    ("proxy/src/main.rs", include_str!("../../proxy/src/main.rs")),
];

/// The images' tag: a hash of the context, so a `pob` with another
/// Containerfile (other harness versions) builds its own
pub fn context_tag() -> String {
    // FNV-1a, 64 bits: a name, not a check
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for (path, text) in CONTEXT {
        for b in path.bytes().chain([0]).chain(text.bytes()).chain([0]) {
            h ^= b as u64;
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
    }
    format!("{h:016x}")
}

/// The proxy's port for HTTPS, and the route's to the MCP endpoint
pub const PROXY_PORT: u16 = 3128;
pub const ROUTE_PORT: u16 = 8731;
/// The MCP endpoint as the harness sees it
pub fn endpoint_url() -> String {
    format!("http://pob-proxy:{ROUTE_PORT}{}", host::http::PATH)
}

/// The defaults the container runs with (docs/agent.md)
pub const MEMORY: &str = "4g";
pub const CPUS: &str = "2";
pub const PIDS: &str = "512";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Harness {
    Claude,
    Codex,
}

impl Harness {
    pub fn parse(s: &str) -> Option<Harness> {
        match s {
            "claude" | "claude-code" => Some(Harness::Claude),
            "codex" => Some(Harness::Codex),
            _ => None,
        }
    }

    /// The name `pob agent` takes, and the volumes'
    pub fn name(self) -> &'static str {
        match self {
            Harness::Claude => "claude",
            Harness::Codex => "codex",
        }
    }

    /// The connection's name: its commits' `Source: agent <name>`, its
    /// agent directory's prefix
    pub fn connection(self) -> &'static str {
        match self {
            Harness::Claude => "claude-code",
            Harness::Codex => "codex",
        }
    }

    /// The command in the container: the harness with the endpoint, its
    /// token read from `POB_TOKEN` (set from this process's environment,
    /// so it is on no command line)
    pub fn container_command(self, extra: &[String]) -> Vec<String> {
        let url = endpoint_url();
        let mut v: Vec<String> = match self {
            // Claude Code takes the config as JSON; the shell puts the token in
            Harness::Claude => vec![
                "sh".into(),
                "-c".into(),
                format!(
                    "exec claude --mcp-config \"{{\\\"mcpServers\\\":{{\\\"pob\\\":{{\\\"type\\\":\\\"http\\\",\
                     \\\"url\\\":\\\"{url}\\\",\\\"headers\\\":{{\\\"Authorization\\\":\\\"Bearer $POB_TOKEN\\\"}}}}}}}}\" \"$@\""
                ),
                "claude".into(),
            ],
            // Codex reads a bearer token from a variable it is named
            Harness::Codex => vec![
                "codex".into(),
                "-c".into(),
                format!("mcp_servers.pob.url=\"{url}\""),
                "-c".into(),
                "mcp_servers.pob.bearer_token_env_var=\"POB_TOKEN\"".into(),
            ],
        };
        v.extend(extra.iter().cloned());
        v
    }

    /// The command with `--no-container`: the harness here, with `pob mcp`
    /// on stdio (`mcp` is `pob mcp`'s full command line)
    pub fn local_command(self, mcp: &[String], extra: &[String]) -> Vec<String> {
        let mut v = match self {
            Harness::Claude => {
                let config = serde_json::json!({ "mcpServers": { "pob": {
                    "type": "stdio", "command": mcp[0], "args": &mcp[1..] } } });
                vec![
                    "claude".to_string(),
                    "--mcp-config".into(),
                    config.to_string(),
                ]
            }
            Harness::Codex => vec![
                "codex".to_string(),
                "-c".into(),
                format!("mcp_servers.pob.command={}", serde_json::json!(mcp[0])),
                "-c".into(),
                format!("mcp_servers.pob.args={}", serde_json::json!(&mcp[1..])),
            ],
        };
        v.extend(extra.iter().cloned());
        v
    }
}

/// How the proxy reaches the host's MCP endpoint
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Route {
    /// Linux: a Unix socket `pob agent` forwards from, in a directory
    /// mounted into the proxy at `/run/pob`
    Unix,
    /// macOS: the address gvproxy maps to the Mac's loopback
    HostAlias,
    /// Windows in WSL's NAT mode: a port `pob agent` forwards from, on the
    /// Windows side of the WSL adapter (the machine's default gateway)
    Gateway(IpAddr),
}

/// Where the proxy container finds the socket of [`Route::Unix`]
pub const ROUTE_SOCKET: &str = "/run/pob/mcp.sock";

impl Route {
    /// The proxy's `--route` upstream; `port` is the forwarded port for
    /// [`Route::Gateway`], the endpoint's for [`Route::HostAlias`]
    pub fn upstream(&self, port: u16) -> String {
        match self {
            Route::Unix => format!("unix:{ROUTE_SOCKET}"),
            Route::HostAlias => format!("host.containers.internal:{port}"),
            Route::Gateway(ip) => match ip {
                IpAddr::V6(_) => format!("[{ip}]:{port}"),
                IpAddr::V4(_) => format!("{ip}:{port}"),
            },
        }
    }
}

/// Whether a `.wslconfig` turns on WSL's mirrored networking. NAT is the
/// default ("Accessing network applications with WSL", Microsoft Learn);
/// `networkingMode=mirrored` in `[wsl2]` turns mirrored on.
pub fn wsl_mirrored(wslconfig: &str) -> bool {
    let mut section = String::new();
    for line in wslconfig.lines() {
        let line = line.split(['#', ';']).next().unwrap_or_default().trim();
        if let Some(s) = line.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
            section = s.trim().to_ascii_lowercase();
        } else if section == "wsl2"
            && let Some((k, v)) = line.split_once('=')
            && k.trim().eq_ignore_ascii_case("networkingMode")
        {
            return v.trim().trim_matches('"').eq_ignore_ascii_case("mirrored");
        }
    }
    false
}

/// The default gateway in `ip route show default`'s output
pub fn default_gateway(ip_route: &str) -> Option<IpAddr> {
    ip_route.lines().find_map(|l| {
        let mut w = l.split_whitespace();
        (w.next()? == "default" && w.next()? == "via")
            .then(|| w.next()?.parse().ok())
            .flatten()
    })
}

/// The run's names: network, containers, volumes
pub struct Names {
    pub network: String,
    pub proxy: String,
    pub agent: String,
    pub login: String,
    pub scratch: String,
}

impl Names {
    pub fn new(h: Harness, run: &str) -> Names {
        Names {
            network: format!("pob-agent-{run}"),
            proxy: format!("pob-proxy-{run}"),
            agent: format!("pob-agent-{run}"),
            login: format!("pob-login-{}", h.name()),
            scratch: format!("pob-scratch-{}", h.name()),
        }
    }
}

/// The defensive defaults both containers share
fn hardening() -> Vec<String> {
    [
        "--read-only",
        "--cap-drop",
        "all",
        "--security-opt",
        "no-new-privileges",
    ]
    .map(String::from)
    .to_vec()
}

/// `podman run` for the proxy, detached, on the internal network only
/// (the network with a route out is connected after)
pub fn proxy_args(
    n: &Names,
    image: &str,
    h: Harness,
    route: &Route,
    upstream_port: u16,
    route_dir: Option<&str>,
    allow: &[String],
) -> Vec<String> {
    let mut v: Vec<String> = [
        "run",
        "-d",
        "--rm",
        "--name",
        &n.proxy,
        "--network",
        &n.network,
    ]
    .map(String::from)
    .to_vec();
    v.extend(hardening());
    v.extend(
        ["--memory", "256m", "--pids-limit", "256"]
            .map(String::from)
            .to_vec(),
    );
    match route {
        Route::Unix => {
            let dir = route_dir.expect("the Unix route's directory");
            // The proxy alone sees the socket; SELinux would refuse the
            // connection to a process outside, so the proxy runs unlabelled
            v.extend([
                "-v".into(),
                format!("{dir}:/run/pob"),
                "--security-opt".into(),
                "label=disable".into(),
            ]);
        }
        Route::HostAlias => {}
        Route::Gateway(_) => {}
    }
    v.push(image.into());
    v.extend([
        "--listen".into(),
        format!("0.0.0.0:{PROXY_PORT}"),
        "--allow".into(),
        format!("/etc/pob-proxy/{}.txt", h.name()),
        "--route".into(),
        format!("0.0.0.0:{ROUTE_PORT}={}", route.upstream(upstream_port)),
    ]);
    for a in allow {
        v.extend(["--allow-host".into(), a.clone()]);
    }
    v
}

/// `podman run` for the harness, attached to the terminal
pub fn agent_args(
    n: &Names,
    image: &str,
    proxy_ip: &str,
    tty: bool,
    command: &[String],
) -> Vec<String> {
    let mut v: Vec<String> = vec!["run".into(), "--rm".into(), "-i".into()];
    if tty {
        v.push("-t".into());
    }
    v.extend(
        [
            "--name",
            &n.agent,
            "--hostname",
            "pob-agent",
            "--network",
            &n.network,
            "--add-host",
            &format!("pob-proxy:{proxy_ip}"),
            "--env",
            "POB_TOKEN",
        ]
        .map(String::from),
    );
    v.extend(hardening());
    v.extend(
        [
            "--tmpfs",
            "/tmp:rw,nosuid,nodev,size=1g",
            "-v",
            &format!("{}:/home/node", n.login),
            "-v",
            &format!("{}:/work", n.scratch),
            "--memory",
            MEMORY,
            "--cpus",
            CPUS,
            "--pids-limit",
            PIDS,
        ]
        .map(String::from),
    );
    v.push(image.into());
    v.extend(command.iter().cloned());
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn harnesses() {
        assert_eq!(Harness::parse("claude"), Some(Harness::Claude));
        assert_eq!(Harness::parse("codex"), Some(Harness::Codex));
        assert_eq!(Harness::parse("gemini"), None);

        // Claude Code: the config is JSON once the shell has put the token in
        let c = Harness::Claude.container_command(&["--resume".into()]);
        assert_eq!(c[..2], ["sh", "-c"]);
        assert_eq!(c[3..], ["claude", "--resume"]);
        let script = c[2].replace("$POB_TOKEN", "pob_x").replace("\\\"", "\"");
        let json = script
            .strip_prefix("exec claude --mcp-config \"")
            .and_then(|s| s.strip_suffix("\" \"$@\""))
            .unwrap();
        let v: serde_json::Value = serde_json::from_str(json).unwrap();
        assert_eq!(v["mcpServers"]["pob"]["url"], "http://pob-proxy:8731/mcp");
        assert_eq!(v["mcpServers"]["pob"]["type"], "http");
        assert_eq!(
            v["mcpServers"]["pob"]["headers"]["Authorization"],
            "Bearer pob_x"
        );
        // Codex: the token's variable, never the token
        let c = Harness::Codex.container_command(&[]);
        assert_eq!(
            c,
            [
                "codex",
                "-c",
                "mcp_servers.pob.url=\"http://pob-proxy:8731/mcp\"",
                "-c",
                "mcp_servers.pob.bearer_token_env_var=\"POB_TOKEN\""
            ]
        );

        let mcp = [
            "/bin/pob".to_string(),
            "mcp".into(),
            "--build".into(),
            "RF".into(),
        ];
        let c = Harness::Claude.local_command(&mcp, &[]);
        let v: serde_json::Value = serde_json::from_str(&c[2]).unwrap();
        assert_eq!(v["mcpServers"]["pob"]["command"], "/bin/pob");
        assert_eq!(
            v["mcpServers"]["pob"]["args"],
            serde_json::json!(["mcp", "--build", "RF"])
        );
        let c = Harness::Codex.local_command(&mcp, &[]);
        assert_eq!(c[2], "mcp_servers.pob.command=\"/bin/pob\"");
        assert_eq!(c[4], "mcp_servers.pob.args=[\"mcp\",\"--build\",\"RF\"]");
    }

    #[test]
    fn wslconfig() {
        assert!(!wsl_mirrored(""));
        assert!(!wsl_mirrored("[wsl2]\nmemory=8GB\n"));
        assert!(wsl_mirrored("[wsl2]\nnetworkingMode=mirrored\n"));
        assert!(wsl_mirrored(
            "[WSL2]\r\n  networkingmode = \"Mirrored\" # yes\r\n"
        ));
        assert!(!wsl_mirrored("[wsl2]\nnetworkingMode=NAT\n"));
        assert!(!wsl_mirrored("# networkingMode=mirrored\n[wsl2]\n"));
        assert!(!wsl_mirrored("[experimental]\nnetworkingMode=mirrored\n"));
    }

    #[test]
    fn gateways() {
        let out = "default via 172.25.16.1 dev eth0 proto kernel\n172.25.16.0/20 dev eth0\n";
        assert_eq!(default_gateway(out), Some("172.25.16.1".parse().unwrap()));
        assert_eq!(default_gateway("172.25.16.0/20 dev eth0\n"), None);
        assert_eq!(default_gateway("default dev eth0\n"), None);
        let g = Route::Gateway("172.25.16.1".parse().unwrap());
        assert_eq!(g.upstream(40001), "172.25.16.1:40001");
        assert_eq!(
            Route::HostAlias.upstream(8731),
            "host.containers.internal:8731"
        );
        assert_eq!(Route::Unix.upstream(0), "unix:/run/pob/mcp.sock");
    }

    #[test]
    fn run_arguments() {
        let n = Names::new(Harness::Claude, "42");
        let p = proxy_args(
            &n,
            "pob-proxy:t",
            Harness::Claude,
            &Route::Unix,
            0,
            Some("/run/user/1000/pob-agent-42"),
            &["example.com".into()],
        );
        let s = p.join(" ");
        assert!(s.starts_with("run -d --rm --name pob-proxy-42 --network pob-agent-42 "));
        assert!(s.contains("--read-only --cap-drop all --security-opt no-new-privileges"));
        assert!(s.contains("-v /run/user/1000/pob-agent-42:/run/pob"));
        assert!(s.ends_with(
            "pob-proxy:t --listen 0.0.0.0:3128 --allow /etc/pob-proxy/claude.txt \
             --route 0.0.0.0:8731=unix:/run/pob/mcp.sock --allow-host example.com"
        ));
        // Only the Unix route mounts anything into the proxy
        let p = proxy_args(&n, "i", Harness::Codex, &Route::HostAlias, 8731, None, &[]);
        assert!(!p.contains(&"-v".to_string()));

        let a = agent_args(&n, "pob-agent:t", "10.89.0.2", true, &["claude".into()]);
        let s = a.join(" ");
        for want in [
            "--network pob-agent-42",
            "--add-host pob-proxy:10.89.0.2",
            "--env POB_TOKEN",
            "--read-only",
            "--cap-drop all",
            "--security-opt no-new-privileges",
            "--tmpfs /tmp:rw,nosuid,nodev,size=1g",
            "-v pob-login-claude:/home/node",
            "-v pob-scratch-claude:/work",
            "--memory 4g",
            "--cpus 2",
            "--pids-limit 512",
        ] {
            assert!(s.contains(want), "{want} in {s}");
        }
        assert!(s.ends_with("pob-agent:t claude"));
        // Named volumes only: nothing of the host's is mounted
        for (i, w) in a.iter().enumerate() {
            if w == "-v" {
                assert!(!a[i + 1].starts_with('/'), "{}", a[i + 1]);
            }
        }
        assert!(!s.contains("POB_TOKEN="), "the token is on no command line");
    }

    #[test]
    fn the_context() {
        let tag = context_tag();
        assert_eq!(tag.len(), 16);
        assert_eq!(tag, context_tag());
        let file = CONTEXT[0].1;
        assert!(file.contains("AS proxy") && file.contains("AS agent"));
    }
}
