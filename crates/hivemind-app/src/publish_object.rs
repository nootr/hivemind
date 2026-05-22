use crate::{AppResult, ChunkStorePort, ClockPort, IdentityPort, ObjectStorePort};
use hivemind_core::{ChunkId, ObjectBody, ObjectId, ObjectKind, Payload};

pub struct PublishObject<'a, I, C, O, S>
where
    I: IdentityPort + ?Sized,
    C: ClockPort + ?Sized,
    O: ObjectStorePort,
    S: ChunkStorePort,
{
    identity: &'a I,
    clock: &'a C,
    object_store: &'a O,
    chunk_store: &'a S,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PublishObjectInput {
    pub kind: ObjectKind,
    pub mime_type: String,
    pub payload: Vec<u8>,
    pub tags: Vec<String>,
    pub references: Vec<ObjectId>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PublishObjectOutput {
    pub object_id: ObjectId,
    pub chunk_ids: Vec<ChunkId>,
}

impl<'a, I, C, O, S> PublishObject<'a, I, C, O, S>
where
    I: IdentityPort + ?Sized,
    C: ClockPort + ?Sized,
    O: ObjectStorePort,
    S: ChunkStorePort,
{
    pub fn new(identity: &'a I, clock: &'a C, object_store: &'a O, chunk_store: &'a S) -> Self {
        Self {
            identity,
            clock,
            object_store,
            chunk_store,
        }
    }

    pub async fn execute(&self, input: PublishObjectInput) -> AppResult<PublishObjectOutput> {
        let author = self.identity.agent_id().await?;
        let created_at_ms = self.clock.now_ms().await?;
        let prepared = ObjectBody::prepare(
            input.kind,
            author,
            created_at_ms,
            input.tags,
            input.references,
            input.mime_type,
            input.payload,
        )?;

        let chunk_ids = match &prepared.body.payload {
            Payload::Inline(_) => Vec::new(),
            Payload::Chunked(payload) => {
                payload.chunks.iter().map(|chunk| chunk.chunk_id).collect()
            }
        };

        let envelope = self.identity.sign_object(prepared.body).await?;
        envelope.verify()?;
        let object_id = envelope.object_id;
        self.object_store.put_object(envelope).await?;

        for (chunk_id, bytes) in chunk_ids.iter().copied().zip(prepared.chunks) {
            self.chunk_store.put_chunk(chunk_id, bytes).await?;
        }

        Ok(PublishObjectOutput {
            object_id,
            chunk_ids,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AppError, AppResult};
    use async_trait::async_trait;
    use hivemind_core::{verify_chunk, AgentKeypair, ObjectEnvelope, INLINE_OBJECT_THRESHOLD};
    use std::sync::Mutex;

    struct MockIdentity {
        keypair: AgentKeypair,
    }

    impl MockIdentity {
        fn new(seed: [u8; 32]) -> Self {
            Self {
                keypair: AgentKeypair::from_seed(seed),
            }
        }
    }

    #[async_trait]
    impl IdentityPort for MockIdentity {
        async fn agent_id(&self) -> AppResult<hivemind_core::AgentId> {
            Ok(self.keypair.agent_id())
        }

        async fn sign_object(&self, body: ObjectBody) -> AppResult<ObjectEnvelope> {
            Ok(self.keypair.sign_object(body)?)
        }
    }

    struct MockClock {
        now_ms: u64,
    }

    #[async_trait]
    impl ClockPort for MockClock {
        async fn now_ms(&self) -> AppResult<u64> {
            Ok(self.now_ms)
        }
    }

    #[derive(Default)]
    struct MockObjectStore {
        objects: Mutex<Vec<ObjectEnvelope>>,
        fail: bool,
    }

    #[async_trait]
    impl ObjectStorePort for MockObjectStore {
        async fn put_object(&self, envelope: ObjectEnvelope) -> AppResult<()> {
            if self.fail {
                return Err(AppError::ObjectStore("boom".to_owned()));
            }
            self.objects.lock().unwrap().push(envelope);
            Ok(())
        }
    }

    #[derive(Default)]
    struct MockChunkStore {
        chunks: Mutex<Vec<(ChunkId, Vec<u8>)>>,
    }

    #[async_trait]
    impl ChunkStorePort for MockChunkStore {
        async fn put_chunk(&self, chunk_id: ChunkId, bytes: Vec<u8>) -> AppResult<()> {
            self.chunks.lock().unwrap().push((chunk_id, bytes));
            Ok(())
        }
    }

    fn input(payload: Vec<u8>) -> PublishObjectInput {
        PublishObjectInput {
            kind: ObjectKind::Fact,
            mime_type: "text/plain".to_owned(),
            payload,
            tags: vec!["rust".to_owned()],
            references: Vec::new(),
        }
    }

    fn service<'a>(
        identity: &'a MockIdentity,
        clock: &'a MockClock,
        object_store: &'a MockObjectStore,
        chunk_store: &'a MockChunkStore,
    ) -> PublishObject<'a, MockIdentity, MockClock, MockObjectStore, MockChunkStore> {
        PublishObject::new(identity, clock, object_store, chunk_store)
    }

    #[test]
    fn inline_payload_stores_no_chunks() {
        run(async {
            let identity = MockIdentity::new([1_u8; 32]);
            let clock = MockClock { now_ms: 42 };
            let object_store = MockObjectStore::default();
            let chunk_store = MockChunkStore::default();

            let output = service(&identity, &clock, &object_store, &chunk_store)
                .execute(input(b"hello".to_vec()))
                .await
                .unwrap();

            assert!(output.chunk_ids.is_empty());
            assert!(chunk_store.chunks.lock().unwrap().is_empty());
            assert_eq!(object_store.objects.lock().unwrap().len(), 1);
        });
    }

    #[test]
    fn chunked_payload_stores_expected_chunks() {
        run(async {
            let identity = MockIdentity::new([2_u8; 32]);
            let clock = MockClock { now_ms: 42 };
            let object_store = MockObjectStore::default();
            let chunk_store = MockChunkStore::default();
            let payload = vec![7_u8; INLINE_OBJECT_THRESHOLD + 1];

            let output = service(&identity, &clock, &object_store, &chunk_store)
                .execute(input(payload.clone()))
                .await
                .unwrap();

            let stored_chunks = chunk_store.chunks.lock().unwrap();
            assert_eq!(stored_chunks.len(), 1);
            assert_eq!(output.chunk_ids, vec![stored_chunks[0].0]);
            verify_chunk(stored_chunks[0].0, &stored_chunks[0].1).unwrap();
        });
    }

    #[test]
    fn returned_object_verifies_and_is_stored() {
        run(async {
            let identity = MockIdentity::new([3_u8; 32]);
            let clock = MockClock {
                now_ms: 1_700_000_000_000,
            };
            let object_store = MockObjectStore::default();
            let chunk_store = MockChunkStore::default();

            let output = service(&identity, &clock, &object_store, &chunk_store)
                .execute(input(b"signed object".to_vec()))
                .await
                .unwrap();

            let stored = object_store.objects.lock().unwrap();
            assert_eq!(stored.len(), 1);
            assert_eq!(stored[0].object_id, output.object_id);
            assert_eq!(stored[0].body.author, identity.keypair.agent_id());
            assert_eq!(stored[0].body.created_at_ms, clock.now_ms);
            assert!(matches!(stored[0].body.payload, Payload::Inline(_)));
            stored[0].verify().unwrap();
        });
    }

    #[test]
    fn object_store_failure_does_not_store_chunks() {
        run(async {
            let identity = MockIdentity::new([4_u8; 32]);
            let clock = MockClock { now_ms: 42 };
            let object_store = MockObjectStore {
                objects: Mutex::new(Vec::new()),
                fail: true,
            };
            let chunk_store = MockChunkStore::default();
            let payload = vec![7_u8; INLINE_OBJECT_THRESHOLD + 1];

            let err = service(&identity, &clock, &object_store, &chunk_store)
                .execute(input(payload))
                .await
                .unwrap_err();

            assert_eq!(err, AppError::ObjectStore("boom".to_owned()));
            assert!(object_store.objects.lock().unwrap().is_empty());
            assert!(chunk_store.chunks.lock().unwrap().is_empty());
        });
    }

    #[test]
    fn payload_too_large_fails_without_storing() {
        run(async {
            let identity = MockIdentity::new([4_u8; 32]);
            let clock = MockClock { now_ms: 42 };
            let object_store = MockObjectStore::default();
            let chunk_store = MockChunkStore::default();
            let payload = vec![0_u8; hivemind_core::MAX_PAYLOAD_SIZE + 1];

            let err = service(&identity, &clock, &object_store, &chunk_store)
                .execute(input(payload))
                .await
                .unwrap_err();

            assert_eq!(err, AppError::Core(hivemind_core::Error::PayloadTooLarge));
            assert!(object_store.objects.lock().unwrap().is_empty());
            assert!(chunk_store.chunks.lock().unwrap().is_empty());
        });
    }

    fn run(future: impl std::future::Future<Output = ()>) {
        futures::executor::block_on(future);
    }
}
