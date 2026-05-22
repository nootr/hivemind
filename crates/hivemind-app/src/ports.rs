use crate::AppResult;
use async_trait::async_trait;
use hivemind_core::{AgentId, ChunkId, ObjectBody, ObjectEnvelope};

#[async_trait]
pub trait IdentityPort: Send + Sync {
    async fn agent_id(&self) -> AppResult<AgentId>;
    async fn sign_object(&self, body: ObjectBody) -> AppResult<ObjectEnvelope>;
}

#[async_trait]
pub trait ClockPort: Send + Sync {
    async fn now_ms(&self) -> AppResult<u64>;
}

#[async_trait]
pub trait ObjectStorePort: Send + Sync {
    async fn put_object(&self, envelope: ObjectEnvelope) -> AppResult<()>;
}

#[async_trait]
pub trait ChunkStorePort: Send + Sync {
    async fn put_chunk(&self, chunk_id: ChunkId, bytes: Vec<u8>) -> AppResult<()>;
}
