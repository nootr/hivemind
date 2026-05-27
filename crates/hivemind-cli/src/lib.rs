use clap::{Parser, Subcommand};
use hivemind_core::{valid_node_id, ChatMessage, PeerInfo, PeerRecord};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    env, fs,
    path::{Path, PathBuf},
    process::{Command as ProcessCommand, Stdio},
    time::Duration,
};

const DEFAULT_NODE_URL: &str = "http://127.0.0.1:7747";
const DEFAULT_REPO_URL: &str = "https://github.com/nootr/hivemind";
const DEFAULT_INSTALL_URL: &str = "https://hivemind.jhx.app/install.sh";

#[derive(Debug, Parser, Eq, PartialEq)]
#[command(name = "hive")]
pub struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand, Eq, PartialEq)]
pub enum Command {
    Setup,
    Peers,
    Update {
        #[arg(long)]
        repo_url: Option<String>,
        #[arg(long)]
        branch: Option<String>,
        #[arg(long)]
        tag: Option<String>,
        #[arg(long)]
        rev: Option<String>,
    },
    Node {
        #[command(subcommand)]
        command: NodeCommand,
    },
    Join {
        node_url: String,
    },
    Peer {
        #[command(subcommand)]
        command: PeerCommand,
    },
    Say {
        text: String,
        #[arg(long, default_value = "default")]
        room: String,
    },
    Ask {
        text: String,
        #[arg(long, default_value = "default")]
        room: String,
        #[arg(long, default_value_t = 10)]
        wait_secs: u64,
    },
    Chat {
        #[arg(long, default_value = "default")]
        room: String,
        #[arg(long, default_value_t = 0)]
        after_ms: u64,
    },
}

#[derive(Debug, Subcommand, Eq, PartialEq)]
pub enum NodeCommand {
    Init {
        #[arg(long)]
        config: Option<PathBuf>,
        #[arg(long)]
        data_dir: Option<PathBuf>,
        #[arg(long, default_value = "0.0.0.0:7747")]
        bind_addr: String,
        #[arg(long)]
        public_url: Option<String>,
        #[arg(long)]
        force: bool,
    },
    Start {
        #[arg(long)]
        config: Option<PathBuf>,
        #[arg(long)]
        log: Option<PathBuf>,
    },
    Stop,
    Restart {
        #[arg(long)]
        config: Option<PathBuf>,
        #[arg(long)]
        log: Option<PathBuf>,
    },
    Logs {
        #[arg(long)]
        log: Option<PathBuf>,
        #[arg(long, default_value_t = 80)]
        lines: usize,
    },
    Status,
}

#[derive(Debug, Subcommand, Eq, PartialEq)]
pub enum PeerCommand {
    Trust { node_id: String },
}

#[derive(Debug, thiserror::Error)]
pub enum CliError {
    #[error("request failed: {0}")]
    Request(#[from] reqwest::Error),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("invalid node id")]
    InvalidNodeId,
    #[error("peer not found")]
    PeerNotFound,
    #[error("could not determine home directory; pass --config and --data-dir")]
    HomeDir,
    #[error("set only one of --branch, --tag or --rev")]
    MultipleUpdateRefs,
    #[error("update command failed: {0}")]
    UpdateFailed(String),
    #[error("node control failed: {0}")]
    NodeControlFailed(String),
}

#[derive(Debug, Serialize)]
struct SayRequest<'a> {
    text: &'a str,
    room: &'a str,
}

#[derive(Debug, Deserialize)]
struct NodeInfoResponse {
    node_url: String,
    node_id: String,
    #[serde(default)]
    name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct PeersResponse {
    peers: Vec<PeerRecord>,
}

#[derive(Debug, Deserialize)]
struct MessagesResponse {
    messages: Vec<ChatMessage>,
}

pub async fn run() -> Result<(), CliError> {
    let cli = Cli::parse();
    let output = execute(cli, &reqwest::Client::new()).await?;
    println!("{output}");
    Ok(())
}

pub async fn execute(cli: Cli, client: &reqwest::Client) -> Result<String, CliError> {
    match cli.command {
        Command::Setup => setup(client).await,
        Command::Peers => peers(client).await,
        Command::Update {
            repo_url,
            branch,
            tag,
            rev,
        } => update(repo_url, branch, tag, rev),
        Command::Node { command } => match command {
            NodeCommand::Init {
                config,
                data_dir,
                bind_addr,
                public_url,
                force,
            } => node_init(config, data_dir, &bind_addr, public_url.as_deref(), force),
            NodeCommand::Start { config, log } => node_start(client, config, log).await,
            NodeCommand::Stop => node_stop(),
            NodeCommand::Restart { config, log } => node_restart(client, config, log).await,
            NodeCommand::Logs { log, lines } => node_logs(log, lines),
            NodeCommand::Status => node_status(client).await,
        },
        Command::Join { node_url } => join(client, &node_url).await,
        Command::Peer { command } => match command {
            PeerCommand::Trust { node_id } => trust_peer(client, &node_id).await,
        },
        Command::Say { text, room } => say(client, &text, &room).await,
        Command::Ask {
            text,
            room,
            wait_secs,
        } => ask(client, &text, &room, wait_secs).await,
        Command::Chat { room, after_ms } => chat(client, &room, after_ms).await,
    }
}

fn update(
    repo_url: Option<String>,
    branch: Option<String>,
    tag: Option<String>,
    rev: Option<String>,
) -> Result<String, CliError> {
    if [branch.is_some(), tag.is_some(), rev.is_some()]
        .into_iter()
        .filter(|value| *value)
        .count()
        > 1
    {
        return Err(CliError::MultipleUpdateRefs);
    }

    let repo_url = repo_url
        .or_else(|| env::var("HIVEMIND_REPO_URL").ok())
        .unwrap_or_else(|| DEFAULT_REPO_URL.to_owned());
    let branch = branch.or_else(|| env::var("HIVEMIND_BRANCH").ok());
    let tag = tag.or_else(|| env::var("HIVEMIND_TAG").ok());
    let rev = rev.or_else(|| env::var("HIVEMIND_REV").ok());
    if [branch.is_some(), tag.is_some(), rev.is_some()]
        .into_iter()
        .filter(|value| *value)
        .count()
        > 1
    {
        return Err(CliError::MultipleUpdateRefs);
    }

    if branch.is_none() && rev.is_none() && repo_url == DEFAULT_REPO_URL {
        if let Err(installer_err) = update_with_installer(tag.as_deref()) {
            if command_available("cargo") && command_available("git") {
                for package in ["hivemind-cli", "hivemind-node"] {
                    update_package(package, &repo_url, None, tag.as_deref(), None)?;
                }
            } else {
                return Err(installer_err);
            }
        }
    } else {
        for package in ["hivemind-cli", "hivemind-node"] {
            update_package(
                package,
                &repo_url,
                branch.as_deref(),
                tag.as_deref(),
                rev.as_deref(),
            )?;
        }
    }
    Ok(
        "HIVEMIND updated. Restart the node if it is already running, then run `hive node status`."
            .to_owned(),
    )
}

fn update_with_installer(tag: Option<&str>) -> Result<(), CliError> {
    let install_url =
        env::var("HIVEMIND_INSTALL_URL").unwrap_or_else(|_| DEFAULT_INSTALL_URL.to_owned());
    let mut command = ProcessCommand::new("sh");
    command
        .arg("-c")
        .arg("curl -fsSL \"$HIVEMIND_INSTALL_URL\" | sh");
    command.env("HIVEMIND_INSTALL_URL", install_url);
    if let Some(tag) = tag {
        command.env("HIVEMIND_TAG", tag);
    }
    let status = command.status()?;
    if status.success() {
        Ok(())
    } else {
        Err(CliError::UpdateFailed(format!(
            "installer exited with {status}"
        )))
    }
}

fn command_available(command: &str) -> bool {
    ProcessCommand::new(command)
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

fn update_package(
    package: &str,
    repo_url: &str,
    branch: Option<&str>,
    tag: Option<&str>,
    rev: Option<&str>,
) -> Result<(), CliError> {
    let mut command = ProcessCommand::new("cargo");
    command
        .arg("install")
        .arg("--git")
        .arg(repo_url)
        .arg(package)
        .arg("--locked")
        .arg("--force");
    if let Some(branch) = branch {
        command.arg("--branch").arg(branch);
    } else if let Some(tag) = tag {
        command.arg("--tag").arg(tag);
    } else if let Some(rev) = rev {
        command.arg("--rev").arg(rev);
    }
    let status = command.status()?;
    if status.success() {
        Ok(())
    } else {
        Err(CliError::UpdateFailed(format!(
            "cargo install {package} exited with {status}"
        )))
    }
}

fn node_init(
    config: Option<PathBuf>,
    data_dir: Option<PathBuf>,
    bind_addr: &str,
    public_url: Option<&str>,
    force: bool,
) -> Result<String, CliError> {
    let (config, data_dir) = match (config, data_dir) {
        (Some(config), Some(data_dir)) => (config, data_dir),
        (config, data_dir) => {
            let home = home_dir()?;
            (
                config.unwrap_or_else(|| home.join(".hivemind/node.toml")),
                data_dir.unwrap_or_else(|| home.join(".hivemind/data")),
            )
        }
    };

    if config.exists() && !force {
        return Ok(format!(
            "node config already exists: {}\nstart node:\n  hive node start\nthen run:\n  hive setup",
            config.display()
        ));
    }

    if let Some(parent) = config.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::create_dir_all(&data_dir)?;
    let public_url_line = public_url
        .map(|url| format!("public_url = \"{}\"\n", toml_escape(url)))
        .unwrap_or_default();
    fs::write(
        &config,
        format!(
            "data_dir = \"{}\"\nbind_addr = \"{}\"\n{}",
            toml_escape_path(&data_dir),
            toml_escape(bind_addr),
            public_url_line
        ),
    )?;

    Ok(format!(
        "wrote node config: {}\nstart node:\n  hive node start\nthen run:\n  hive setup",
        config.display()
    ))
}

async fn node_start(
    client: &reqwest::Client,
    config: Option<PathBuf>,
    log: Option<PathBuf>,
) -> Result<String, CliError> {
    let home = home_dir()?;
    let pid = home.join(".hivemind/node.pid");
    let config = config.unwrap_or_else(|| home.join(".hivemind/node.toml"));
    let log = log.unwrap_or_else(|| home.join(".hivemind/node.log"));
    if node_running(client).await {
        return Ok(format!(
            "hivemind-node already running at {}\nconfig: {}\nlog: {}\nthen run:\n  hive setup",
            node_url(),
            config.display(),
            log.display()
        ));
    }
    if let Some(parent) = log.parent() {
        fs::create_dir_all(parent)?;
    }
    let stdout = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log)?;
    let stderr = stdout.try_clone()?;
    if let Some(parent) = pid.parent() {
        fs::create_dir_all(parent)?;
    }
    let child = ProcessCommand::new("hivemind-node")
        .arg("--config")
        .arg(&config)
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr))
        .spawn()?;
    fs::write(&pid, format!("{}\n", child.id()))?;
    for _ in 0..20 {
        if node_running(client).await {
            return Ok(format!(
                "started hivemind-node pid {}\nconfig: {}\nlog: {}\npid: {}\nthen run:\n  hive setup",
                child.id(),
                config.display(),
                log.display(),
                pid.display()
            ));
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    Ok(format!(
        "started hivemind-node pid {}, but it did not answer health checks yet\nconfig: {}\nlog: {}\ncheck status with:\n  hive node status",
        child.id(),
        config.display(),
        log.display()
    ))
}

fn node_stop() -> Result<String, CliError> {
    let pid_path = home_dir()?.join(".hivemind/node.pid");
    if !pid_path.exists() {
        return Ok(format!(
            "no hivemind-node pid file found at {}\nif the node is running, stop it manually or restart your shell session",
            pid_path.display()
        ));
    }
    let pid = fs::read_to_string(&pid_path)?.trim().to_owned();
    if pid.is_empty() || !pid.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(CliError::NodeControlFailed(format!(
            "invalid pid file: {}",
            pid_path.display()
        )));
    }
    if !pid_belongs_to_hivemind_node(&pid)? {
        let _ = fs::remove_file(&pid_path);
        return Ok(format!(
            "removed stale pid file {}; pid {pid} is not a running hivemind-node",
            pid_path.display()
        ));
    }
    let status = stop_pid(&pid)?;
    if status.success() {
        let _ = fs::remove_file(&pid_path);
        Ok(format!("stopped hivemind-node pid {pid}"))
    } else {
        Err(CliError::NodeControlFailed(format!(
            "failed to stop hivemind-node pid {pid}: {status}"
        )))
    }
}

#[cfg(unix)]
fn pid_belongs_to_hivemind_node(pid: &str) -> Result<bool, CliError> {
    let output = ProcessCommand::new("ps")
        .arg("-p")
        .arg(pid)
        .arg("-o")
        .arg("comm=")
        .output()?;
    if !output.status.success() {
        return Ok(false);
    }
    let command = String::from_utf8_lossy(&output.stdout);
    Ok(command
        .trim()
        .rsplit('/')
        .next()
        .map(|name| name == "hivemind-node")
        .unwrap_or(false))
}

#[cfg(windows)]
fn pid_belongs_to_hivemind_node(pid: &str) -> Result<bool, CliError> {
    let output = ProcessCommand::new("tasklist")
        .arg("/FI")
        .arg(format!("PID eq {pid}"))
        .arg("/FO")
        .arg("CSV")
        .arg("/NH")
        .output()?;
    if !output.status.success() {
        return Ok(false);
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .to_ascii_lowercase()
        .contains("hivemind-node.exe"))
}

#[cfg(unix)]
fn stop_pid(pid: &str) -> Result<std::process::ExitStatus, CliError> {
    Ok(ProcessCommand::new("kill").arg(pid).status()?)
}

#[cfg(windows)]
fn stop_pid(pid: &str) -> Result<std::process::ExitStatus, CliError> {
    Ok(ProcessCommand::new("taskkill")
        .arg("/PID")
        .arg(pid)
        .arg("/F")
        .status()?)
}

async fn node_restart(
    client: &reqwest::Client,
    config: Option<PathBuf>,
    log: Option<PathBuf>,
) -> Result<String, CliError> {
    let stop_output = node_stop().unwrap_or_else(|err| format!("stop skipped: {err}"));
    tokio::time::sleep(Duration::from_millis(300)).await;
    let start_output = node_start(client, config, log).await?;
    Ok(format!("{stop_output}\n\n{start_output}"))
}

fn node_logs(log: Option<PathBuf>, lines: usize) -> Result<String, CliError> {
    let home = home_dir()?;
    let log = log.unwrap_or_else(|| home.join(".hivemind/node.log"));
    if !log.exists() {
        return Ok(format!("node log does not exist yet: {}", log.display()));
    }
    let content = fs::read_to_string(&log)?;
    let mut tail = content.lines().rev().take(lines).collect::<Vec<_>>();
    tail.reverse();
    if tail.is_empty() {
        Ok(format!("node log is empty: {}", log.display()))
    } else {
        Ok(tail.join("\n"))
    }
}

async fn node_status(client: &reqwest::Client) -> Result<String, CliError> {
    if !node_health(client).await {
        return Ok(format!(
            "hivemind-node is not reachable at {}\nstart it with:\n  hive node start",
            node_url()
        ));
    }
    match node_info(client).await {
        Ok(node) => Ok(format!(
            "hivemind-node is running\nlocal control URL: {}\nadvertised node URL: {}\nnode name: {}\nnode id: {}",
            node_url(),
            node.node_url,
            node.name.as_deref().unwrap_or("unknown"),
            node.node_id
        )),
        Err(_) => Ok(format!(
            "hivemind-node answered health at {}, but /v1/node failed",
            node_url()
        )),
    }
}

async fn node_health(client: &reqwest::Client) -> bool {
    client
        .get(format!("{}/health", node_url()))
        .send()
        .await
        .map(|response| response.status().is_success())
        .unwrap_or(false)
}

async fn node_running(client: &reqwest::Client) -> bool {
    node_health(client).await && node_info(client).await.is_ok()
}

async fn setup(client: &reqwest::Client) -> Result<String, CliError> {
    let node = node_info(client).await?;
    let peers = fetch_peers(client).await?.peers;
    let mut lines = vec![
        "Hive setup".to_owned(),
        format!("local control URL: {}", node_url()),
        format!("advertised node URL: {}", node.node_url),
        format!("node name: {}", node.name.as_deref().unwrap_or("unknown")),
        format!("node id: {}", node.node_id),
        "".to_owned(),
    ];
    if peers.is_empty() {
        lines.push(
            "No peers discovered yet. Keep the node running; it beacons and listens continuously."
                .to_owned(),
        );
    } else {
        lines.push("Discovered peer candidates:".to_owned());
        for peer in peers {
            let status = if peer.trusted { "trusted" } else { "untrusted" };
            lines.push(format_peer_line(&peer, status));
        }
    }
    lines.push("".to_owned());
    lines.push("Discovery is not trust. Compare node IDs out-of-band, then run:".to_owned());
    lines.push("  hive peer trust <node-id>".to_owned());
    Ok(lines.join("\n"))
}

async fn join(client: &reqwest::Client, remote_node_url: &str) -> Result<String, CliError> {
    let remote_url = remote_node_url.trim_end_matches('/');
    let local = node_info(client).await?;
    let remote = client
        .post(format!("{remote_url}/v1/join"))
        .json(&PeerInfo {
            node_url: local.node_url,
            node_id: local.node_id,
            name: local.name,
        })
        .send()
        .await?
        .error_for_status()?
        .json::<PeersResponse>()
        .await?;

    let mut joined = 0;
    for peer in remote.peers {
        let response = client
            .post(format!("{}/v1/peers", node_url()))
            .json(&PeerInfo {
                node_url: peer.node_url,
                node_id: peer.node_id,
                name: peer.name,
            })
            .send()
            .await?;
        if response.status().is_success() {
            joined += 1;
        }
    }
    Ok(format!(
        "joined peer network via {remote_url}; imported {joined} untrusted peer candidates"
    ))
}

async fn peers(client: &reqwest::Client) -> Result<String, CliError> {
    let peers = fetch_peers(client).await?.peers;
    if peers.is_empty() {
        return Ok("no peers discovered yet".to_owned());
    }
    Ok(peers
        .into_iter()
        .map(|peer| {
            let status = if peer.trusted { "trusted" } else { "untrusted" };
            format_peer_line(&peer, status)
        })
        .collect::<Vec<_>>()
        .join("\n"))
}

async fn trust_peer(client: &reqwest::Client, node_id: &str) -> Result<String, CliError> {
    if !valid_node_id(node_id) {
        return Err(CliError::InvalidNodeId);
    }
    let response = client
        .post(format!("{}/v1/peers/{node_id}/trust", node_url()))
        .send()
        .await?;
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return Err(CliError::PeerNotFound);
    }
    let peer = response.error_for_status()?.json::<PeerRecord>().await?;
    Ok(format!(
        "trusted {} {} ({})",
        peer.name.as_deref().unwrap_or("unknown"),
        peer.node_url,
        peer.node_id
    ))
}

async fn say(client: &reqwest::Client, text: &str, room: &str) -> Result<String, CliError> {
    let message = client
        .post(format!("{}/v1/chat", node_url()))
        .json(&SayRequest { text, room })
        .send()
        .await?
        .error_for_status()?
        .json::<ChatMessage>()
        .await?;
    Ok(format!("sent {}", message.id))
}

async fn ask(
    client: &reqwest::Client,
    text: &str,
    room: &str,
    wait_secs: u64,
) -> Result<String, CliError> {
    let since = now_ms();
    let sent = say(client, text, room).await?;
    tokio::time::sleep(Duration::from_secs(wait_secs)).await;
    let replies = fetch_messages(client, room, since).await?.messages;
    let authors = author_trust(client).await?;
    let mut lines = vec![sent, "".to_owned(), "Replies:".to_owned()];
    for reply in replies.into_iter().filter(|message| message.text != text) {
        lines.push(format_message(&reply, &authors));
    }
    Ok(lines.join("\n"))
}

async fn chat(client: &reqwest::Client, room: &str, after_ms: u64) -> Result<String, CliError> {
    let messages = fetch_messages(client, room, after_ms).await?.messages;
    if messages.is_empty() {
        return Ok("no messages".to_owned());
    }
    let authors = author_trust(client).await?;
    Ok(messages
        .into_iter()
        .map(|message| {
            format!(
                "{} {}",
                message.created_at_ms,
                format_message(&message, &authors)
            )
        })
        .collect::<Vec<_>>()
        .join("\n"))
}

async fn node_info(client: &reqwest::Client) -> Result<NodeInfoResponse, CliError> {
    Ok(client
        .get(format!("{}/v1/node", node_url()))
        .send()
        .await?
        .error_for_status()?
        .json::<NodeInfoResponse>()
        .await?)
}

async fn fetch_peers(client: &reqwest::Client) -> Result<PeersResponse, CliError> {
    Ok(client
        .get(format!("{}/v1/peers", node_url()))
        .send()
        .await?
        .error_for_status()?
        .json::<PeersResponse>()
        .await?)
}

async fn fetch_messages(
    client: &reqwest::Client,
    room: &str,
    after_ms: u64,
) -> Result<MessagesResponse, CliError> {
    Ok(client
        .get(format!(
            "{}/v1/chat?room={}&after_ms={}",
            node_url(),
            room,
            after_ms
        ))
        .send()
        .await?
        .error_for_status()?
        .json::<MessagesResponse>()
        .await?)
}

async fn author_trust(client: &reqwest::Client) -> Result<BTreeMap<String, String>, CliError> {
    let mut authors = BTreeMap::new();
    let node = node_info(client).await?;
    authors.insert(node.node_id, "self".to_owned());
    for peer in fetch_peers(client).await?.peers {
        let label = if peer.trusted { "trusted" } else { "untrusted" };
        authors.insert(peer.node_id, label.to_owned());
    }
    Ok(authors)
}

fn format_peer_line(peer: &PeerRecord, status: &str) -> String {
    format!(
        "{status}\tname={}\turl={}\tshort={}\tnode_id={}\tsource={}\tlast_seen_ms={}",
        peer.name.as_deref().unwrap_or("unknown"),
        peer.node_url,
        short_id(&peer.node_id),
        peer.node_id,
        peer.source,
        peer.last_seen_ms
    )
}

fn format_message(message: &ChatMessage, authors: &BTreeMap<String, String>) -> String {
    let label = authors
        .get(&message.author_node_id)
        .map(String::as_str)
        .unwrap_or("untrusted");
    format!(
        "[{}] {}: {}",
        label,
        short_id(&message.author_node_id),
        message.text
    )
}

fn home_dir() -> Result<PathBuf, CliError> {
    env::var_os("HOME")
        .or_else(|| env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .ok_or(CliError::HomeDir)
}

fn toml_escape_path(path: &Path) -> String {
    toml_escape(&path.display().to_string())
}

fn toml_escape(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

fn node_url() -> String {
    env::var("HIVEMIND_NODE_URL").unwrap_or_else(|_| DEFAULT_NODE_URL.to_owned())
}

fn short_id(node_id: &str) -> String {
    node_id.chars().take(8).collect()
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

pub fn write_example_config(path: &PathBuf, port: u16) -> Result<(), CliError> {
    fs::write(
        path,
        format!("data_dir = \"./data-{port}\"\nbind_addr = \"0.0.0.0:{port}\"\n"),
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn parses_setup() {
        assert_eq!(
            Cli::parse_from(["hive", "setup"]),
            Cli {
                command: Command::Setup
            }
        );
    }

    #[test]
    fn parses_update() {
        assert_eq!(
            Cli::parse_from([
                "hive",
                "update",
                "--repo-url",
                "https://github.com/nootr/hivemind",
                "--branch",
                "main"
            ]),
            Cli {
                command: Command::Update {
                    repo_url: Some("https://github.com/nootr/hivemind".to_owned()),
                    branch: Some("main".to_owned()),
                    tag: None,
                    rev: None,
                }
            }
        );
    }

    #[test]
    fn parses_say() {
        assert_eq!(
            Cli::parse_from(["hive", "say", "hello", "--room", "ops"]),
            Cli {
                command: Command::Say {
                    text: "hello".to_owned(),
                    room: "ops".to_owned(),
                }
            }
        );
    }

    #[test]
    fn parses_node_init() {
        assert_eq!(
            Cli::parse_from([
                "hive",
                "node",
                "init",
                "--public-url",
                "http://127.0.0.1:18888"
            ]),
            Cli {
                command: Command::Node {
                    command: NodeCommand::Init {
                        config: None,
                        data_dir: None,
                        bind_addr: "0.0.0.0:7747".to_owned(),
                        public_url: Some("http://127.0.0.1:18888".to_owned()),
                        force: false,
                    }
                }
            }
        );
    }

    #[test]
    fn parses_node_start() {
        assert_eq!(
            Cli::parse_from([
                "hive",
                "node",
                "start",
                "--config",
                "/tmp/node.toml",
                "--log",
                "/tmp/node.log"
            ]),
            Cli {
                command: Command::Node {
                    command: NodeCommand::Start {
                        config: Some(PathBuf::from("/tmp/node.toml")),
                        log: Some(PathBuf::from("/tmp/node.log")),
                    }
                }
            }
        );
    }

    #[test]
    fn parses_node_stop() {
        assert_eq!(
            Cli::parse_from(["hive", "node", "stop"]),
            Cli {
                command: Command::Node {
                    command: NodeCommand::Stop
                }
            }
        );
    }

    #[test]
    fn parses_node_restart() {
        assert_eq!(
            Cli::parse_from(["hive", "node", "restart"]),
            Cli {
                command: Command::Node {
                    command: NodeCommand::Restart {
                        config: None,
                        log: None,
                    }
                }
            }
        );
    }

    #[test]
    fn parses_node_logs() {
        assert_eq!(
            Cli::parse_from(["hive", "node", "logs", "--lines", "20"]),
            Cli {
                command: Command::Node {
                    command: NodeCommand::Logs {
                        log: None,
                        lines: 20,
                    }
                }
            }
        );
    }

    #[test]
    fn default_install_url_uses_custom_domain() {
        assert_eq!(DEFAULT_INSTALL_URL, "https://hivemind.jhx.app/install.sh");
    }

    #[test]
    fn parses_node_status() {
        assert_eq!(
            Cli::parse_from(["hive", "node", "status"]),
            Cli {
                command: Command::Node {
                    command: NodeCommand::Status
                }
            }
        );
    }

    #[test]
    fn node_init_writes_config() {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("node.toml");
        let data_dir = dir.path().join("data");
        let output = node_init(
            Some(config.clone()),
            Some(data_dir.clone()),
            "127.0.0.1:18888",
            Some("http://127.0.0.1:18888"),
            false,
        )
        .unwrap();
        assert!(output.contains("hive node start"));
        let written = fs::read_to_string(config).unwrap();
        assert!(written.contains(&format!("data_dir = \"{}\"", data_dir.display())));
        assert!(written.contains("bind_addr = \"127.0.0.1:18888\""));
        assert!(written.contains("public_url = \"http://127.0.0.1:18888\""));
    }

    #[test]
    fn node_init_omits_public_url_by_default() {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("node.toml");
        let data_dir = dir.path().join("data");
        node_init(
            Some(config.clone()),
            Some(data_dir),
            "127.0.0.1:18888",
            None,
            false,
        )
        .unwrap();
        let written = fs::read_to_string(config).unwrap();
        assert!(!written.contains("public_url ="));
    }

    #[test]
    fn parses_join() {
        assert_eq!(
            Cli::parse_from(["hive", "join", "http://127.0.0.1:17748"]),
            Cli {
                command: Command::Join {
                    node_url: "http://127.0.0.1:17748".to_owned(),
                }
            }
        );
    }

    #[test]
    fn parses_peer_trust() {
        assert_eq!(
            Cli::parse_from(["hive", "peer", "trust", &"a".repeat(64)]),
            Cli {
                command: Command::Peer {
                    command: PeerCommand::Trust {
                        node_id: "a".repeat(64),
                    }
                }
            }
        );
    }

    #[test]
    fn formats_message_with_trust_label() {
        let mut authors = BTreeMap::new();
        authors.insert("a".repeat(64), "trusted".to_owned());
        let message = ChatMessage {
            id: "id".to_owned(),
            room: "default".to_owned(),
            author_node_id: "a".repeat(64),
            created_at_ms: 1,
            text: "hello".to_owned(),
            signature: "sig".to_owned(),
        };
        assert_eq!(
            format_message(&message, &authors),
            "[trusted] aaaaaaaa: hello"
        );
    }

    #[test]
    fn shortens_node_id() {
        assert_eq!(short_id("abcdef123456"), "abcdef12");
    }
}
