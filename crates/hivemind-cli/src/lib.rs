use clap::{Parser, Subcommand};
use hivemind_core::{valid_node_id, ChatMessage, PeerInfo, PeerRecord};
use serde::{Deserialize, Serialize};
use std::{env, fs, path::PathBuf, time::Duration};

const DEFAULT_NODE_URL: &str = "http://127.0.0.1:7747";

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

async fn setup(client: &reqwest::Client) -> Result<String, CliError> {
    let node = node_info(client).await?;
    let peers = fetch_peers(client).await?.peers;
    let mut lines = vec![
        "Hive setup".to_owned(),
        format!("local node: {}", node.node_url),
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
            lines.push(format!("{status}\t{}\t{}", peer.node_url, peer.node_id));
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
            format!(
                "{status}\t{}\t{}\t(source: {})",
                peer.node_url, peer.node_id, peer.source
            )
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
    Ok(format!("trusted {} ({})", peer.node_url, peer.node_id))
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
    let mut lines = vec![sent, "".to_owned(), "Replies:".to_owned()];
    for reply in replies.into_iter().filter(|message| message.text != text) {
        lines.push(format!(
            "{}: {}",
            short_id(&reply.author_node_id),
            reply.text
        ));
    }
    Ok(lines.join("\n"))
}

async fn chat(client: &reqwest::Client, room: &str, after_ms: u64) -> Result<String, CliError> {
    let messages = fetch_messages(client, room, after_ms).await?.messages;
    if messages.is_empty() {
        return Ok("no messages".to_owned());
    }
    Ok(messages
        .into_iter()
        .map(|message| {
            format!(
                "{} {}: {}",
                message.created_at_ms,
                short_id(&message.author_node_id),
                message.text
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
        format!(
            "data_dir = \"./data-{port}\"\nbind_addr = \"0.0.0.0:{port}\"\npublic_url = \"http://127.0.0.1:{port}\"\n"
        ),
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
    fn shortens_node_id() {
        assert_eq!(short_id("abcdef123456"), "abcdef12");
    }
}
