use base64::{engine::general_purpose::STANDARD, Engine};
use clap::{Parser, Subcommand};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs,
    net::{Ipv4Addr, SocketAddr, UdpSocket},
    path::PathBuf,
    time::{Duration, Instant},
};

const DEFAULT_NODE_URL: &str = "http://127.0.0.1:7747";
const DEFAULT_CONFIG_RELATIVE_PATH: &str = ".config/hivemind/hive.json";
const DISCOVERY_PORT: u16 = 7748;
const DISCOVERY_QUERY: &[u8] = b"HIVEMIND_DISCOVER_V1";
const DISCOVERY_RESPONSE_PREFIX: &str = "HIVEMIND_NODE_V1 ";

#[derive(Debug, Parser, Eq, PartialEq)]
#[command(name = "hive")]
#[command(about = "Shared team memory CLI for AI agents")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand, Eq, PartialEq)]
pub enum Command {
    /// Configure the CLI for a Hive team node.
    Init {
        /// Node URL to use.
        #[arg(long = "node-url", default_value = DEFAULT_NODE_URL)]
        node_url: String,

        /// File containing the API token.
        #[arg(long = "token-file", conflicts_with = "token")]
        token_file: Option<PathBuf>,

        /// API token value. Prefer --token-file for local node setup.
        #[arg(long = "token", conflicts_with = "token_file")]
        token: Option<String>,

        /// Config file path. Defaults to ~/.config/hivemind/hive.json.
        #[arg(long = "config")]
        config_path: Option<PathBuf>,
    },

    /// Join a Hive team node using an invite link or code.
    Join {
        /// Invite link or code.
        invite: String,
    },

    /// Discover Hive nodes on the local network.
    Discover {
        /// Discovery timeout in milliseconds.
        #[arg(long = "timeout-ms", default_value_t = 1500)]
        timeout_ms: u64,
    },

    /// Show how to share this node with teammates.
    Share {
        /// Node URL to share. Defaults to configured node URL, then localhost.
        #[arg(long = "node-url")]
        node_url: Option<String>,
    },

    /// List peer candidates from local config.
    Peers,

    /// Manage local peer trust.
    Peer {
        #[command(subcommand)]
        command: PeerCommand,
    },

    /// Save a text memory to HIVEMIND.
    Remember {
        /// Text to remember.
        text: String,

        /// Tag to attach to the memory. Repeat for multiple tags.
        #[arg(long = "tag", short = 't')]
        tags: Vec<String>,
    },

    /// Find memories by exact tag.
    Find {
        /// Exact tag to search for.
        tag: String,
    },

    /// Print a memory by object ID.
    Use {
        /// Object ID to retrieve.
        object_id: String,
    },
}

#[derive(Debug, Subcommand, Eq, PartialEq)]
pub enum PeerCommand {
    /// Mark a peer node ID as trusted in local config.
    Trust {
        /// Peer node ID/public-key fingerprint to trust.
        node_id: String,
    },

    /// Mark a peer node ID as untrusted in local config.
    Untrust {
        /// Peer node ID/public-key fingerprint to untrust.
        node_id: String,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Config {
    pub node_url: String,
    pub api_token: String,
}

#[derive(Debug, Deserialize, Eq, PartialEq, Serialize)]
struct FileConfig {
    node_url: String,
    api_token: String,
    #[serde(default)]
    peers: Vec<FilePeer>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct FilePeer {
    node_url: String,
    #[serde(default)]
    node_id: Option<String>,
    trusted: bool,
    source: String,
}

impl Config {
    pub fn from_env() -> Result<Self, CliError> {
        let node_url = std::env::var("HIVEMIND_NODE_URL")
            .unwrap_or_else(|_| DEFAULT_NODE_URL.to_owned())
            .trim_end_matches('/')
            .to_owned();
        let api_token = std::env::var("HIVEMIND_API_TOKEN").map_err(|_| CliError::MissingConfig)?;

        if api_token.trim().is_empty() {
            return Err(CliError::MissingConfig);
        }

        Ok(Self {
            node_url,
            api_token,
        })
    }

    pub fn from_env_or_file() -> Result<Self, CliError> {
        if let Ok(api_token) = std::env::var("HIVEMIND_API_TOKEN") {
            if !api_token.trim().is_empty() {
                return Self::from_env();
            }
        }

        let path = default_config_path()?;
        let file_config = read_file_config(&path)?;
        let node_url = std::env::var("HIVEMIND_NODE_URL")
            .unwrap_or(file_config.node_url)
            .trim_end_matches('/')
            .to_owned();
        let api_token = file_config.api_token.trim().to_owned();

        if api_token.is_empty() {
            return Err(CliError::ConfigFileInvalid { path });
        }

        Ok(Self {
            node_url,
            api_token,
        })
    }
}

#[derive(Debug, thiserror::Error)]
pub enum CliError {
    #[error("{MISSING_CONFIG_HELP}")]
    MissingConfig,

    #[error("could not determine a config path; set HIVEMIND_CONFIG or HOME")]
    MissingConfigPath,

    #[error("failed to read config file {path}: {source}")]
    ConfigRead {
        path: PathBuf,
        source: std::io::Error,
    },

    #[error("failed to write config file {path}: {source}")]
    ConfigWrite {
        path: PathBuf,
        source: std::io::Error,
    },

    #[error("config file {path} is invalid")]
    ConfigFileInvalid { path: PathBuf },

    #[error("hive init requires --token-file or --token")]
    MissingInitToken,

    #[error("invalid invite link or code; use hive://join?node=<node-url>&invite=<code> or configure a node first and pass a code")]
    InvalidInvite,

    #[error("unknown peer node id; run `hive peers` and trust a listed node id")]
    UnknownPeerNodeId,

    #[error("request failed: {0}")]
    Request(#[from] reqwest::Error),

    #[error("node returned {status}: {body}")]
    Api {
        status: reqwest::StatusCode,
        body: String,
    },

    #[error("response contained invalid base64 payload")]
    InvalidBase64,

    #[error("memory payload is not valid UTF-8 text")]
    InvalidUtf8,
}

const MISSING_CONFIG_HELP: &str = "No Hive team node configured.\n\nJoin a team node:\n  hive join <invite-link-or-code>\n\nOr configure manually:\n  hive init --node-url http://127.0.0.1:7747 --token-file ./data/api.token\n\nRunning your own node?\n  hive share";

#[derive(Debug, Serialize, Eq, PartialEq)]
struct PublishObjectRequest {
    object_type: String,
    mime_type: String,
    payload_base64: String,
    tags: Vec<String>,
    references: Vec<String>,
}

#[derive(Debug, Deserialize, Eq, PartialEq)]
struct PublishObjectResponse {
    object_id: String,
    chunk_ids: Vec<String>,
}

#[derive(Debug, Deserialize, Eq, PartialEq)]
struct TagLookupResponse {
    tag: String,
    objects: Vec<ObjectSummary>,
}

#[derive(Debug, Deserialize, Eq, PartialEq)]
struct ObjectSummary {
    object_id: String,
    object_type: String,
    author_agent_id: String,
    created_at_ms: u64,
    mime_type: String,
    payload_size: u64,
    chunk_count: u32,
}

#[derive(Debug, Deserialize, Eq, PartialEq)]
struct GetObjectResponse {
    object_id: String,
    object_type: String,
    author_agent_id: String,
    created_at_ms: u64,
    mime_type: String,
    tags: Vec<String>,
    references: Vec<String>,
    payload_base64: String,
    verified: bool,
}

#[derive(Debug, Serialize, Eq, PartialEq)]
struct CreateInviteRequest {
    node_url: String,
    ttl_seconds: Option<u64>,
    uses: Option<u32>,
}

#[derive(Debug, Deserialize, Eq, PartialEq)]
struct CreateInviteResponse {
    invite_code: String,
    invite_url: String,
    node_url: String,
    expires_at_ms: u64,
    uses_remaining: u32,
}

#[derive(Debug, Serialize, Eq, PartialEq)]
struct JoinInviteRequest {
    invite_code: String,
}

#[derive(Debug, Deserialize, Eq, PartialEq)]
struct JoinInviteResponse {
    node_url: String,
    api_token: String,
    #[serde(default)]
    peers: Vec<PeerSummary>,
}

#[derive(Debug, Deserialize, Eq, PartialEq)]
struct PeerSummary {
    node_url: String,
    #[serde(default)]
    node_id: Option<String>,
    trusted: bool,
}

struct ParsedInvite {
    node_url: String,
    invite_code: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct DiscoveredNode {
    node_url: String,
    node_id: String,
}

pub async fn run_from_env() -> Result<(), CliError> {
    let cli = Cli::parse();
    let client = reqwest::Client::new();
    let output = execute_from_env(cli, &client).await?;
    println!("{output}");
    Ok(())
}

pub async fn execute_from_env(cli: Cli, client: &reqwest::Client) -> Result<String, CliError> {
    match cli.command {
        Command::Init {
            node_url,
            token_file,
            token,
            config_path,
        } => init(node_url, token_file, token, config_path),
        Command::Join { invite } => join(client, invite).await,
        Command::Discover { timeout_ms } => discover(timeout_ms),
        Command::Share { node_url } => share(client, node_url).await,
        Command::Peers => list_peers(),
        Command::Peer { command } => update_peer_trust(command),
        Command::Remember { text, tags } => {
            let config = Config::from_env_or_file()?;
            remember(&config, client, text, tags).await
        }
        Command::Find { tag } => {
            let config = Config::from_env_or_file()?;
            find(&config, client, tag).await
        }
        Command::Use { object_id } => {
            let config = Config::from_env_or_file()?;
            use_memory(&config, client, object_id).await
        }
    }
}

pub async fn execute(
    cli: Cli,
    config: &Config,
    client: &reqwest::Client,
) -> Result<String, CliError> {
    match cli.command {
        Command::Remember { text, tags } => remember(config, client, text, tags).await,
        Command::Find { tag } => find(config, client, tag).await,
        Command::Use { object_id } => use_memory(config, client, object_id).await,
        Command::Share { node_url } => {
            let advertised_node_url = node_url.unwrap_or_else(|| config.node_url.clone());
            share_with_config(client, config, advertised_node_url).await
        }
        Command::Discover { timeout_ms } => discover(timeout_ms),
        Command::Peers | Command::Peer { .. } => execute_from_env(cli, client).await,
        Command::Init { .. } | Command::Join { .. } => execute_from_env(cli, client).await,
    }
}

async fn remember(
    config: &Config,
    client: &reqwest::Client,
    text: String,
    tags: Vec<String>,
) -> Result<String, CliError> {
    let request = remember_request(&text, tags);
    let response = post_json(client, config, "/v1/objects", &request)
        .await?
        .json::<PublishObjectResponse>()
        .await?;
    Ok(format_remember_response(&response))
}

async fn find(config: &Config, client: &reqwest::Client, tag: String) -> Result<String, CliError> {
    let path = format!("/v1/tags/{}", encode_path_segment(&tag));
    let response = get(client, config, &path)
        .await?
        .json::<TagLookupResponse>()
        .await?;
    Ok(format_find_response(&response))
}

async fn use_memory(
    config: &Config,
    client: &reqwest::Client,
    object_id: String,
) -> Result<String, CliError> {
    let path = format!("/v1/objects/{}", encode_path_segment(&object_id));
    let response = get(client, config, &path)
        .await?
        .json::<GetObjectResponse>()
        .await?;
    format_use_response(&response)
}

fn init(
    node_url: String,
    token_file: Option<PathBuf>,
    token: Option<String>,
    config_path: Option<PathBuf>,
) -> Result<String, CliError> {
    let token = match (token_file, token) {
        (Some(path), None) => fs::read_to_string(&path)
            .map_err(|source| CliError::ConfigRead { path, source })?
            .trim()
            .to_owned(),
        (None, Some(token)) => token.trim().to_owned(),
        _ => return Err(CliError::MissingInitToken),
    };

    if token.is_empty() {
        return Err(CliError::MissingInitToken);
    }

    let path = config_path.map(Ok).unwrap_or_else(default_config_path)?;
    write_file_config(
        &path,
        &FileConfig {
            node_url: node_url.trim_end_matches('/').to_owned(),
            api_token: token,
            peers: Vec::new(),
        },
    )?;

    Ok(format!(
        "configured Hive team node {}\nconfig: {}",
        node_url.trim_end_matches('/'),
        path.display()
    ))
}

async fn join(client: &reqwest::Client, invite: String) -> Result<String, CliError> {
    let parsed = parse_invite(&invite)?;
    let request = JoinInviteRequest {
        invite_code: parsed.invite_code,
    };
    let response = checked_response(
        client
            .post(format!("{}/v1/join", parsed.node_url))
            .json(&request)
            .send()
            .await?,
    )
    .await?
    .json::<JoinInviteResponse>()
    .await?;

    let path = default_config_path()?;
    let mut file_config = read_file_config(&path).unwrap_or_else(|_| FileConfig {
        node_url: response.node_url.clone(),
        api_token: response.api_token.clone(),
        peers: Vec::new(),
    });
    file_config.node_url = response.node_url.clone();
    file_config.api_token = response.api_token;

    let mut peer_count = 0;
    for peer in response
        .peers
        .iter()
        .filter(|peer| peer.node_url.trim_end_matches('/') != response.node_url)
    {
        let node_url = peer.node_url.trim_end_matches('/').to_owned();
        if let Some(existing) = file_config
            .peers
            .iter_mut()
            .find(|existing| existing.node_url == node_url)
        {
            if existing.node_id.is_none() {
                existing.node_id = peer.node_id.clone();
            }
            continue;
        }
        file_config.peers.push(FilePeer {
            node_url,
            node_id: peer.node_id.clone(),
            trusted: false,
            source: "invite".to_owned(),
        });
        peer_count += 1;
    }

    write_file_config(&path, &file_config)?;

    let mut output = format!(
        "joined Hive team node {}\nconfig: {}",
        response.node_url,
        path.display()
    );
    if peer_count > 0 {
        output.push_str(&format!(
            "\nreceived {peer_count} untrusted peer candidate(s); ask the user before trusting them with `hive peer trust <node-id>`"
        ));
    }
    Ok(output)
}

fn discover(timeout_ms: u64) -> Result<String, CliError> {
    let timeout = Duration::from_millis(timeout_ms.max(100));
    let socket =
        UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).map_err(|source| CliError::ConfigRead {
            path: PathBuf::from("udp discovery socket"),
            source,
        })?;
    socket
        .set_broadcast(true)
        .map_err(|source| CliError::ConfigRead {
            path: PathBuf::from("udp discovery socket"),
            source,
        })?;
    socket
        .set_read_timeout(Some(Duration::from_millis(120)))
        .map_err(|source| CliError::ConfigRead {
            path: PathBuf::from("udp discovery socket"),
            source,
        })?;

    let targets = [
        SocketAddr::from((Ipv4Addr::BROADCAST, DISCOVERY_PORT)),
        SocketAddr::from((Ipv4Addr::LOCALHOST, DISCOVERY_PORT)),
    ];
    for target in targets {
        let _ = socket.send_to(DISCOVERY_QUERY, target);
    }

    let deadline = Instant::now() + timeout;
    let mut nodes = BTreeMap::new();
    let mut buf = [0_u8; 1024];
    while Instant::now() < deadline {
        match socket.recv_from(&mut buf) {
            Ok((len, _peer)) => {
                let response = String::from_utf8_lossy(&buf[..len]);
                if let Some(rest) = response.strip_prefix(DISCOVERY_RESPONSE_PREFIX) {
                    if let Some(node) = parse_discovery_node(rest) {
                        nodes.insert(node.node_id.clone(), node);
                    }
                }
            }
            Err(err)
                if matches!(
                    err.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) => {}
            Err(_err) => break,
        }
    }

    Ok(format_discover_response(nodes.into_values().collect()))
}

fn parse_discovery_node(input: &str) -> Option<DiscoveredNode> {
    let mut parts = input.split_whitespace();
    let node_url = parts.next()?.trim_end_matches('/');
    let node_id = parts.next()?;
    if validate_node_url(node_url) && validate_node_id(node_id) {
        Some(DiscoveredNode {
            node_url: node_url.to_owned(),
            node_id: node_id.to_owned(),
        })
    } else {
        None
    }
}

fn validate_node_url(node_url: &str) -> bool {
    !node_url.is_empty()
        && !node_url.chars().any(char::is_whitespace)
        && (node_url.starts_with("http://") || node_url.starts_with("https://"))
        && reqwest::Url::parse(node_url).is_ok()
}

fn validate_node_id(node_id: &str) -> bool {
    node_id.len() == 64 && node_id.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn format_discover_response(nodes: Vec<DiscoveredNode>) -> String {
    if nodes.is_empty() {
        return "no Hive nodes discovered on the local network".to_owned();
    }

    let mut lines = vec!["Discovered Hive nodes:".to_owned(), "".to_owned()];
    for (index, node) in nodes.iter().enumerate() {
        lines.push(format!(
            "{}. {}\n   node id: {}",
            index + 1,
            node.node_url,
            node.node_id
        ));
    }
    lines.push("".to_owned());
    lines.push(
        "Discovery is not trust. Ask a teammate/admin for an invite before joining:".to_owned(),
    );
    lines.push(format!(
        "  hive join 'hive://join?node={}&invite=...'",
        encode_query_value(&nodes.first().expect("nodes is non-empty").node_url)
    ));
    lines.join("\n")
}

fn list_peers() -> Result<String, CliError> {
    let path = default_config_path()?;
    let config = read_file_config(&path)?;
    if config.peers.is_empty() {
        return Ok("no peer candidates configured".to_owned());
    }

    Ok(config
        .peers
        .iter()
        .map(|peer| {
            let status = if peer.trusted { "trusted" } else { "untrusted" };
            let node_id = peer.node_id.as_deref().unwrap_or("unknown-node-id");
            format!(
                "{status}\t{}\t{}\t(source: {})",
                peer.node_url, node_id, peer.source
            )
        })
        .collect::<Vec<_>>()
        .join("\n"))
}

fn update_peer_trust(command: PeerCommand) -> Result<String, CliError> {
    let path = default_config_path()?;
    update_peer_trust_at(&path, command)
}

fn update_peer_trust_at(path: &PathBuf, command: PeerCommand) -> Result<String, CliError> {
    let mut config = read_file_config(path)?;
    let (node_id, trusted) = match command {
        PeerCommand::Trust { node_id } => (node_id.trim().to_owned(), true),
        PeerCommand::Untrust { node_id } => (node_id.trim().to_owned(), false),
    };

    if node_id.is_empty() {
        return Err(CliError::UnknownPeerNodeId);
    }

    let Some(peer) = config
        .peers
        .iter_mut()
        .find(|peer| peer.node_id.as_deref() == Some(node_id.as_str()))
    else {
        return Err(CliError::UnknownPeerNodeId);
    };
    peer.trusted = trusted;
    let node_url = peer.node_url.clone();

    write_file_config(path, &config)?;
    let status = if trusted { "trusted" } else { "untrusted" };
    Ok(format!("marked peer {node_url} ({node_id}) as {status}"))
}

async fn share(client: &reqwest::Client, node_url: Option<String>) -> Result<String, CliError> {
    let config = match Config::from_env_or_file() {
        Ok(config) => config,
        Err(CliError::MissingConfig) => {
            let node_url = node_url.unwrap_or_else(|| DEFAULT_NODE_URL.to_owned());
            return Ok(format_share_response(node_url, None));
        }
        Err(err) => return Err(err),
    };

    let advertised_node_url = node_url.unwrap_or_else(|| config.node_url.clone());
    share_with_config(client, &config, advertised_node_url).await
}

async fn share_with_config(
    client: &reqwest::Client,
    config: &Config,
    advertised_node_url: String,
) -> Result<String, CliError> {
    let advertised_node_url = advertised_node_url.trim_end_matches('/').to_owned();
    if is_loopback_url(&advertised_node_url) {
        return Ok(format_share_response(advertised_node_url, None));
    }

    let request = CreateInviteRequest {
        node_url: advertised_node_url,
        ttl_seconds: Some(24 * 60 * 60),
        uses: Some(1),
    };
    let response = post_json(client, config, "/v1/invites", &request)
        .await?
        .json::<CreateInviteResponse>()
        .await?;
    Ok(format_share_response(
        response.node_url,
        Some(response.invite_url),
    ))
}

fn configured_node_url() -> Option<String> {
    if let Ok(node_url) = std::env::var("HIVEMIND_NODE_URL") {
        if !node_url.trim().is_empty() {
            return Some(node_url.trim_end_matches('/').to_owned());
        }
    }

    let path = default_config_path().ok()?;
    read_file_config(&path).ok().map(|config| config.node_url)
}

fn format_share_response(node_url: String, invite_url: Option<String>) -> String {
    let node_url = node_url.trim_end_matches('/').to_owned();
    if is_loopback_url(&node_url) {
        return format!(
            "This node is configured as local-only:\n  {node_url}\n\nTo share it with teammates, expose the node on a private reachable URL, then run:\n  hive share --node-url https://hive.your-team.internal\n\nDo not paste API tokens into shared URLs."
        );
    }

    match invite_url {
        Some(invite_url) => format!(
            "This node is available at:\n  {node_url}\n\nShare with a teammate:\n  hive join '{invite_url}'\n\nThe invite is short-lived and limited-use. Do not paste API tokens into shared URLs."
        ),
        None => format!(
            "This node is available at:\n  {node_url}\n\nNo invite was created because the CLI is not configured with an admin token.\nDo not paste API tokens into shared URLs."
        ),
    }
}

fn parse_invite(invite: &str) -> Result<ParsedInvite, CliError> {
    let invite = invite.trim();
    if invite.starts_with("hive://") {
        let url = reqwest::Url::parse(invite).map_err(|_| CliError::InvalidInvite)?;
        if url.host_str() != Some("join") {
            return Err(CliError::InvalidInvite);
        }
        let mut node_url = None;
        let mut invite_code = None;
        for (key, value) in url.query_pairs() {
            match key.as_ref() {
                "node" => node_url = Some(value.trim_end_matches('/').to_owned()),
                "invite" => invite_code = Some(value.to_string()),
                _ => {}
            }
        }
        let node_url = node_url.ok_or(CliError::InvalidInvite)?;
        let invite_code = invite_code.ok_or(CliError::InvalidInvite)?;
        if node_url.is_empty() || invite_code.is_empty() {
            return Err(CliError::InvalidInvite);
        }
        return Ok(ParsedInvite {
            node_url,
            invite_code,
        });
    }

    let node_url = configured_node_url().unwrap_or_else(|| DEFAULT_NODE_URL.to_owned());
    if invite.is_empty() {
        return Err(CliError::InvalidInvite);
    }
    Ok(ParsedInvite {
        node_url,
        invite_code: invite.to_owned(),
    })
}

fn is_loopback_url(node_url: &str) -> bool {
    let Ok(url) = reqwest::Url::parse(node_url) else {
        return false;
    };
    matches!(
        url.host_str(),
        Some("localhost") | Some("127.0.0.1") | Some("::1")
    )
}

fn default_config_path() -> Result<PathBuf, CliError> {
    if let Ok(path) = std::env::var("HIVEMIND_CONFIG") {
        if !path.trim().is_empty() {
            return Ok(PathBuf::from(path));
        }
    }

    let home = std::env::var("HOME").map_err(|_| CliError::MissingConfigPath)?;
    Ok(PathBuf::from(home).join(DEFAULT_CONFIG_RELATIVE_PATH))
}

fn read_file_config(path: &PathBuf) -> Result<FileConfig, CliError> {
    let input = fs::read_to_string(path).map_err(|source| {
        if source.kind() == std::io::ErrorKind::NotFound {
            CliError::MissingConfig
        } else {
            CliError::ConfigRead {
                path: path.clone(),
                source,
            }
        }
    })?;
    serde_json::from_str(&input).map_err(|_| CliError::ConfigFileInvalid { path: path.clone() })
}

fn write_file_config(path: &PathBuf, config: &FileConfig) -> Result<(), CliError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|source| CliError::ConfigWrite {
            path: path.clone(),
            source,
        })?;
    }

    let bytes = serde_json::to_vec_pretty(config)
        .map_err(|_| CliError::ConfigFileInvalid { path: path.clone() })?;
    write_secret_file(path, &bytes)
}

#[cfg(unix)]
fn write_secret_file(path: &PathBuf, bytes: &[u8]) -> Result<(), CliError> {
    use std::{
        fs::{self, OpenOptions},
        io::Write,
        os::unix::fs::{OpenOptionsExt, PermissionsExt},
    };

    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .mode(0o600)
        .open(path)
        .map_err(|source| CliError::ConfigWrite {
            path: path.clone(),
            source,
        })?;
    file.write_all(bytes)
        .and_then(|_| file.write_all(b"\n"))
        .and_then(|_| fs::set_permissions(path, fs::Permissions::from_mode(0o600)))
        .map_err(|source| CliError::ConfigWrite {
            path: path.clone(),
            source,
        })
}

#[cfg(not(unix))]
fn write_secret_file(path: &PathBuf, bytes: &[u8]) -> Result<(), CliError> {
    fs::write(path, [bytes, b"\n"].concat()).map_err(|source| CliError::ConfigWrite {
        path: path.clone(),
        source,
    })
}

fn remember_request(text: &str, tags: Vec<String>) -> PublishObjectRequest {
    PublishObjectRequest {
        object_type: "fact".to_owned(),
        mime_type: "text/plain".to_owned(),
        payload_base64: STANDARD.encode(text.as_bytes()),
        tags,
        references: Vec::new(),
    }
}

async fn get(
    client: &reqwest::Client,
    config: &Config,
    path: &str,
) -> Result<reqwest::Response, CliError> {
    checked_response(
        client
            .get(format!("{}{}", config.node_url, path))
            .bearer_auth(&config.api_token)
            .send()
            .await?,
    )
    .await
}

async fn post_json<T: Serialize + ?Sized>(
    client: &reqwest::Client,
    config: &Config,
    path: &str,
    body: &T,
) -> Result<reqwest::Response, CliError> {
    checked_response(
        client
            .post(format!("{}{}", config.node_url, path))
            .bearer_auth(&config.api_token)
            .json(body)
            .send()
            .await?,
    )
    .await
}

async fn checked_response(response: reqwest::Response) -> Result<reqwest::Response, CliError> {
    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }
    let body = response.text().await.unwrap_or_default();
    Err(CliError::Api { status, body })
}

fn format_remember_response(response: &PublishObjectResponse) -> String {
    if response.chunk_ids.is_empty() {
        format!("saved {}", response.object_id)
    } else {
        format!(
            "saved {}\nchunks: {}",
            response.object_id,
            response.chunk_ids.join(",")
        )
    }
}

fn format_find_response(response: &TagLookupResponse) -> String {
    if response.objects.is_empty() {
        return format!("no memories found for tag {:?}", response.tag);
    }

    response
        .objects
        .iter()
        .enumerate()
        .map(|(index, object)| {
            format!(
                "{}. {} {} {} ({} bytes, {} chunks)",
                index + 1,
                object.object_id,
                object.object_type,
                object.mime_type,
                object.payload_size,
                object.chunk_count
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn format_use_response(response: &GetObjectResponse) -> Result<String, CliError> {
    let payload = STANDARD
        .decode(response.payload_base64.as_bytes())
        .map_err(|_| CliError::InvalidBase64)?;
    let payload = String::from_utf8(payload).map_err(|_| CliError::InvalidUtf8)?;

    let mut lines = vec![
        format!("object_id: {}", response.object_id),
        format!("type: {}", response.object_type),
        format!("mime_type: {}", response.mime_type),
        format!("tags: {}", response.tags.join(",")),
        format!("verified: {}", response.verified),
    ];
    if !response.references.is_empty() {
        lines.push(format!("references: {}", response.references.join(",")));
    }
    lines.push("".to_owned());
    lines.push(payload);
    Ok(lines.join("\n"))
}

fn encode_path_segment(value: &str) -> String {
    encode_query_value(value)
}

fn encode_query_value(value: &str) -> String {
    let mut output = String::new();
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                output.push(byte as char);
            }
            _ => output.push_str(&format!("%{byte:02X}")),
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn parses_init_command_with_token_file() {
        let cli = Cli::parse_from([
            "hive",
            "init",
            "--node-url",
            "https://hive.example.internal",
            "--token-file",
            "./api.token",
        ]);

        assert_eq!(
            cli,
            Cli {
                command: Command::Init {
                    node_url: "https://hive.example.internal".to_owned(),
                    token_file: Some(PathBuf::from("./api.token")),
                    token: None,
                    config_path: None,
                },
            }
        );
    }

    #[test]
    fn parses_join_command() {
        let cli = Cli::parse_from(["hive", "join", "hive://join?invite=ABCD"]);

        assert_eq!(
            cli,
            Cli {
                command: Command::Join {
                    invite: "hive://join?invite=ABCD".to_owned(),
                },
            }
        );
    }

    #[test]
    fn parses_share_command() {
        let cli = Cli::parse_from([
            "hive",
            "share",
            "--node-url",
            "https://hive.example.internal",
        ]);

        assert_eq!(
            cli,
            Cli {
                command: Command::Share {
                    node_url: Some("https://hive.example.internal".to_owned()),
                },
            }
        );
    }

    #[test]
    fn parses_discover_command() {
        let cli = Cli::parse_from(["hive", "discover", "--timeout-ms", "250"]);

        assert_eq!(
            cli,
            Cli {
                command: Command::Discover { timeout_ms: 250 },
            }
        );
    }

    #[test]
    fn parses_remember_command_with_tags() {
        let cli = Cli::parse_from([
            "hive",
            "remember",
            "Replay failed Stripe webhooks before retrying invoices.",
            "--tag",
            "billing",
            "-t",
            "stripe",
        ]);

        assert_eq!(
            cli,
            Cli {
                command: Command::Remember {
                    text: "Replay failed Stripe webhooks before retrying invoices.".to_owned(),
                    tags: vec!["billing".to_owned(), "stripe".to_owned()],
                },
            }
        );
    }

    #[test]
    fn parses_find_command() {
        let cli = Cli::parse_from(["hive", "find", "billing"]);

        assert_eq!(
            cli,
            Cli {
                command: Command::Find {
                    tag: "billing".to_owned(),
                },
            }
        );
    }

    #[test]
    fn parses_use_command() {
        let cli = Cli::parse_from(["hive", "use", "abc123"]);

        assert_eq!(
            cli,
            Cli {
                command: Command::Use {
                    object_id: "abc123".to_owned(),
                },
            }
        );
    }

    #[test]
    fn parses_peers_command() {
        let cli = Cli::parse_from(["hive", "peers"]);

        assert_eq!(
            cli,
            Cli {
                command: Command::Peers
            }
        );
    }

    #[test]
    fn parses_peer_trust_command() {
        let cli = Cli::parse_from([
            "hive",
            "peer",
            "trust",
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        ]);

        assert_eq!(
            cli,
            Cli {
                command: Command::Peer {
                    command: PeerCommand::Trust {
                        node_id: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                            .to_owned(),
                    },
                },
            }
        );
    }

    #[test]
    fn peer_trust_updates_by_node_id_not_url() {
        let tempdir = tempfile::tempdir().unwrap();
        let config_path = tempdir.path().join("hive.json");
        write_file_config(
            &config_path,
            &FileConfig {
                node_url: "https://hive.example.internal".to_owned(),
                api_token: "secret".to_owned(),
                peers: vec![FilePeer {
                    node_url: "https://node-b.internal".to_owned(),
                    node_id: Some(
                        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                            .to_owned(),
                    ),
                    trusted: false,
                    source: "invite".to_owned(),
                }],
            },
        )
        .unwrap();
        let output = update_peer_trust_at(
            &config_path,
            PeerCommand::Trust {
                node_id: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                    .to_owned(),
            },
        )
        .unwrap();

        assert!(output.contains("marked peer https://node-b.internal"));
        assert!(read_file_config(&config_path).unwrap().peers[0].trusted);
    }

    #[test]
    fn peer_trust_rejects_url() {
        let tempdir = tempfile::tempdir().unwrap();
        let config_path = tempdir.path().join("hive.json");
        write_file_config(
            &config_path,
            &FileConfig {
                node_url: "https://hive.example.internal".to_owned(),
                api_token: "secret".to_owned(),
                peers: Vec::new(),
            },
        )
        .unwrap();
        let error = update_peer_trust_at(
            &config_path,
            PeerCommand::Trust {
                node_id: "https://node-b.internal".to_owned(),
            },
        )
        .unwrap_err();

        assert!(matches!(error, CliError::UnknownPeerNodeId));
    }

    #[test]
    fn file_config_defaults_missing_peers() {
        let config: FileConfig = serde_json::from_str(
            r#"{"node_url":"https://hive.example.internal","api_token":"secret"}"#,
        )
        .unwrap();

        assert_eq!(config.peers, Vec::new());
    }

    #[test]
    fn init_writes_config_file() {
        let tempdir = tempfile::tempdir().unwrap();
        let token_path = tempdir.path().join("api.token");
        let config_path = tempdir.path().join("hive.json");
        fs::write(&token_path, "secret-token\n").unwrap();

        let output = init(
            "https://hive.example.internal/".to_owned(),
            Some(token_path),
            None,
            Some(config_path.clone()),
        )
        .unwrap();

        assert!(output.contains("configured Hive team node https://hive.example.internal"));
        assert_eq!(
            read_file_config(&config_path).unwrap(),
            FileConfig {
                node_url: "https://hive.example.internal".to_owned(),
                api_token: "secret-token".to_owned(),
                peers: Vec::new(),
            }
        );
    }

    #[cfg(unix)]
    #[test]
    fn init_writes_owner_only_config_file() {
        use std::os::unix::fs::PermissionsExt;

        let tempdir = tempfile::tempdir().unwrap();
        let config_path = tempdir.path().join("hive.json");

        init(
            "http://127.0.0.1:7747".to_owned(),
            None,
            Some("secret-token".to_owned()),
            Some(config_path.clone()),
        )
        .unwrap();

        let mode = fs::metadata(config_path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }

    #[test]
    fn formats_loopback_share_response() {
        let output = format_share_response("http://127.0.0.1:7747".to_owned(), None);

        assert!(output.contains("local-only"));
        assert!(output.contains("hive share --node-url"));
        assert!(output.contains("Do not paste API tokens"));
    }

    #[test]
    fn parses_invite_link() {
        let parsed =
            parse_invite("hive://join?node=https%3A%2F%2Fhive.example.internal&invite=ABCD-EFGH")
                .unwrap();

        assert_eq!(parsed.node_url, "https://hive.example.internal");
        assert_eq!(parsed.invite_code, "ABCD-EFGH");
    }

    #[test]
    fn formats_reachable_share_response() {
        let output = format_share_response(
            "https://hive.example.internal".to_owned(),
            Some("hive://join?node=https%3A%2F%2Fhive.example.internal&invite=ABCD".to_owned()),
        );

        assert!(output.contains("This node is available at:"));
        assert!(output.contains("hive join 'hive://join"));
        assert!(output.contains("short-lived and limited-use"));
        assert!(output.contains("Do not paste API tokens"));
    }

    #[test]
    fn formats_discovered_nodes_as_untrusted() {
        let output = format_discover_response(vec![DiscoveredNode {
            node_url: "http://192.168.1.20:7747".to_owned(),
            node_id: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_owned(),
        }]);

        assert!(output.contains("Discovered Hive nodes"));
        assert!(output.contains("Discovery is not trust"));
        assert!(output
            .contains("node id: aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"));
        assert!(output
            .contains("hive join 'hive://join?node=http%3A%2F%2F192.168.1.20%3A7747&invite=..."));
    }

    #[test]
    fn discovery_response_parser_rejects_invalid_urls_and_node_ids() {
        assert_eq!(
            parse_discovery_node(
                "https://hive.example.internal aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
            ),
            Some(DiscoveredNode {
                node_url: "https://hive.example.internal".to_owned(),
                node_id: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_owned(),
            })
        );
        assert_eq!(parse_discovery_node("javascript:alert(1) aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"), None);
        assert_eq!(
            parse_discovery_node(
                "http:// aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
            ),
            None
        );
        assert_eq!(
            parse_discovery_node("https://hive.example.internal not-a-node-id"),
            None
        );
    }

    #[test]
    fn missing_config_error_guides_user() {
        let output = CliError::MissingConfig.to_string();

        assert!(output.contains("No Hive team node configured"));
        assert!(output.contains("hive join <invite-link-or-code>"));
        assert!(output.contains("hive init --node-url"));
        assert!(output.contains("hive share"));
    }

    #[test]
    fn missing_config_file_returns_guidance_error() {
        let tempdir = tempfile::tempdir().unwrap();
        let missing_path = tempdir.path().join("missing.json");

        let error = read_file_config(&missing_path).unwrap_err();

        assert_eq!(error.to_string(), CliError::MissingConfig.to_string());
    }

    #[test]
    fn remember_request_encodes_text_fact() {
        let request = remember_request("hello hive", vec!["demo".to_owned()]);

        assert_eq!(
            request,
            PublishObjectRequest {
                object_type: "fact".to_owned(),
                mime_type: "text/plain".to_owned(),
                payload_base64: STANDARD.encode(b"hello hive"),
                tags: vec!["demo".to_owned()],
                references: Vec::new(),
            }
        );
    }

    #[test]
    fn formats_find_results() {
        let response = TagLookupResponse {
            tag: "billing".to_owned(),
            objects: vec![ObjectSummary {
                object_id: "abc".to_owned(),
                object_type: "fact".to_owned(),
                author_agent_id: "agent".to_owned(),
                created_at_ms: 1,
                mime_type: "text/plain".to_owned(),
                payload_size: 11,
                chunk_count: 0,
            }],
        };

        assert_eq!(
            format_find_response(&response),
            "1. abc fact text/plain (11 bytes, 0 chunks)"
        );
    }

    #[test]
    fn formats_use_response_as_text_memory() {
        let response = GetObjectResponse {
            object_id: "abc".to_owned(),
            object_type: "fact".to_owned(),
            author_agent_id: "agent".to_owned(),
            created_at_ms: 1,
            mime_type: "text/plain".to_owned(),
            tags: vec!["billing".to_owned(), "stripe".to_owned()],
            references: Vec::new(),
            payload_base64: STANDARD.encode(b"hello hive"),
            verified: true,
        };

        assert_eq!(
            format_use_response(&response).unwrap(),
            "object_id: abc\ntype: fact\nmime_type: text/plain\ntags: billing,stripe\nverified: true\n\nhello hive"
        );
    }

    #[test]
    fn encodes_path_segment() {
        assert_eq!(
            encode_path_segment("billing/stripe ops"),
            "billing%2Fstripe%20ops"
        );
    }
}
