use minicbor::{Decode, Encode};
use std::{fmt, str::FromStr};

const OBJECT_ID_DOMAIN: &[u8] = b"hm-object-v1";
const CHUNK_ID_DOMAIN: &[u8] = b"hm-chunk-v1";
const AGENT_ID_DOMAIN: &[u8] = b"hm-agent-v1";

#[derive(Clone, Copy, Debug, Decode, Encode, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[cbor(transparent)]
pub struct ObjectId(#[n(0)] [u8; 32]);

#[derive(Clone, Copy, Debug, Decode, Encode, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[cbor(transparent)]
pub struct ChunkId(#[n(0)] [u8; 32]);

#[derive(Clone, Copy, Debug, Decode, Encode, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[cbor(transparent)]
pub struct AgentId(#[n(0)] [u8; 32]);

impl ObjectId {
    pub(crate) fn from_canonical_body(body: &[u8]) -> Self {
        Self(domain_hash(OBJECT_ID_DOMAIN, body))
    }

    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl ChunkId {
    pub fn from_chunk_bytes(bytes: &[u8]) -> Self {
        Self(domain_hash(CHUNK_ID_DOMAIN, bytes))
    }

    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl AgentId {
    pub fn from_public_key(public_key: &[u8; 32]) -> Self {
        Self(domain_hash(AGENT_ID_DOMAIN, public_key))
    }

    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

fn domain_hash(domain: &[u8], bytes: &[u8]) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new();
    hasher.update(domain);
    hasher.update(bytes);
    *hasher.finalize().as_bytes()
}

macro_rules! impl_hex_display {
    ($ty:ty) => {
        impl fmt::Display for $ty {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}", hex::encode(self.0))
            }
        }

        impl FromStr for $ty {
            type Err = hex::FromHexError;

            fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
                let mut bytes = [0_u8; 32];
                hex::decode_to_slice(s, &mut bytes)?;
                Ok(Self(bytes))
            }
        }
    };
}

impl_hex_display!(ObjectId);
impl_hex_display!(ChunkId);
impl_hex_display!(AgentId);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_ids_are_domain_separated_from_raw_hashes() {
        let public_key = [7_u8; 32];
        let agent_id = AgentId::from_public_key(&public_key);
        assert_ne!(agent_id.as_bytes(), blake3::hash(&public_key).as_bytes());
    }

    #[test]
    fn ids_display_as_hex_and_parse_back() {
        let chunk_id = ChunkId::from_chunk_bytes(b"hello");
        let parsed = chunk_id.to_string().parse::<ChunkId>().unwrap();
        assert_eq!(chunk_id, parsed);
    }
}
