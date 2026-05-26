use serde::Deserialize;
use std::{net::SocketAddr, path::PathBuf};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub struct NodeConfig {
    pub data: DataConfig,
    pub api: ApiFileConfig,
    pub identity: IdentityConfig,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub struct DataConfig {
    pub dir: PathBuf,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub struct ApiFileConfig {
    pub bind_addr: SocketAddr,
    pub auth_token_file: PathBuf,
    #[serde(default)]
    pub public_url: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub struct IdentityConfig {
    pub agent_key_path: PathBuf,
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("failed to read config file: {0}")]
    Read(#[from] std::io::Error),

    #[error("failed to parse config file: {0}")]
    Parse(#[from] toml::de::Error),
}

impl NodeConfig {
    pub fn from_toml_str(input: &str) -> Result<Self, ConfigError> {
        Ok(toml::from_str(input)?)
    }

    pub fn from_file(path: impl AsRef<std::path::Path>) -> Result<Self, ConfigError> {
        let input = std::fs::read_to_string(path)?;
        Self::from_toml_str(&input)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_node_config() {
        let config = NodeConfig::from_toml_str(
            r#"
[data]
dir = "./data"

[api]
bind_addr = "127.0.0.1:7747"
auth_token_file = "./data/api.token"

[identity]
agent_key_path = "./data/agent.ed25519"
"#,
        )
        .unwrap();

        assert_eq!(config.data.dir, PathBuf::from("./data"));
        assert_eq!(config.api.bind_addr, "127.0.0.1:7747".parse().unwrap());
        assert_eq!(config.api.public_url, None);
        assert_eq!(
            config.identity.agent_key_path,
            PathBuf::from("./data/agent.ed25519")
        );
    }
}
