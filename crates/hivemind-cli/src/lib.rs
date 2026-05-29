use clap::{Parser, Subcommand};
use hivemind_core::{
    encode_message_text, split_message_text, valid_node_id, AgentRecord, ChatMessage,
    DeliveryRecord, MessageKind, MessageMeta, PeerInfo, PeerRecord, PeerTrustState, ReceiptAction,
};
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
#[command(
    name = "hive",
    disable_version_flag = true,
    after_help = "Version: hive --version | hive -v"
)]
pub struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand, Eq, PartialEq)]
pub enum Command {
    Setup,
    Peers,
    Agents,
    Agent {
        #[command(subcommand)]
        command: AgentCommand,
    },
    Deliveries {
        message_id: String,
    },
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
        #[arg(long)]
        reply_to: Option<String>,
    },
    Answer {
        message_id: String,
        text: String,
        #[arg(long, default_value = "default")]
        room: String,
    },
    Ask {
        text: String,
        #[arg(long, default_value = "default")]
        room: String,
        #[arg(long, default_value_t = 30)]
        wait_secs: u64,
    },
    Chat {
        #[arg(long, default_value = "default")]
        room: String,
        #[arg(long, default_value_t = 0)]
        after_ms: u64,
        #[arg(short = 'f', long = "follow")]
        follow: bool,
        #[arg(long, default_value_t = 2)]
        interval_secs: u64,
    },
    Inbox {
        #[arg(long, default_value = "default")]
        room: String,
        #[arg(long)]
        all: bool,
    },
    Read {
        message_id: String,
        #[arg(long)]
        agent: String,
        #[arg(long, default_value = "default")]
        room: String,
    },
    Claim {
        message_id: String,
        #[arg(long)]
        agent: String,
        #[arg(long, default_value = "default")]
        room: String,
    },
    Done {
        message_id: String,
        #[arg(long)]
        agent: String,
        #[arg(long, default_value = "default")]
        room: String,
    },
    Decline {
        message_id: String,
        #[arg(long)]
        agent: String,
        #[arg(long)]
        reason: Option<String>,
        #[arg(long, default_value = "default")]
        room: String,
    },
    Watch {
        #[arg(long)]
        agent: String,
        #[arg(long)]
        agent_id: Option<String>,
        #[arg(long, default_value = "")]
        capabilities: String,
        #[arg(long, default_value = "default")]
        room: String,
        #[arg(long)]
        after_ms: Option<u64>,
        #[arg(long, default_value_t = 10)]
        interval_secs: u64,
        #[arg(long, default_value_t = 30)]
        heartbeat_secs: u64,
        #[arg(long, default_value_t = 120)]
        ttl_secs: u64,
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
    Deny { node_id: String },
}

#[derive(Debug, Subcommand, Eq, PartialEq)]
pub enum AgentCommand {
    Heartbeat {
        #[arg(long)]
        name: String,
        #[arg(long)]
        agent_id: Option<String>,
        #[arg(long, default_value = "")]
        capabilities: String,
        #[arg(long, default_value_t = 120)]
        ttl_secs: u64,
    },
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

#[derive(Debug, Serialize)]
struct AgentHeartbeatRequest<'a> {
    agent_id: Option<&'a str>,
    name: &'a str,
    capabilities: Vec<String>,
    ttl_secs: u64,
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

#[derive(Debug, Deserialize)]
struct DeliveriesResponse {
    deliveries: Vec<DeliveryRecord>,
}

#[derive(Debug, Deserialize)]
struct AgentsResponse {
    agents: Vec<AgentRecord>,
}

pub async fn run() -> Result<(), CliError> {
    if version_requested(env::args()) {
        println!("hive {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    let cli = Cli::parse();
    let output = execute(cli, &reqwest::Client::new()).await?;
    println!("{output}");
    Ok(())
}

fn version_requested<I, S>(args: I) -> bool
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let args = args
        .into_iter()
        .map(|arg| arg.as_ref().to_owned())
        .collect::<Vec<_>>();
    matches!(args.as_slice(), [_, flag] if flag == "-v" || flag == "--version")
}

pub async fn execute(cli: Cli, client: &reqwest::Client) -> Result<String, CliError> {
    match cli.command {
        Command::Setup => setup(client).await,
        Command::Peers => peers(client).await,
        Command::Agents => agents(client).await,
        Command::Agent { command } => match command {
            AgentCommand::Heartbeat {
                name,
                agent_id,
                capabilities,
                ttl_secs,
            } => agent_heartbeat(client, &name, agent_id.as_deref(), &capabilities, ttl_secs).await,
        },
        Command::Deliveries { message_id } => deliveries(client, &message_id).await,
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
            PeerCommand::Deny { node_id } => deny_peer(client, &node_id).await,
        },
        Command::Say {
            text,
            room,
            reply_to,
        } => say(client, &text, &room, reply_to.as_deref()).await,
        Command::Answer {
            message_id,
            text,
            room,
        } => answer(client, &message_id, &text, &room).await,
        Command::Ask {
            text,
            room,
            wait_secs,
        } => ask(client, &text, &room, wait_secs).await,
        Command::Chat {
            room,
            after_ms,
            follow,
            interval_secs,
        } => {
            if follow {
                chat_follow(client, &room, after_ms, interval_secs).await
            } else {
                chat(client, &room, after_ms).await
            }
        }
        Command::Inbox { room, all } => inbox(client, &room, all).await,
        Command::Read {
            message_id,
            agent,
            room,
        } => {
            receipt(
                client,
                ReceiptAction::Read,
                &message_id,
                &agent,
                None,
                &room,
            )
            .await
        }
        Command::Claim {
            message_id,
            agent,
            room,
        } => {
            receipt(
                client,
                ReceiptAction::Claim,
                &message_id,
                &agent,
                None,
                &room,
            )
            .await
        }
        Command::Done {
            message_id,
            agent,
            room,
        } => {
            receipt(
                client,
                ReceiptAction::Done,
                &message_id,
                &agent,
                None,
                &room,
            )
            .await
        }
        Command::Decline {
            message_id,
            agent,
            reason,
            room,
        } => {
            receipt(
                client,
                ReceiptAction::Decline,
                &message_id,
                &agent,
                reason.as_deref(),
                &room,
            )
            .await
        }
        Command::Watch {
            agent,
            agent_id,
            capabilities,
            room,
            after_ms,
            interval_secs,
            heartbeat_secs,
            ttl_secs,
        } => {
            watch(
                client,
                WatchOptions {
                    agent: &agent,
                    agent_id: agent_id.as_deref(),
                    capabilities: &capabilities,
                    room: &room,
                    after_ms,
                    interval_secs,
                    heartbeat_secs,
                    ttl_secs,
                },
            )
            .await
        }
    }
}

struct WatchOptions<'a> {
    agent: &'a str,
    agent_id: Option<&'a str>,
    capabilities: &'a str,
    room: &'a str,
    after_ms: Option<u64>,
    interval_secs: u64,
    heartbeat_secs: u64,
    ttl_secs: u64,
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
            lines.push(format_peer_line(&peer));
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
        "joined peer network via {remote_url}; imported {joined} unknown peer candidates"
    ))
}

async fn peers(client: &reqwest::Client) -> Result<String, CliError> {
    let peers = fetch_peers(client).await?.peers;
    if peers.is_empty() {
        return Ok("no peers discovered yet".to_owned());
    }
    Ok(peers
        .into_iter()
        .map(|peer| format_peer_line(&peer))
        .collect::<Vec<_>>()
        .join("\n"))
}

async fn deliveries(client: &reqwest::Client, message_id: &str) -> Result<String, CliError> {
    let deliveries = fetch_deliveries(client, message_id).await?.deliveries;
    if deliveries.is_empty() {
        return Ok("no delivery records".to_owned());
    }
    Ok(deliveries
        .into_iter()
        .map(|delivery| format_delivery_line(&delivery))
        .collect::<Vec<_>>()
        .join("\n"))
}

async fn agent_heartbeat(
    client: &reqwest::Client,
    name: &str,
    agent_id: Option<&str>,
    capabilities: &str,
    ttl_secs: u64,
) -> Result<String, CliError> {
    let agent = heartbeat_agent(client, name, agent_id, capabilities, ttl_secs).await?;
    Ok(format!(
        "agent online name={} agent_id={} node={} expires_at_ms={}",
        agent.name,
        agent.agent_id,
        short_id(&agent.node_id),
        agent.expires_at_ms
    ))
}

async fn heartbeat_agent(
    client: &reqwest::Client,
    name: &str,
    agent_id: Option<&str>,
    capabilities: &str,
    ttl_secs: u64,
) -> Result<AgentRecord, CliError> {
    let capabilities = parse_capabilities(capabilities);
    Ok(client
        .post(format!("{}/v1/agents/heartbeat", node_url()))
        .json(&AgentHeartbeatRequest {
            agent_id,
            name,
            capabilities,
            ttl_secs,
        })
        .send()
        .await?
        .error_for_status()?
        .json::<AgentRecord>()
        .await?)
}

async fn agents(client: &reqwest::Client) -> Result<String, CliError> {
    let agent_views = aggregate_agents(client).await?;
    if agent_views.is_empty() {
        return Ok("no agents seen".to_owned());
    }
    Ok(agent_views
        .into_iter()
        .map(|view| format_agent_line(&view))
        .collect::<Vec<_>>()
        .join("\n"))
}

async fn trust_peer(client: &reqwest::Client, node_id: &str) -> Result<String, CliError> {
    update_peer_state(client, node_id, "trust", "trusted").await
}

async fn deny_peer(client: &reqwest::Client, node_id: &str) -> Result<String, CliError> {
    update_peer_state(client, node_id, "deny", "blocked").await
}

async fn update_peer_state(
    client: &reqwest::Client,
    node_id: &str,
    route: &str,
    label: &str,
) -> Result<String, CliError> {
    if !valid_node_id(node_id) {
        return Err(CliError::InvalidNodeId);
    }
    let response = client
        .post(format!("{}/v1/peers/{node_id}/{route}", node_url()))
        .send()
        .await?;
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return Err(CliError::PeerNotFound);
    }
    let peer = response.error_for_status()?.json::<PeerRecord>().await?;
    Ok(format!(
        "{label} {} {} ({})",
        peer.name.as_deref().unwrap_or("unknown"),
        peer_url_label(&peer),
        peer.node_id
    ))
}

async fn send_message(
    client: &reqwest::Client,
    text: &str,
    room: &str,
) -> Result<ChatMessage, CliError> {
    Ok(client
        .post(format!("{}/v1/chat", node_url()))
        .json(&SayRequest { text, room })
        .send()
        .await?
        .error_for_status()?
        .json::<ChatMessage>()
        .await?)
}

fn encode_meta_text(meta: MessageMeta, body: &str) -> Result<String, CliError> {
    encode_message_text(&meta, body).map_err(|err| CliError::NodeControlFailed(err.to_string()))
}

async fn say(
    client: &reqwest::Client,
    text: &str,
    room: &str,
    reply_to: Option<&str>,
) -> Result<String, CliError> {
    let text = if let Some(reply_to) = reply_to {
        if !valid_message_id(reply_to) {
            return Err(CliError::NodeControlFailed("invalid message id".to_owned()));
        }
        encode_meta_text(
            MessageMeta {
                kind: MessageKind::Answer,
                reply_to: Some(reply_to.to_owned()),
                action: None,
                agent: None,
                note: None,
            },
            text,
        )?
    } else {
        text.to_owned()
    };
    let message = send_message(client, &text, room).await?;
    Ok(format!("sent {}", message.id))
}

async fn answer(
    client: &reqwest::Client,
    message_id: &str,
    text: &str,
    room: &str,
) -> Result<String, CliError> {
    say(client, text, room, Some(message_id)).await
}

async fn receipt(
    client: &reqwest::Client,
    action: ReceiptAction,
    message_id: &str,
    agent: &str,
    note: Option<&str>,
    room: &str,
) -> Result<String, CliError> {
    if !valid_message_id(message_id) {
        return Err(CliError::NodeControlFailed("invalid message id".to_owned()));
    }
    let body = match note {
        Some(note) if !note.trim().is_empty() => {
            format!(
                "{agent} {} {}: {note}",
                action.as_str(),
                short_id(message_id)
            )
        }
        _ => format!("{agent} {} {}", action.as_str(), short_id(message_id)),
    };
    let text = encode_meta_text(
        MessageMeta {
            kind: MessageKind::Receipt,
            reply_to: Some(message_id.to_owned()),
            action: Some(action),
            agent: Some(agent.to_owned()),
            note: note.map(str::to_owned),
        },
        &body,
    )?;
    let message = send_message(client, &text, room).await?;
    Ok(format!(
        "{} {} via {}",
        action.as_str(),
        message_id,
        message.id
    ))
}

async fn ask(
    client: &reqwest::Client,
    text: &str,
    room: &str,
    wait_secs: u64,
) -> Result<String, CliError> {
    let since = now_ms();
    let trusted = trusted_peers(client).await?;
    let question_text = encode_meta_text(
        MessageMeta {
            kind: MessageKind::Question,
            reply_to: None,
            action: None,
            agent: None,
            note: None,
        },
        text,
    )?;
    let message = send_message(client, &question_text, room).await?;
    tokio::time::sleep(Duration::from_millis(500)).await;
    let initial_deliveries = fetch_deliveries(client, &message.id).await?.deliveries;
    let local_node_id = node_info(client).await?.node_id;
    let agent_views = aggregate_agents(client).await?;
    let active_agents = agent_views
        .iter()
        .filter(|view| view.agent.node_id != local_node_id && view.agent.expires_at_ms > now_ms())
        .count();

    let mut lines = vec![
        format!("sent {}", message.id),
        format!("trusted nodes: {}", trusted.len()),
        format!(
            "delivered nodes: {}/{}",
            delivered_count(&initial_deliveries),
            trusted.len()
        ),
        format!("active responder agents: {}", active_agents),
        "".to_owned(),
        "Deliveries:".to_owned(),
    ];
    lines.extend(format_delivery_summary(&trusted, &initial_deliveries));
    lines.push("".to_owned());
    lines.push(format!("Waiting {wait_secs}s for replies..."));

    tokio::time::sleep(Duration::from_secs(wait_secs)).await;
    let question_id = message.id.clone();
    let final_deliveries = fetch_deliveries(client, &question_id).await?.deliveries;
    let replies = fetch_messages(client, room, since).await?.messages;
    let authors = author_trust(client).await?;

    lines.push("".to_owned());
    lines.push("Final deliveries:".to_owned());
    lines.extend(format_delivery_summary(&trusted, &final_deliveries));
    lines.push("".to_owned());
    lines.push("Replies:".to_owned());
    let mut reply_count = 0;
    for reply in replies.into_iter().filter(|reply| {
        reply.id != question_id
            && !matches!(
                split_message_text(&reply.text).0.map(|meta| meta.kind),
                Some(MessageKind::Receipt)
            )
    }) {
        lines.push(format_message(&reply, &authors));
        reply_count += 1;
    }
    if reply_count == 0 {
        lines.push("no replies".to_owned());
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
        .map(|message| format_chat_line(&message, &authors))
        .collect::<Vec<_>>()
        .join("\n"))
}

#[derive(Debug)]
struct InboxQuestion {
    message: ChatMessage,
    body: String,
    status: String,
    claimed_by: Vec<String>,
    read_by: Vec<String>,
    answers: usize,
    last_note: Option<String>,
}

async fn inbox(client: &reqwest::Client, room: &str, all: bool) -> Result<String, CliError> {
    let messages = fetch_messages(client, room, 0).await?.messages;
    let authors = author_trust(client).await?;
    let mut questions = BTreeMap::<String, InboxQuestion>::new();

    for message in &messages {
        let (meta, body) = split_message_text(&message.text);
        match meta.map(|meta| (meta.kind, meta)) {
            Some((MessageKind::Question, _meta)) => {
                questions.insert(
                    message.id.clone(),
                    InboxQuestion {
                        message: message.clone(),
                        body,
                        status: "open".to_owned(),
                        claimed_by: Vec::new(),
                        read_by: Vec::new(),
                        answers: 0,
                        last_note: None,
                    },
                );
            }
            Some((MessageKind::Answer, meta)) => {
                if let Some(reply_to) = meta.reply_to {
                    if let Some(question) = questions.get_mut(&reply_to) {
                        question.answers += 1;
                        if question.status != "done" {
                            question.status = "answered".to_owned();
                        }
                    }
                }
            }
            Some((MessageKind::Receipt, meta)) => {
                if let Some(reply_to) = meta.reply_to {
                    if let Some(question) = questions.get_mut(&reply_to) {
                        let agent = meta
                            .agent
                            .unwrap_or_else(|| short_id(&message.author_node_id));
                        match meta.action {
                            Some(ReceiptAction::Read) => push_unique(&mut question.read_by, agent),
                            Some(ReceiptAction::Claim) => {
                                push_unique(&mut question.claimed_by, agent);
                                if question.status == "open" {
                                    question.status = "claimed".to_owned();
                                }
                            }
                            Some(ReceiptAction::Done) => question.status = "done".to_owned(),
                            Some(ReceiptAction::Decline) => {
                                if question.status == "open" || question.status == "claimed" {
                                    question.status = "declined".to_owned();
                                }
                            }
                            None => {}
                        }
                        if let Some(note) = meta.note {
                            question.last_note = Some(note);
                        }
                    }
                }
            }
            None => {}
        }
    }

    let mut items = questions.into_values().collect::<Vec<_>>();
    items.sort_by_key(|item| item.message.created_at_ms);
    if !all {
        items.retain(|item| matches!(item.status.as_str(), "open" | "claimed" | "declined"));
    }
    if items.is_empty() {
        return Ok("inbox empty".to_owned());
    }
    Ok(items
        .into_iter()
        .map(|item| format_inbox_item(&item, &authors))
        .collect::<Vec<_>>()
        .join("\n"))
}

async fn chat_follow(
    client: &reqwest::Client,
    room: &str,
    after_ms: u64,
    interval_secs: u64,
) -> Result<String, CliError> {
    if interval_secs == 0 {
        return Err(CliError::NodeControlFailed(
            "follow interval must be greater than zero".to_owned(),
        ));
    }
    let mut last_seen_ms = after_ms;
    loop {
        let messages = fetch_messages(client, room, last_seen_ms).await?.messages;
        if !messages.is_empty() {
            let authors = author_trust(client).await?;
            for message in messages {
                last_seen_ms = last_seen_ms.max(message.created_at_ms);
                println!("{}", format_chat_line(&message, &authors));
            }
        }
        tokio::time::sleep(Duration::from_secs(interval_secs)).await;
    }
}

async fn watch(client: &reqwest::Client, options: WatchOptions<'_>) -> Result<String, CliError> {
    if options.interval_secs == 0 || options.heartbeat_secs == 0 || options.ttl_secs == 0 {
        return Err(CliError::NodeControlFailed(
            "watch intervals and ttl must be greater than zero".to_owned(),
        ));
    }
    let agent = heartbeat_agent(
        client,
        options.agent,
        options.agent_id,
        options.capabilities,
        options.ttl_secs,
    )
    .await?;
    let mut last_seen_ms = options.after_ms.unwrap_or_else(now_ms);
    println!(
        "watching room={} as {} ({}) node={} after_ms={}",
        options.room,
        agent.name,
        agent.agent_id,
        short_id(&agent.node_id),
        last_seen_ms
    );
    let mut heartbeat_elapsed = 0_u64;
    loop {
        tokio::time::sleep(Duration::from_secs(options.interval_secs)).await;
        heartbeat_elapsed = heartbeat_elapsed.saturating_add(options.interval_secs);
        if heartbeat_elapsed >= options.heartbeat_secs {
            heartbeat_agent(
                client,
                options.agent,
                options.agent_id,
                options.capabilities,
                options.ttl_secs,
            )
            .await?;
            heartbeat_elapsed = 0;
        }
        let messages = fetch_messages(client, options.room, last_seen_ms)
            .await?
            .messages;
        if messages.is_empty() {
            continue;
        }
        let authors = author_trust(client).await?;
        for message in messages {
            last_seen_ms = last_seen_ms.max(message.created_at_ms);
            println!("{}", format_chat_line(&message, &authors));
        }
    }
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

async fn fetch_deliveries(
    client: &reqwest::Client,
    message_id: &str,
) -> Result<DeliveriesResponse, CliError> {
    Ok(client
        .get(format!("{}/v1/deliveries/{message_id}", node_url()))
        .send()
        .await?
        .error_for_status()?
        .json::<DeliveriesResponse>()
        .await?)
}

async fn fetch_agents_from(
    client: &reqwest::Client,
    base_url: &str,
) -> Result<AgentsResponse, CliError> {
    let response = tokio::time::timeout(
        Duration::from_secs(2),
        client
            .get(format!("{}/v1/agents", base_url.trim_end_matches('/')))
            .send(),
    )
    .await
    .map_err(|err| CliError::NodeControlFailed(format!("agent fetch timeout: {err}")))??;
    Ok(response
        .error_for_status()?
        .json::<AgentsResponse>()
        .await?)
}

async fn trusted_peers(client: &reqwest::Client) -> Result<Vec<PeerRecord>, CliError> {
    Ok(fetch_peers(client)
        .await?
        .peers
        .into_iter()
        .filter(|peer| peer.trust_state == PeerTrustState::Trusted)
        .collect())
}

#[derive(Debug)]
struct AgentView {
    agent: AgentRecord,
    node_url: String,
}

async fn aggregate_agents(client: &reqwest::Client) -> Result<Vec<AgentView>, CliError> {
    let mut views = Vec::new();
    if let Ok(response) = fetch_agents_from(client, &node_url()).await {
        views.extend(response.agents.into_iter().map(|agent| AgentView {
            agent,
            node_url: node_url(),
        }));
    }
    for peer in trusted_peers(client).await? {
        if let Ok(response) = fetch_agents_from(client, &peer.node_url).await {
            views.extend(response.agents.into_iter().map(|agent| AgentView {
                agent,
                node_url: peer.node_url.clone(),
            }));
        }
    }
    views.sort_by(|a, b| {
        b.agent
            .expires_at_ms
            .cmp(&a.agent.expires_at_ms)
            .then_with(|| a.agent.name.cmp(&b.agent.name))
            .then_with(|| a.agent.agent_id.cmp(&b.agent.agent_id))
    });
    Ok(views)
}

async fn author_trust(client: &reqwest::Client) -> Result<BTreeMap<String, String>, CliError> {
    let mut authors = BTreeMap::new();
    let node = node_info(client).await?;
    authors.insert(node.node_id, "self".to_owned());
    for peer in fetch_peers(client).await?.peers {
        authors.insert(peer.node_id, peer.trust_state.as_str().to_owned());
    }
    Ok(authors)
}

fn format_peer_line(peer: &PeerRecord) -> String {
    format!(
        "{}\tname={}\turl={}\tshort={}\tnode_id={}\tsource={}\tlast_seen_ms={}",
        peer.trust_state.as_str(),
        peer.name.as_deref().unwrap_or("unknown"),
        peer_url_label(peer),
        short_id(&peer.node_id),
        peer.node_id,
        peer.source,
        peer.last_seen_ms
    )
}

fn format_inbox_item(item: &InboxQuestion, authors: &BTreeMap<String, String>) -> String {
    let label = authors
        .get(&item.message.author_node_id)
        .map(String::as_str)
        .unwrap_or("unknown");
    let mut line = format!(
        "{}\tid={}\tfrom={}:{}\tat={}\t{}",
        item.status,
        short_id(&item.message.id),
        label,
        short_id(&item.message.author_node_id),
        item.message.created_at_ms,
        item.body.replace('\n', " ")
    );
    if !item.claimed_by.is_empty() {
        line.push_str(&format!("\tclaimed_by={}", item.claimed_by.join(",")));
    }
    if !item.read_by.is_empty() {
        line.push_str(&format!("\tread_by={}", item.read_by.join(",")));
    }
    if item.answers > 0 {
        line.push_str(&format!("\tanswers={}", item.answers));
    }
    if let Some(note) = &item.last_note {
        line.push_str(&format!("\tnote={}", note.replace('\n', " ")));
    }
    line
}

fn push_unique(values: &mut Vec<String>, value: String) {
    if !values.iter().any(|existing| existing == &value) {
        values.push(value);
    }
}

fn format_delivery_line(delivery: &DeliveryRecord) -> String {
    let mut line = format!(
        "{}\tpeer={}\turl={}\tattempted_at_ms={}",
        delivery.status.as_str(),
        short_id(&delivery.peer_node_id),
        delivery.peer_url,
        delivery.attempted_at_ms
    );
    if let Some(delivered_at_ms) = delivery.delivered_at_ms {
        line.push_str(&format!("\tdelivered_at_ms={delivered_at_ms}"));
    }
    if let Some(error) = &delivery.error {
        line.push_str(&format!("\terror={error}"));
    }
    line
}

fn delivered_count(deliveries: &[DeliveryRecord]) -> usize {
    deliveries
        .iter()
        .filter(|delivery| delivery.status.as_str() == "delivered")
        .count()
}

fn format_delivery_summary(trusted: &[PeerRecord], deliveries: &[DeliveryRecord]) -> Vec<String> {
    if trusted.is_empty() {
        return vec!["  no trusted peers".to_owned()];
    }
    trusted
        .iter()
        .map(|peer| {
            let delivery = deliveries
                .iter()
                .find(|delivery| delivery.peer_node_id == peer.node_id);
            match delivery {
                Some(delivery) => {
                    let mut line = format!(
                        "  {} {} {}",
                        status_mark(delivery.status.as_str()),
                        delivery.status.as_str(),
                        peer_label(peer)
                    );
                    if let Some(error) = &delivery.error {
                        line.push_str(&format!(" ({error})"));
                    }
                    line
                }
                None => format!("  ? not-recorded {}", peer_label(peer)),
            }
        })
        .collect()
}

fn status_mark(status: &str) -> &'static str {
    match status {
        "delivered" => "✓",
        "failed" => "✗",
        "pending" => "…",
        _ => "?",
    }
}

fn peer_label(peer: &PeerRecord) -> String {
    format!(
        "{} {}",
        peer.name.as_deref().unwrap_or("unknown"),
        short_id(&peer.node_id)
    )
}

fn format_agent_line(view: &AgentView) -> String {
    let now = now_ms();
    let status = if view.agent.expires_at_ms > now {
        "active"
    } else {
        "stale"
    };
    let capabilities = if view.agent.capabilities.is_empty() {
        "none".to_owned()
    } else {
        view.agent.capabilities.join(",")
    };
    format!(
        "{}\tname={}\tagent_id={}\tnode={}\turl={}\tlast_seen_ms={}\texpires_at_ms={}\tcapabilities={}",
        status,
        view.agent.name,
        view.agent.agent_id,
        short_id(&view.agent.node_id),
        view.node_url,
        view.agent.last_seen_ms,
        view.agent.expires_at_ms,
        capabilities
    )
}

fn parse_capabilities(input: &str) -> Vec<String> {
    input
        .split(',')
        .map(str::trim)
        .filter(|capability| !capability.is_empty())
        .map(str::to_owned)
        .collect()
}

fn peer_url_label(peer: &PeerRecord) -> &str {
    if peer.node_url.is_empty() {
        "unknown"
    } else {
        &peer.node_url
    }
}

fn format_chat_line(message: &ChatMessage, authors: &BTreeMap<String, String>) -> String {
    format!(
        "{} {}",
        message.created_at_ms,
        format_message(message, authors)
    )
}

fn format_message(message: &ChatMessage, authors: &BTreeMap<String, String>) -> String {
    let label = authors
        .get(&message.author_node_id)
        .map(String::as_str)
        .unwrap_or("unknown");
    let (meta, body) = split_message_text(&message.text);
    let prefix = match meta {
        Some(MessageMeta {
            kind: MessageKind::Question,
            ..
        }) => "? ".to_owned(),
        Some(MessageMeta {
            kind: MessageKind::Answer,
            reply_to: Some(reply_to),
            ..
        }) => format!("↳{} ", short_id(&reply_to)),
        Some(MessageMeta {
            kind: MessageKind::Receipt,
            action,
            reply_to,
            agent,
            ..
        }) => format!(
            "{} {}{} ",
            action.map(ReceiptAction::as_str).unwrap_or("receipt"),
            reply_to.as_deref().map(short_id).unwrap_or_default(),
            agent
                .map(|agent| format!(" by {agent}"))
                .unwrap_or_default()
        ),
        Some(_) | None => String::new(),
    };
    format!(
        "[{}] {}: {}{}",
        label,
        short_id(&message.author_node_id),
        prefix,
        body
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

fn valid_message_id(message_id: &str) -> bool {
    message_id.len() == 64 && message_id.bytes().all(|byte| byte.is_ascii_hexdigit())
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
    fn parses_agents() {
        assert_eq!(
            Cli::parse_from(["hive", "agents"]),
            Cli {
                command: Command::Agents
            }
        );
    }

    #[test]
    fn parses_agent_heartbeat() {
        assert_eq!(
            Cli::parse_from([
                "hive",
                "agent",
                "heartbeat",
                "--name",
                "Pi",
                "--agent-id",
                "pi-session",
                "--capabilities",
                "rust,review",
                "--ttl-secs",
                "300"
            ]),
            Cli {
                command: Command::Agent {
                    command: AgentCommand::Heartbeat {
                        name: "Pi".to_owned(),
                        agent_id: Some("pi-session".to_owned()),
                        capabilities: "rust,review".to_owned(),
                        ttl_secs: 300,
                    }
                }
            }
        );
    }

    #[test]
    fn parses_deliveries() {
        assert_eq!(
            Cli::parse_from(["hive", "deliveries", &"a".repeat(64)]),
            Cli {
                command: Command::Deliveries {
                    message_id: "a".repeat(64),
                }
            }
        );
    }

    #[test]
    fn detects_version_request() {
        assert!(version_requested(["hive", "-v"]));
        assert!(version_requested(["hive", "--version"]));
        assert!(!version_requested(["hive", "chat", "-f"]));
    }

    #[test]
    fn parses_chat_follow() {
        assert_eq!(
            Cli::parse_from([
                "hive",
                "chat",
                "--room",
                "ops",
                "--after-ms",
                "123",
                "--follow",
                "--interval-secs",
                "1"
            ]),
            Cli {
                command: Command::Chat {
                    room: "ops".to_owned(),
                    after_ms: 123,
                    follow: true,
                    interval_secs: 1,
                }
            }
        );
    }

    #[test]
    fn parses_watch() {
        assert_eq!(
            Cli::parse_from([
                "hive",
                "watch",
                "--agent",
                "Pi",
                "--agent-id",
                "pi-session",
                "--capabilities",
                "rust,review",
                "--room",
                "ops",
                "--after-ms",
                "123",
                "--interval-secs",
                "5",
                "--heartbeat-secs",
                "15",
                "--ttl-secs",
                "60"
            ]),
            Cli {
                command: Command::Watch {
                    agent: "Pi".to_owned(),
                    agent_id: Some("pi-session".to_owned()),
                    capabilities: "rust,review".to_owned(),
                    room: "ops".to_owned(),
                    after_ms: Some(123),
                    interval_secs: 5,
                    heartbeat_secs: 15,
                    ttl_secs: 60,
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
                    reply_to: None,
                }
            }
        );
    }

    #[test]
    fn parses_answer_and_receipts() {
        let message_id = "a".repeat(64);
        assert_eq!(
            Cli::parse_from(["hive", "answer", &message_id, "done"]),
            Cli {
                command: Command::Answer {
                    message_id: message_id.clone(),
                    text: "done".to_owned(),
                    room: "default".to_owned(),
                }
            }
        );
        assert_eq!(
            Cli::parse_from(["hive", "claim", &message_id, "--agent", "Pi"]),
            Cli {
                command: Command::Claim {
                    message_id: message_id.clone(),
                    agent: "Pi".to_owned(),
                    room: "default".to_owned(),
                }
            }
        );
        assert_eq!(
            Cli::parse_from([
                "hive",
                "decline",
                &message_id,
                "--agent",
                "Pi",
                "--reason",
                "busy"
            ]),
            Cli {
                command: Command::Decline {
                    message_id,
                    agent: "Pi".to_owned(),
                    reason: Some("busy".to_owned()),
                    room: "default".to_owned(),
                }
            }
        );
    }

    #[test]
    fn parses_inbox() {
        assert_eq!(
            Cli::parse_from(["hive", "inbox", "--room", "ops", "--all"]),
            Cli {
                command: Command::Inbox {
                    room: "ops".to_owned(),
                    all: true,
                }
            }
        );
    }

    #[test]
    fn parses_ask_with_30_second_default() {
        assert_eq!(
            Cli::parse_from(["hive", "ask", "help?", "--room", "ops"]),
            Cli {
                command: Command::Ask {
                    text: "help?".to_owned(),
                    room: "ops".to_owned(),
                    wait_secs: 30,
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
    fn parses_peer_deny() {
        assert_eq!(
            Cli::parse_from(["hive", "peer", "deny", &"a".repeat(64)]),
            Cli {
                command: Command::Peer {
                    command: PeerCommand::Deny {
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
