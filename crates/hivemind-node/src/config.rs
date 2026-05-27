use crate::NodeError;
use serde::Deserialize;
use std::{fs, net::SocketAddr, path::PathBuf};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub struct NodeConfig {
    pub data_dir: PathBuf,
    pub bind_addr: SocketAddr,
    pub public_url: Option<String>,
}

impl NodeConfig {
    pub fn from_file(path: impl AsRef<std::path::Path>) -> Result<Self, NodeError> {
        let input = fs::read_to_string(path)?;
        Ok(toml::from_str(&input)?)
    }
}
