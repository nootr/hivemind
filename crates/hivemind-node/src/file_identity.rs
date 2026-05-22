use crate::secret_file::write_new_secret;
use async_trait::async_trait;
use hivemind_app::{AppResult, IdentityPort};
use hivemind_core::{AgentId, AgentKeypair, ObjectBody, ObjectEnvelope};
use std::path::Path;

pub struct FileIdentity {
    keypair: AgentKeypair,
}

#[derive(Debug, thiserror::Error)]
pub enum FileIdentityError {
    #[error("identity io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("identity randomness error: {0}")]
    Random(String),

    #[error("invalid identity key encoding")]
    InvalidEncoding,
}

impl FileIdentity {
    pub fn load_or_create(path: impl AsRef<Path>) -> Result<Self, FileIdentityError> {
        let seed = load_or_create_seed(path.as_ref())?;
        Ok(Self {
            keypair: AgentKeypair::from_seed(seed),
        })
    }

    pub fn agent_id(&self) -> AgentId {
        self.keypair.agent_id()
    }
}

#[async_trait]
impl IdentityPort for FileIdentity {
    async fn agent_id(&self) -> AppResult<AgentId> {
        Ok(self.keypair.agent_id())
    }

    async fn sign_object(&self, body: ObjectBody) -> AppResult<ObjectEnvelope> {
        Ok(self.keypair.sign_object(body)?)
    }
}

fn load_or_create_seed(path: &Path) -> Result<[u8; 32], FileIdentityError> {
    match std::fs::read_to_string(path) {
        Ok(seed_hex) => parse_seed(seed_hex.trim()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            let seed = generate_seed()?;
            write_new_secret(path, format!("{}\n", hex::encode(seed)).as_bytes())?;
            Ok(seed)
        }
        Err(err) => Err(FileIdentityError::Io(err)),
    }
}

fn generate_seed() -> Result<[u8; 32], FileIdentityError> {
    let mut seed = [0_u8; 32];
    getrandom::getrandom(&mut seed).map_err(|err| FileIdentityError::Random(err.to_string()))?;
    Ok(seed)
}

fn parse_seed(seed_hex: &str) -> Result<[u8; 32], FileIdentityError> {
    let mut seed = [0_u8; 32];
    hex::decode_to_slice(seed_hex, &mut seed).map_err(|_| FileIdentityError::InvalidEncoding)?;
    Ok(seed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_generate_load_is_idempotent() {
        let tempdir = tempfile::tempdir().unwrap();
        let path = tempdir.path().join("agent.ed25519");

        let first = FileIdentity::load_or_create(&path).unwrap();
        let second = FileIdentity::load_or_create(&path).unwrap();

        assert_eq!(first.agent_id(), second.agent_id());
    }

    #[test]
    fn invalid_key_file_is_rejected() {
        let tempdir = tempfile::tempdir().unwrap();
        let path = tempdir.path().join("agent.ed25519");
        std::fs::write(&path, "not-hex\n").unwrap();

        assert!(matches!(
            FileIdentity::load_or_create(&path),
            Err(FileIdentityError::InvalidEncoding)
        ));
    }

    #[cfg(unix)]
    #[test]
    fn created_key_file_is_owner_only() {
        let tempdir = tempfile::tempdir().unwrap();
        let path = tempdir.path().join("agent.ed25519");

        FileIdentity::load_or_create(&path).unwrap();

        crate::secret_file::assert_secret_file_mode(&path);
    }
}
