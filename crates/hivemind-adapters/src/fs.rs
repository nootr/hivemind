use async_trait::async_trait;
use hivemind_app::{AppError, AppResult, ChunkStorePort, ObjectStorePort};
use hivemind_core::{verify_chunk, ChunkId, ObjectEnvelope, ObjectId};
use std::{
    io::ErrorKind,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};
use tokio::{
    fs,
    io::{AsyncWriteExt, BufWriter},
};

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Debug)]
pub struct FsContentStore {
    data_dir: PathBuf,
}

#[derive(Debug, thiserror::Error)]
pub enum FsStoreError {
    #[error("filesystem io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("cbor encode error")]
    CborEncode,

    #[error("cbor decode error")]
    CborDecode,

    #[error("object failed verification: {0}")]
    ObjectVerification(#[from] hivemind_core::Error),

    #[error("existing content differs from expected content")]
    ContentMismatch,
}

pub type FsStoreResult<T> = std::result::Result<T, FsStoreError>;

impl FsContentStore {
    pub fn new(data_dir: impl Into<PathBuf>) -> Self {
        Self {
            data_dir: data_dir.into(),
        }
    }

    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    pub fn object_path(&self, object_id: ObjectId) -> PathBuf {
        content_path(
            &self.data_dir,
            "objects",
            &object_id.to_string(),
            Some(".cbor"),
        )
    }

    pub fn chunk_path(&self, chunk_id: ChunkId) -> PathBuf {
        content_path(&self.data_dir, "chunks", &chunk_id.to_string(), None)
    }

    pub async fn put_object_envelope(&self, envelope: &ObjectEnvelope) -> FsStoreResult<()> {
        envelope.verify()?;
        let mut bytes = Vec::new();
        minicbor::encode(envelope, &mut bytes).map_err(|_| FsStoreError::CborEncode)?;
        atomic_write_once(self.object_path(envelope.object_id), &bytes).await
    }

    pub async fn get_object(&self, object_id: ObjectId) -> FsStoreResult<ObjectEnvelope> {
        let bytes = fs::read(self.object_path(object_id)).await?;
        let envelope: ObjectEnvelope =
            minicbor::decode(&bytes).map_err(|_| FsStoreError::CborDecode)?;
        if envelope.object_id != object_id {
            return Err(FsStoreError::ObjectVerification(
                hivemind_core::Error::InvalidObjectId,
            ));
        }
        envelope.verify()?;
        Ok(envelope)
    }

    pub async fn has_object(&self, object_id: ObjectId) -> FsStoreResult<bool> {
        path_exists(self.object_path(object_id)).await
    }

    pub async fn put_chunk_bytes(&self, chunk_id: ChunkId, bytes: &[u8]) -> FsStoreResult<()> {
        verify_chunk(chunk_id, bytes)?;
        atomic_write_once(self.chunk_path(chunk_id), bytes).await
    }

    pub async fn get_chunk(&self, chunk_id: ChunkId) -> FsStoreResult<Vec<u8>> {
        let bytes = fs::read(self.chunk_path(chunk_id)).await?;
        verify_chunk(chunk_id, &bytes)?;
        Ok(bytes)
    }

    pub async fn has_chunk(&self, chunk_id: ChunkId) -> FsStoreResult<bool> {
        path_exists(self.chunk_path(chunk_id)).await
    }
}

#[async_trait]
impl ObjectStorePort for FsContentStore {
    async fn put_object(&self, envelope: ObjectEnvelope) -> AppResult<()> {
        self.put_object_envelope(&envelope)
            .await
            .map_err(|err| AppError::ObjectStore(err.to_string()))
    }
}

#[async_trait]
impl ChunkStorePort for FsContentStore {
    async fn put_chunk(&self, chunk_id: ChunkId, bytes: Vec<u8>) -> AppResult<()> {
        self.put_chunk_bytes(chunk_id, &bytes)
            .await
            .map_err(|err| AppError::ChunkStore(err.to_string()))
    }
}

fn content_path(
    root: &std::path::Path,
    kind: &str,
    hex_id: &str,
    extension: Option<&str>,
) -> PathBuf {
    let prefix = &hex_id[..2];
    let filename = match extension {
        Some(extension) => format!("{hex_id}{extension}"),
        None => hex_id.to_owned(),
    };
    root.join(kind).join(prefix).join(filename)
}

async fn path_exists(path: PathBuf) -> FsStoreResult<bool> {
    match fs::metadata(path).await {
        Ok(_) => Ok(true),
        Err(err) if err.kind() == ErrorKind::NotFound => Ok(false),
        Err(err) => Err(FsStoreError::Io(err)),
    }
}

async fn atomic_write_once(path: PathBuf, bytes: &[u8]) -> FsStoreResult<()> {
    if path_exists(path.clone()).await? {
        return ensure_existing_content_matches(&path, bytes).await;
    }

    let parent = path
        .parent()
        .expect("content-addressed paths always have a parent");
    fs::create_dir_all(parent).await?;

    let tmp_path = temp_path(&path);
    let file = fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&tmp_path)
        .await;

    let file = match file {
        Ok(file) => file,
        Err(err) if err.kind() == ErrorKind::AlreadyExists => {
            let _ = fs::remove_file(&tmp_path).await;
            fs::OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&tmp_path)
                .await?
        }
        Err(err) => return Err(FsStoreError::Io(err)),
    };

    let mut writer = BufWriter::new(file);
    writer.write_all(bytes).await?;
    writer.flush().await?;
    let file = writer.into_inner();
    file.sync_all().await?;

    match fs::rename(&tmp_path, &path).await {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == ErrorKind::AlreadyExists => {
            let _ = fs::remove_file(&tmp_path).await;
            ensure_existing_content_matches(&path, bytes).await
        }
        Err(err) if path_exists(path.clone()).await? => {
            let _ = fs::remove_file(&tmp_path).await;
            let _ = err;
            ensure_existing_content_matches(&path, bytes).await
        }
        Err(err) => Err(FsStoreError::Io(err)),
    }
}

async fn ensure_existing_content_matches(path: &Path, expected: &[u8]) -> FsStoreResult<()> {
    let existing = fs::read(path).await?;
    if existing == expected {
        Ok(())
    } else {
        Err(FsStoreError::ContentMismatch)
    }
}

fn temp_path(path: &std::path::Path) -> PathBuf {
    let filename = path
        .file_name()
        .expect("content-addressed paths always have a filename")
        .to_string_lossy();
    let counter = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    path.with_file_name(format!(".{filename}.tmp-{}-{counter}", std::process::id()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use hivemind_app::{ClockPort, IdentityPort, PublishObject, PublishObjectInput};
    use hivemind_core::{
        AgentId, AgentKeypair, ObjectBody, ObjectKind, Payload, INLINE_OBJECT_THRESHOLD,
    };

    struct TestIdentity {
        keypair: AgentKeypair,
    }

    impl TestIdentity {
        fn new() -> Self {
            Self {
                keypair: AgentKeypair::from_seed([9_u8; 32]),
            }
        }
    }

    #[async_trait]
    impl IdentityPort for TestIdentity {
        async fn agent_id(&self) -> AppResult<AgentId> {
            Ok(self.keypair.agent_id())
        }

        async fn sign_object(&self, body: ObjectBody) -> AppResult<ObjectEnvelope> {
            Ok(self.keypair.sign_object(body)?)
        }
    }

    struct TestClock;

    #[async_trait]
    impl ClockPort for TestClock {
        async fn now_ms(&self) -> AppResult<u64> {
            Ok(1_700_000_000_000)
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

    #[tokio::test]
    async fn stores_inline_object_envelope_to_expected_path() {
        let tempdir = tempfile::tempdir().unwrap();
        let store = FsContentStore::new(tempdir.path());
        let identity = TestIdentity::new();
        let clock = TestClock;

        let output = PublishObject::new(&identity, &clock, &store, &store)
            .execute(input(b"hello".to_vec()))
            .await
            .unwrap();

        let path = store.object_path(output.object_id);
        assert!(path.exists());
        assert!(path.to_string_lossy().contains("/objects/"));
        assert!(path.to_string_lossy().ends_with(".cbor"));
    }

    #[tokio::test]
    async fn stores_chunk_to_expected_path() {
        let tempdir = tempfile::tempdir().unwrap();
        let store = FsContentStore::new(tempdir.path());
        let identity = TestIdentity::new();
        let clock = TestClock;

        let output = PublishObject::new(&identity, &clock, &store, &store)
            .execute(input(vec![1_u8; INLINE_OBJECT_THRESHOLD + 1]))
            .await
            .unwrap();

        assert_eq!(output.chunk_ids.len(), 1);
        let path = store.chunk_path(output.chunk_ids[0]);
        assert!(path.exists());
        assert!(path.to_string_lossy().contains("/chunks/"));
    }

    #[tokio::test]
    async fn duplicate_writes_are_idempotent() {
        let tempdir = tempfile::tempdir().unwrap();
        let store = FsContentStore::new(tempdir.path());
        let chunk_id = ChunkId::from_chunk_bytes(b"same");

        store.put_chunk_bytes(chunk_id, b"same").await.unwrap();
        store.put_chunk_bytes(chunk_id, b"same").await.unwrap();

        assert_eq!(store.get_chunk(chunk_id).await.unwrap(), b"same");
    }

    #[tokio::test]
    async fn existing_different_content_is_rejected() {
        let tempdir = tempfile::tempdir().unwrap();
        let store = FsContentStore::new(tempdir.path());
        let chunk_id = ChunkId::from_chunk_bytes(b"same");
        let path = store.chunk_path(chunk_id);
        fs::create_dir_all(path.parent().unwrap()).await.unwrap();
        fs::write(&path, b"corrupt").await.unwrap();

        let err = store.put_chunk_bytes(chunk_id, b"same").await.unwrap_err();

        assert!(matches!(err, FsStoreError::ContentMismatch));
        assert_eq!(fs::read(&path).await.unwrap(), b"corrupt");
    }

    #[tokio::test]
    async fn object_roundtrip_decodes_and_verifies() {
        let tempdir = tempfile::tempdir().unwrap();
        let store = FsContentStore::new(tempdir.path());
        let identity = TestIdentity::new();
        let clock = TestClock;

        let output = PublishObject::new(&identity, &clock, &store, &store)
            .execute(input(b"roundtrip".to_vec()))
            .await
            .unwrap();

        let envelope = store.get_object(output.object_id).await.unwrap();
        envelope.verify().unwrap();
        assert!(matches!(envelope.body.payload, Payload::Inline(_)));
    }

    #[tokio::test]
    async fn chunk_roundtrip_verifies_hash() {
        let tempdir = tempfile::tempdir().unwrap();
        let store = FsContentStore::new(tempdir.path());
        let chunk_id = ChunkId::from_chunk_bytes(b"chunk bytes");

        store
            .put_chunk_bytes(chunk_id, b"chunk bytes")
            .await
            .unwrap();

        let bytes = store.get_chunk(chunk_id).await.unwrap();
        assert_eq!(bytes, b"chunk bytes");
    }

    #[tokio::test]
    async fn publish_use_case_works_with_fs_content_store() {
        let tempdir = tempfile::tempdir().unwrap();
        let store = FsContentStore::new(tempdir.path());
        let identity = TestIdentity::new();
        let clock = TestClock;

        let output = PublishObject::new(&identity, &clock, &store, &store)
            .execute(input(vec![2_u8; INLINE_OBJECT_THRESHOLD + 1]))
            .await
            .unwrap();

        assert!(store.has_object(output.object_id).await.unwrap());
        for chunk_id in output.chunk_ids {
            assert!(store.has_chunk(chunk_id).await.unwrap());
        }
    }
}
