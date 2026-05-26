use base64::{engine::general_purpose::STANDARD, Engine};
use clap::{Parser, Subcommand};
use serde::{Deserialize, Serialize};

const DEFAULT_NODE_URL: &str = "http://127.0.0.1:7747";

#[derive(Debug, Parser, Eq, PartialEq)]
#[command(name = "hive")]
#[command(about = "Shared memory CLI for AI agents")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand, Eq, PartialEq)]
pub enum Command {
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

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Config {
    pub node_url: String,
    pub api_token: String,
}

impl Config {
    pub fn from_env() -> Result<Self, CliError> {
        let node_url = std::env::var("HIVEMIND_NODE_URL")
            .unwrap_or_else(|_| DEFAULT_NODE_URL.to_owned())
            .trim_end_matches('/')
            .to_owned();
        let api_token =
            std::env::var("HIVEMIND_API_TOKEN").map_err(|_| CliError::MissingApiToken)?;

        if api_token.trim().is_empty() {
            return Err(CliError::MissingApiToken);
        }

        Ok(Self {
            node_url,
            api_token,
        })
    }
}

#[derive(Debug, thiserror::Error)]
pub enum CliError {
    #[error("HIVEMIND_API_TOKEN is required")]
    MissingApiToken,

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

pub async fn run_from_env() -> Result<(), CliError> {
    let cli = Cli::parse();
    let config = Config::from_env()?;
    let client = reqwest::Client::new();
    let output = execute(cli, &config, &client).await?;
    println!("{output}");
    Ok(())
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
