use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    #[error("randomness error: {0}")]
    Random(String),
    #[error("invalid hex")]
    Hex(#[from] hex::FromHexError),
    #[error("invalid key")]
    InvalidKey,
    #[error("invalid signature")]
    InvalidSignature,
    #[error("invalid message id")]
    InvalidMessageId,
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
}

#[derive(Clone)]
pub struct NodeKey {
    signing_key: SigningKey,
}

impl NodeKey {
    pub fn generate() -> Result<Self, CoreError> {
        let mut seed = [0_u8; 32];
        getrandom::getrandom(&mut seed).map_err(|err| CoreError::Random(err.to_string()))?;
        Ok(Self {
            signing_key: SigningKey::from_bytes(&seed),
        })
    }

    pub fn from_seed_hex(seed_hex: &str) -> Result<Self, CoreError> {
        let bytes = hex::decode(seed_hex.trim())?;
        let seed: [u8; 32] = bytes.try_into().map_err(|_| CoreError::InvalidKey)?;
        Ok(Self {
            signing_key: SigningKey::from_bytes(&seed),
        })
    }

    pub fn seed_hex(&self) -> String {
        hex::encode(self.signing_key.to_bytes())
    }

    pub fn node_id(&self) -> String {
        hex::encode(self.signing_key.verifying_key().to_bytes())
    }

    pub fn sign_chat(&self, room: &str, created_at_ms: u64, text: &str) -> ChatMessage {
        let author_node_id = self.node_id();
        let unsigned = UnsignedChatMessage {
            room: room.to_owned(),
            author_node_id: author_node_id.clone(),
            created_at_ms,
            text: text.to_owned(),
        };
        let signing_bytes = unsigned.signing_bytes().expect("chat signing bytes encode");
        let signature = self.signing_key.sign(&signing_bytes);
        let signature_hex = hex::encode(signature.to_bytes());
        let id = message_id(
            &unsigned.room,
            &author_node_id,
            created_at_ms,
            &unsigned.text,
            &signature_hex,
        );
        ChatMessage {
            id,
            room: unsigned.room,
            author_node_id,
            created_at_ms,
            text: unsigned.text,
            signature: signature_hex,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq)]
pub struct PeerInfo {
    pub node_url: String,
    pub node_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq)]
pub struct PeerRecord {
    pub node_url: String,
    pub node_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default)]
    pub last_seen_ms: u64,
    pub trusted: bool,
    pub source: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq)]
pub struct ChatMessage {
    pub id: String,
    pub room: String,
    pub author_node_id: String,
    pub created_at_ms: u64,
    pub text: String,
    pub signature: String,
}

#[derive(Serialize)]
struct UnsignedChatMessage {
    room: String,
    author_node_id: String,
    created_at_ms: u64,
    text: String,
}

impl UnsignedChatMessage {
    fn signing_bytes(&self) -> Result<Vec<u8>, CoreError> {
        Ok(serde_json::to_vec(self)?)
    }
}

impl ChatMessage {
    pub fn expected_id(&self) -> String {
        message_id(
            &self.room,
            &self.author_node_id,
            self.created_at_ms,
            &self.text,
            &self.signature,
        )
    }

    pub fn verify(&self) -> Result<(), CoreError> {
        if self.id != self.expected_id() {
            return Err(CoreError::InvalidMessageId);
        }
        let key_bytes = hex::decode(&self.author_node_id)?;
        let key_bytes: [u8; 32] = key_bytes.try_into().map_err(|_| CoreError::InvalidKey)?;
        let verifying_key =
            VerifyingKey::from_bytes(&key_bytes).map_err(|_| CoreError::InvalidKey)?;
        let sig_bytes = hex::decode(&self.signature)?;
        let sig_bytes: [u8; 64] = sig_bytes
            .try_into()
            .map_err(|_| CoreError::InvalidSignature)?;
        let signature = Signature::from_bytes(&sig_bytes);
        let unsigned = UnsignedChatMessage {
            room: self.room.clone(),
            author_node_id: self.author_node_id.clone(),
            created_at_ms: self.created_at_ms,
            text: self.text.clone(),
        };
        verifying_key
            .verify(&unsigned.signing_bytes()?, &signature)
            .map_err(|_| CoreError::InvalidSignature)
    }
}

pub fn valid_node_id(node_id: &str) -> bool {
    node_id.len() == 64 && node_id.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn message_id(
    room: &str,
    author_node_id: &str,
    created_at_ms: u64,
    text: &str,
    signature: &str,
) -> String {
    let mut hasher = Sha256::new();
    hash_field(&mut hasher, room.as_bytes());
    hash_field(&mut hasher, author_node_id.as_bytes());
    hash_field(&mut hasher, &created_at_ms.to_be_bytes());
    hash_field(&mut hasher, text.as_bytes());
    hash_field(&mut hasher, signature.as_bytes());
    hex::encode(hasher.finalize())
}

fn hash_field(hasher: &mut Sha256, bytes: &[u8]) {
    hasher.update((bytes.len() as u64).to_be_bytes());
    hasher.update(bytes);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_key_roundtrips_seed() {
        let key = NodeKey::generate().unwrap();
        let loaded = NodeKey::from_seed_hex(&key.seed_hex()).unwrap();
        assert_eq!(key.node_id(), loaded.node_id());
    }

    #[test]
    fn signed_chat_verifies() {
        let key = NodeKey::generate().unwrap();
        let message = key.sign_chat("default", 123, "hello");
        message.verify().unwrap();
    }

    #[test]
    fn wrong_author_fails_verification() {
        let key = NodeKey::generate().unwrap();
        let other = NodeKey::generate().unwrap();
        let mut message = key.sign_chat("default", 123, "hello");
        message.author_node_id = other.node_id();
        assert!(matches!(message.verify(), Err(CoreError::InvalidMessageId)));
    }

    #[test]
    fn changed_chat_fails_verification() {
        let key = NodeKey::generate().unwrap();
        let mut message = key.sign_chat("default", 123, "hello");
        message.text = "tampered".to_owned();
        assert!(matches!(message.verify(), Err(CoreError::InvalidMessageId)));
    }

    #[test]
    fn changed_id_fails_verification() {
        let key = NodeKey::generate().unwrap();
        let mut message = key.sign_chat("default", 123, "hello");
        message.id = "bad".to_owned();
        assert!(matches!(message.verify(), Err(CoreError::InvalidMessageId)));
    }

    #[test]
    fn validates_node_ids() {
        assert!(valid_node_id(&"a".repeat(64)));
        assert!(!valid_node_id("not-a-node-id"));
    }
}
