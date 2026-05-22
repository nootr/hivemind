use crate::{AgentId, ChunkId, Error, ObjectId, Result};
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use minicbor::{Decode, Encode};
use std::num::NonZeroUsize;

const OBJECT_SIGNATURE_DOMAIN: &[u8] = b"hm-object-signature-v1";

pub const INLINE_OBJECT_THRESHOLD: usize = 16 * 1024;
pub const DEFAULT_CHUNK_SIZE: usize = 64 * 1024;
pub const MAX_PAYLOAD_SIZE: usize = 10 * 1024 * 1024;

#[derive(Clone, Debug, Decode, Encode, Eq, PartialEq)]
pub struct ObjectBody {
    #[n(0)]
    pub schema_version: u16,
    #[n(1)]
    pub kind: ObjectKind,
    #[n(2)]
    pub author: AgentId,
    #[n(3)]
    pub created_at_ms: u64,
    #[n(4)]
    pub tags: Vec<String>,
    #[n(5)]
    pub references: Vec<ObjectId>,
    #[n(6)]
    pub payload: Payload,
}

#[derive(Clone, Copy, Debug, Decode, Encode, Eq, PartialEq)]
pub enum ObjectKind {
    #[n(0)]
    Skill,
    #[n(1)]
    Fact,
    #[n(2)]
    Procedure,
    #[n(3)]
    Insight,
    #[n(4)]
    Rating,
    #[n(5)]
    Report,
    #[n(6)]
    Tombstone,
    #[n(7)]
    Alias,
}

#[derive(Clone, Debug, Decode, Encode, Eq, PartialEq)]
pub enum Payload {
    #[n(0)]
    Inline(#[n(0)] InlinePayload),
    #[n(1)]
    Chunked(#[n(0)] ChunkedPayload),
}

#[derive(Clone, Debug, Decode, Encode, Eq, PartialEq)]
pub struct InlinePayload {
    #[n(0)]
    pub mime_type: String,
    #[n(1)]
    pub bytes: Vec<u8>,
}

#[derive(Clone, Debug, Decode, Encode, Eq, PartialEq)]
pub struct ChunkedPayload {
    #[n(0)]
    pub mime_type: String,
    #[n(1)]
    pub total_size: u64,
    #[n(2)]
    pub chunks: Vec<ChunkRef>,
}

#[derive(Clone, Debug, Decode, Encode, Eq, PartialEq)]
pub struct ChunkRef {
    #[n(0)]
    pub index: u32,
    #[n(1)]
    pub chunk_id: ChunkId,
    #[n(2)]
    pub size: u32,
}

#[derive(Clone, Debug, Decode, Encode, Eq, PartialEq)]
pub struct ObjectEnvelope {
    #[n(0)]
    pub object_id: ObjectId,
    #[n(1)]
    pub body: ObjectBody,
    #[n(2)]
    pub author_public_key: [u8; 32],
    #[n(3)]
    pub author_signature: [u8; 64],
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PreparedObject {
    pub body: ObjectBody,
    pub chunks: Vec<Vec<u8>>,
}

pub struct AgentKeypair {
    signing_key: SigningKey,
}

impl AgentKeypair {
    pub fn from_seed(seed: [u8; 32]) -> Self {
        Self {
            signing_key: SigningKey::from_bytes(&seed),
        }
    }

    pub fn public_key(&self) -> [u8; 32] {
        self.signing_key.verifying_key().to_bytes()
    }

    pub fn agent_id(&self) -> AgentId {
        AgentId::from_public_key(&self.public_key())
    }

    pub fn sign_object(&self, body: ObjectBody) -> Result<ObjectEnvelope> {
        if body.author != self.agent_id() {
            return Err(Error::InvalidObjectSignature);
        }
        body.validate()?;

        let canonical_body = canonical_body_bytes(&body)?;
        let object_id = ObjectId::from_canonical_body(&canonical_body);
        let signing_payload = object_signing_payload(&object_id, &canonical_body);
        let signature = self.signing_key.sign(&signing_payload).to_bytes();

        Ok(ObjectEnvelope {
            object_id,
            body,
            author_public_key: self.public_key(),
            author_signature: signature,
        })
    }
}

impl ObjectBody {
    pub fn prepare(
        kind: ObjectKind,
        author: AgentId,
        created_at_ms: u64,
        tags: Vec<String>,
        references: Vec<ObjectId>,
        mime_type: impl Into<String>,
        payload_bytes: Vec<u8>,
    ) -> Result<PreparedObject> {
        if payload_bytes.len() > MAX_PAYLOAD_SIZE {
            return Err(Error::PayloadTooLarge);
        }

        let mime_type = mime_type.into();
        let (payload, chunks) = if payload_bytes.len() <= INLINE_OBJECT_THRESHOLD {
            (
                Payload::Inline(InlinePayload {
                    mime_type,
                    bytes: payload_bytes,
                }),
                Vec::new(),
            )
        } else {
            let (chunks, chunk_refs) = chunk_payload(&payload_bytes, default_chunk_size());
            (
                Payload::Chunked(ChunkedPayload {
                    mime_type,
                    total_size: payload_bytes.len() as u64,
                    chunks: chunk_refs,
                }),
                chunks,
            )
        };

        let body = Self {
            schema_version: 1,
            kind,
            author,
            created_at_ms,
            tags,
            references,
            payload,
        };
        body.validate()?;
        Ok(PreparedObject { body, chunks })
    }

    pub fn object_id(&self) -> Result<ObjectId> {
        self.validate()?;
        Ok(ObjectId::from_canonical_body(&canonical_body_bytes(self)?))
    }

    pub fn validate(&self) -> Result<()> {
        if self.schema_version != 1 {
            return Err(Error::InvalidObjectBody);
        }

        match &self.payload {
            Payload::Inline(payload) => {
                if payload.bytes.len() > INLINE_OBJECT_THRESHOLD {
                    return Err(Error::InvalidObjectBody);
                }
            }
            Payload::Chunked(payload) => {
                if payload.total_size == 0 || payload.chunks.is_empty() {
                    return Err(Error::InvalidObjectBody);
                }

                let mut total_size = 0_u64;
                for (expected_index, chunk) in payload.chunks.iter().enumerate() {
                    if chunk.index != expected_index as u32 || chunk.size == 0 {
                        return Err(Error::InvalidObjectBody);
                    }
                    total_size += u64::from(chunk.size);
                }

                if total_size != payload.total_size || total_size <= INLINE_OBJECT_THRESHOLD as u64
                {
                    return Err(Error::InvalidObjectBody);
                }
            }
        }

        Ok(())
    }
}

impl ObjectEnvelope {
    pub fn verify(&self) -> Result<()> {
        self.body.validate()?;

        let canonical_body = canonical_body_bytes(&self.body)?;
        let expected_object_id = ObjectId::from_canonical_body(&canonical_body);
        if self.object_id != expected_object_id {
            return Err(Error::InvalidObjectId);
        }

        let expected_agent_id = AgentId::from_public_key(&self.author_public_key);
        if self.body.author != expected_agent_id {
            return Err(Error::InvalidObjectSignature);
        }

        let verifying_key = VerifyingKey::from_bytes(&self.author_public_key)
            .map_err(|_| Error::InvalidObjectSignature)?;
        let signature = Signature::from_bytes(&self.author_signature);
        let signing_payload = object_signing_payload(&self.object_id, &canonical_body);
        verifying_key
            .verify(&signing_payload, &signature)
            .map_err(|_| Error::InvalidObjectSignature)
    }
}

pub fn chunk_payload(payload: &[u8], chunk_size: NonZeroUsize) -> (Vec<Vec<u8>>, Vec<ChunkRef>) {
    let chunks: Vec<Vec<u8>> = payload
        .chunks(chunk_size.get())
        .map(std::borrow::ToOwned::to_owned)
        .collect();
    let refs = chunks
        .iter()
        .enumerate()
        .map(|(index, chunk)| ChunkRef {
            index: index as u32,
            chunk_id: ChunkId::from_chunk_bytes(chunk),
            size: chunk.len() as u32,
        })
        .collect();
    (chunks, refs)
}

fn default_chunk_size() -> NonZeroUsize {
    NonZeroUsize::new(DEFAULT_CHUNK_SIZE).expect("DEFAULT_CHUNK_SIZE must be non-zero")
}

pub fn verify_chunk(chunk_id: ChunkId, bytes: &[u8]) -> Result<()> {
    if ChunkId::from_chunk_bytes(bytes) == chunk_id {
        Ok(())
    } else {
        Err(Error::InvalidChunkId)
    }
}

fn canonical_body_bytes(body: &ObjectBody) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    minicbor::encode(body, &mut bytes).map_err(|_| Error::CborEncode)?;
    Ok(bytes)
}

fn object_signing_payload(object_id: &ObjectId, canonical_body: &[u8]) -> Vec<u8> {
    let mut payload = Vec::with_capacity(
        OBJECT_SIGNATURE_DOMAIN.len() + object_id.as_bytes().len() + canonical_body.len(),
    );
    payload.extend_from_slice(OBJECT_SIGNATURE_DOMAIN);
    payload.extend_from_slice(object_id.as_bytes());
    payload.extend_from_slice(canonical_body);
    payload
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keypair() -> AgentKeypair {
        AgentKeypair::from_seed([42_u8; 32])
    }

    fn body_with_payload(author: AgentId, payload: Vec<u8>) -> ObjectBody {
        ObjectBody::prepare(
            ObjectKind::Fact,
            author,
            1_700_000_000_000,
            vec!["rust".to_owned(), "libp2p".to_owned()],
            Vec::new(),
            "text/plain",
            payload,
        )
        .unwrap()
        .body
    }

    #[test]
    fn same_body_produces_same_object_id() {
        let author = keypair().agent_id();
        let body = body_with_payload(author, b"hello".to_vec());
        assert_eq!(body.object_id().unwrap(), body.object_id().unwrap());
    }

    #[test]
    fn changed_payload_changes_object_id() {
        let author = keypair().agent_id();
        let one = body_with_payload(author, b"hello".to_vec());
        let two = body_with_payload(author, b"goodbye".to_vec());
        assert_ne!(one.object_id().unwrap(), two.object_id().unwrap());
    }

    #[test]
    fn changed_author_changes_object_id() {
        let one_author = AgentKeypair::from_seed([1_u8; 32]).agent_id();
        let two_author = AgentKeypair::from_seed([2_u8; 32]).agent_id();
        let one = body_with_payload(one_author, b"hello".to_vec());
        let two = body_with_payload(two_author, b"hello".to_vec());
        assert_ne!(one.object_id().unwrap(), two.object_id().unwrap());
    }

    #[test]
    fn same_chunk_bytes_produce_same_chunk_id() {
        let one = ChunkId::from_chunk_bytes(b"chunk");
        let two = ChunkId::from_chunk_bytes(b"chunk");
        assert_eq!(one, two);
    }

    #[test]
    fn valid_signature_is_accepted() {
        let keypair = keypair();
        let body = body_with_payload(keypair.agent_id(), b"signed".to_vec());
        let envelope = keypair.sign_object(body).unwrap();
        assert_eq!(envelope.verify(), Ok(()));
    }

    #[test]
    fn invalid_signature_is_rejected() {
        let keypair = keypair();
        let body = body_with_payload(keypair.agent_id(), b"signed".to_vec());
        let mut envelope = keypair.sign_object(body).unwrap();
        envelope.author_signature[0] ^= 0xff;
        assert_eq!(envelope.verify(), Err(Error::InvalidObjectSignature));
    }

    #[test]
    fn payload_at_inline_threshold_stays_inline() {
        let body = body_with_payload(keypair().agent_id(), vec![1_u8; INLINE_OBJECT_THRESHOLD]);
        assert!(matches!(body.payload, Payload::Inline(_)));
    }

    #[test]
    fn payload_above_inline_threshold_becomes_chunked_refs() {
        let prepared = ObjectBody::prepare(
            ObjectKind::Fact,
            keypair().agent_id(),
            1_700_000_000_000,
            Vec::new(),
            Vec::new(),
            "text/plain",
            vec![1_u8; INLINE_OBJECT_THRESHOLD + 1],
        )
        .unwrap();

        match prepared.body.payload {
            Payload::Chunked(payload) => {
                assert_eq!(payload.total_size, (INLINE_OBJECT_THRESHOLD + 1) as u64);
                assert_eq!(payload.chunks.len(), 1);
                assert_eq!(prepared.chunks.len(), 1);
                assert_eq!(
                    payload.chunks[0].chunk_id,
                    ChunkId::from_chunk_bytes(&prepared.chunks[0])
                );
            }
            Payload::Inline(_) => panic!("expected chunked payload"),
        }
    }

    #[test]
    fn invalid_chunked_payload_is_rejected() {
        let body = ObjectBody {
            schema_version: 1,
            kind: ObjectKind::Fact,
            author: keypair().agent_id(),
            created_at_ms: 1_700_000_000_000,
            tags: Vec::new(),
            references: Vec::new(),
            payload: Payload::Chunked(ChunkedPayload {
                mime_type: "text/plain".to_owned(),
                total_size: 999,
                chunks: Vec::new(),
            }),
        };

        assert_eq!(body.validate(), Err(Error::InvalidObjectBody));
    }

    #[test]
    fn chunk_payload_uses_nonzero_chunk_size() {
        let chunk_size = NonZeroUsize::new(3).unwrap();
        let (chunks, refs) = chunk_payload(b"abcdefg", chunk_size);
        assert_eq!(
            chunks,
            vec![b"abc".to_vec(), b"def".to_vec(), b"g".to_vec()]
        );
        assert_eq!(refs.len(), 3);
    }

    #[test]
    fn chunk_verification_rejects_wrong_bytes() {
        let id = ChunkId::from_chunk_bytes(b"right");
        assert_eq!(verify_chunk(id, b"wrong"), Err(Error::InvalidChunkId));
    }
}
