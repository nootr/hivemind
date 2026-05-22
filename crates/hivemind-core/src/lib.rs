//! Core HIVEMIND protocol types.
//!
//! This crate intentionally has no async runtime, networking, filesystem or
//! database dependencies. It owns deterministic IDs, canonical encoding and
//! object envelope signing/verification.

mod error;
mod ids;
mod object;

pub use error::{Error, Result};
pub use ids::{AgentId, ChunkId, ObjectId};
pub use object::{
    chunk_payload, verify_chunk, AgentKeypair, ChunkRef, ChunkedPayload, InlinePayload, ObjectBody,
    ObjectEnvelope, ObjectKind, Payload, PreparedObject, DEFAULT_CHUNK_SIZE,
    INLINE_OBJECT_THRESHOLD, MAX_PAYLOAD_SIZE,
};
