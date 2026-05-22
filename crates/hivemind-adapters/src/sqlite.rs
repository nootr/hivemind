use hivemind_core::{AgentId, ChunkId, ObjectEnvelope, ObjectId, ObjectKind, Payload};
use rusqlite::{params, Connection, OptionalExtension};
use std::{collections::BTreeMap, path::Path, sync::Mutex};

#[derive(Debug, thiserror::Error)]
pub enum SqliteStoreError {
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),

    #[error("metadata store lock poisoned")]
    LockPoisoned,

    #[error("invalid object metadata")]
    InvalidObjectMetadata,

    #[error("object failed verification: {0}")]
    ObjectVerification(#[from] hivemind_core::Error),
}

pub type SqliteStoreResult<T> = std::result::Result<T, SqliteStoreError>;

#[derive(Debug)]
pub struct SqliteMetadataStore {
    connection: Mutex<Connection>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StoredChunk {
    pub chunk_id: ChunkId,
    pub position: u32,
    pub size: u32,
    pub path: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ObjectMetadata {
    pub object_id: ObjectId,
    pub object_kind: ObjectKind,
    pub author_agent_id: AgentId,
    pub created_at_ms: u64,
    pub mime_type: String,
    pub payload_size: u64,
    pub chunk_count: u32,
    pub object_path: String,
    pub received_at_ms: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ChunkMetadata {
    pub chunk_id: ChunkId,
    pub size: u32,
    pub path: String,
    pub received_at_ms: u64,
}

impl SqliteMetadataStore {
    pub fn open(path: impl AsRef<Path>) -> SqliteStoreResult<Self> {
        let connection = Connection::open(path)?;
        connection.execute_batch("PRAGMA foreign_keys = ON;")?;
        let store = Self {
            connection: Mutex::new(connection),
        };
        store.migrate()?;
        Ok(store)
    }

    pub fn in_memory() -> SqliteStoreResult<Self> {
        let connection = Connection::open_in_memory()?;
        connection.execute_batch("PRAGMA foreign_keys = ON;")?;
        let store = Self {
            connection: Mutex::new(connection),
        };
        store.migrate()?;
        Ok(store)
    }

    pub fn migrate(&self) -> SqliteStoreResult<()> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| SqliteStoreError::LockPoisoned)?;
        connection.execute_batch(MIGRATIONS)?;
        Ok(())
    }

    pub fn record_object(
        &self,
        envelope: &ObjectEnvelope,
        object_path: impl AsRef<Path>,
        chunks: &[StoredChunk],
        received_at_ms: u64,
    ) -> SqliteStoreResult<()> {
        envelope.verify()?;
        let payload_metadata = payload_metadata(&envelope.body.payload)?;
        validate_stored_chunks(&envelope.body.payload, chunks)?;

        let mut connection = self
            .connection
            .lock()
            .map_err(|_| SqliteStoreError::LockPoisoned)?;
        let tx = connection.transaction()?;
        tx.execute(
            "INSERT INTO objects (
                object_id, object_kind, author_agent_id, created_at_ms, mime_type,
                payload_size, chunk_count, object_path, received_at_ms
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
             ON CONFLICT(object_id) DO UPDATE SET
                object_kind = excluded.object_kind,
                author_agent_id = excluded.author_agent_id,
                created_at_ms = excluded.created_at_ms,
                mime_type = excluded.mime_type,
                payload_size = excluded.payload_size,
                chunk_count = excluded.chunk_count,
                object_path = excluded.object_path,
                received_at_ms = excluded.received_at_ms",
            params![
                envelope.object_id.to_string(),
                object_kind_to_str(envelope.body.kind),
                envelope.body.author.to_string(),
                envelope.body.created_at_ms as i64,
                payload_metadata.mime_type,
                payload_metadata.payload_size as i64,
                payload_metadata.chunk_count as i64,
                object_path.as_ref().to_string_lossy().as_ref(),
                received_at_ms as i64,
            ],
        )?;

        tx.execute(
            "DELETE FROM object_chunks WHERE object_id = ?1",
            params![envelope.object_id.to_string()],
        )?;
        tx.execute(
            "DELETE FROM tags WHERE object_id = ?1",
            params![envelope.object_id.to_string()],
        )?;

        for chunk in chunks {
            tx.execute(
                "INSERT INTO chunks (chunk_id, size, path, received_at_ms)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(chunk_id) DO UPDATE SET
                    size = excluded.size,
                    path = excluded.path,
                    received_at_ms = excluded.received_at_ms",
                params![
                    chunk.chunk_id.to_string(),
                    chunk.size as i64,
                    chunk.path,
                    received_at_ms as i64,
                ],
            )?;
            tx.execute(
                "INSERT INTO object_chunks (object_id, chunk_id, position, size)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(object_id, position) DO UPDATE SET
                    chunk_id = excluded.chunk_id,
                    size = excluded.size",
                params![
                    envelope.object_id.to_string(),
                    chunk.chunk_id.to_string(),
                    chunk.position as i64,
                    chunk.size as i64,
                ],
            )?;
        }

        for tag in &envelope.body.tags {
            tx.execute(
                "INSERT OR IGNORE INTO tags (tag, object_id) VALUES (?1, ?2)",
                params![tag, envelope.object_id.to_string()],
            )?;
        }

        tx.commit()?;
        Ok(())
    }

    pub fn get_object_metadata(
        &self,
        object_id: ObjectId,
    ) -> SqliteStoreResult<Option<ObjectMetadata>> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| SqliteStoreError::LockPoisoned)?;
        connection
            .query_row(
                "SELECT object_id, object_kind, author_agent_id, created_at_ms, mime_type,
                        payload_size, chunk_count, object_path, received_at_ms
                 FROM objects WHERE object_id = ?1",
                params![object_id.to_string()],
                object_metadata_from_row,
            )
            .optional()
            .map_err(SqliteStoreError::Sqlite)
    }

    pub fn get_chunk_metadata(
        &self,
        chunk_id: ChunkId,
    ) -> SqliteStoreResult<Option<ChunkMetadata>> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| SqliteStoreError::LockPoisoned)?;
        connection
            .query_row(
                "SELECT chunk_id, size, path, received_at_ms FROM chunks WHERE chunk_id = ?1",
                params![chunk_id.to_string()],
                chunk_metadata_from_row,
            )
            .optional()
            .map_err(SqliteStoreError::Sqlite)
    }

    pub fn chunks_for_object(&self, object_id: ObjectId) -> SqliteStoreResult<Vec<StoredChunk>> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| SqliteStoreError::LockPoisoned)?;
        let mut statement = connection.prepare(
            "SELECT oc.chunk_id, oc.position, oc.size, c.path
             FROM object_chunks oc
             JOIN chunks c ON c.chunk_id = oc.chunk_id
             WHERE oc.object_id = ?1
             ORDER BY oc.position ASC",
        )?;
        let rows = statement.query_map(params![object_id.to_string()], stored_chunk_from_row)?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(SqliteStoreError::Sqlite)
    }

    pub fn objects_for_tag(&self, tag: &str) -> SqliteStoreResult<Vec<ObjectId>> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| SqliteStoreError::LockPoisoned)?;
        let mut statement = connection
            .prepare("SELECT object_id FROM tags WHERE tag = ?1 ORDER BY object_id ASC")?;
        let rows = statement.query_map(params![tag], |row| {
            parse_object_id(row.get::<_, String>(0)?)
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(SqliteStoreError::Sqlite)
    }
}

struct PayloadMetadata {
    mime_type: String,
    payload_size: u64,
    chunk_count: u32,
}

fn payload_metadata(payload: &Payload) -> SqliteStoreResult<PayloadMetadata> {
    match payload {
        Payload::Inline(inline) => Ok(PayloadMetadata {
            mime_type: inline.mime_type.clone(),
            payload_size: inline.bytes.len() as u64,
            chunk_count: 0,
        }),
        Payload::Chunked(chunked) => Ok(PayloadMetadata {
            mime_type: chunked.mime_type.clone(),
            payload_size: chunked.total_size,
            chunk_count: chunked.chunks.len() as u32,
        }),
    }
}

fn validate_stored_chunks(payload: &Payload, chunks: &[StoredChunk]) -> SqliteStoreResult<()> {
    match payload {
        Payload::Inline(_) if chunks.is_empty() => Ok(()),
        Payload::Inline(_) => Err(SqliteStoreError::InvalidObjectMetadata),
        Payload::Chunked(chunked) => {
            if chunked.chunks.len() != chunks.len() {
                return Err(SqliteStoreError::InvalidObjectMetadata);
            }

            let mut chunks_by_position = BTreeMap::new();
            for chunk in chunks {
                if chunks_by_position.insert(chunk.position, chunk).is_some() {
                    return Err(SqliteStoreError::InvalidObjectMetadata);
                }
            }

            for expected in &chunked.chunks {
                let Some(actual) = chunks_by_position.get(&expected.index) else {
                    return Err(SqliteStoreError::InvalidObjectMetadata);
                };
                if actual.chunk_id != expected.chunk_id || actual.size != expected.size {
                    return Err(SqliteStoreError::InvalidObjectMetadata);
                }
            }

            Ok(())
        }
    }
}

fn object_kind_to_str(kind: ObjectKind) -> &'static str {
    match kind {
        ObjectKind::Skill => "skill",
        ObjectKind::Fact => "fact",
        ObjectKind::Procedure => "procedure",
        ObjectKind::Insight => "insight",
        ObjectKind::Rating => "rating",
        ObjectKind::Report => "report",
        ObjectKind::Tombstone => "tombstone",
        ObjectKind::Alias => "alias",
    }
}

fn object_kind_from_str(value: &str) -> rusqlite::Result<ObjectKind> {
    match value {
        "skill" => Ok(ObjectKind::Skill),
        "fact" => Ok(ObjectKind::Fact),
        "procedure" => Ok(ObjectKind::Procedure),
        "insight" => Ok(ObjectKind::Insight),
        "rating" => Ok(ObjectKind::Rating),
        "report" => Ok(ObjectKind::Report),
        "tombstone" => Ok(ObjectKind::Tombstone),
        "alias" => Ok(ObjectKind::Alias),
        _ => Err(rusqlite::Error::InvalidQuery),
    }
}

fn object_metadata_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<ObjectMetadata> {
    Ok(ObjectMetadata {
        object_id: parse_object_id(row.get::<_, String>(0)?)?,
        object_kind: object_kind_from_str(&row.get::<_, String>(1)?)?,
        author_agent_id: parse_agent_id(row.get::<_, String>(2)?)?,
        created_at_ms: row.get::<_, i64>(3)? as u64,
        mime_type: row.get(4)?,
        payload_size: row.get::<_, i64>(5)? as u64,
        chunk_count: row.get::<_, i64>(6)? as u32,
        object_path: row.get(7)?,
        received_at_ms: row.get::<_, i64>(8)? as u64,
    })
}

fn chunk_metadata_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<ChunkMetadata> {
    Ok(ChunkMetadata {
        chunk_id: parse_chunk_id(row.get::<_, String>(0)?)?,
        size: row.get::<_, i64>(1)? as u32,
        path: row.get(2)?,
        received_at_ms: row.get::<_, i64>(3)? as u64,
    })
}

fn stored_chunk_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<StoredChunk> {
    Ok(StoredChunk {
        chunk_id: parse_chunk_id(row.get::<_, String>(0)?)?,
        position: row.get::<_, i64>(1)? as u32,
        size: row.get::<_, i64>(2)? as u32,
        path: row.get(3)?,
    })
}

fn parse_object_id(value: String) -> rusqlite::Result<ObjectId> {
    value.parse().map_err(|_| rusqlite::Error::InvalidQuery)
}

fn parse_chunk_id(value: String) -> rusqlite::Result<ChunkId> {
    value.parse().map_err(|_| rusqlite::Error::InvalidQuery)
}

fn parse_agent_id(value: String) -> rusqlite::Result<AgentId> {
    value.parse().map_err(|_| rusqlite::Error::InvalidQuery)
}

const MIGRATIONS: &str = r#"
CREATE TABLE IF NOT EXISTS objects (
  object_id TEXT PRIMARY KEY NOT NULL,
  object_kind TEXT NOT NULL,
  author_agent_id TEXT NOT NULL,
  created_at_ms INTEGER NOT NULL,
  mime_type TEXT NOT NULL,
  payload_size INTEGER NOT NULL,
  chunk_count INTEGER NOT NULL,
  object_path TEXT NOT NULL,
  received_at_ms INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS chunks (
  chunk_id TEXT PRIMARY KEY NOT NULL,
  size INTEGER NOT NULL,
  path TEXT NOT NULL,
  received_at_ms INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS object_chunks (
  object_id TEXT NOT NULL,
  chunk_id TEXT NOT NULL,
  position INTEGER NOT NULL,
  size INTEGER NOT NULL,
  PRIMARY KEY (object_id, position),
  FOREIGN KEY (object_id) REFERENCES objects(object_id) ON DELETE CASCADE,
  FOREIGN KEY (chunk_id) REFERENCES chunks(chunk_id)
);

CREATE TABLE IF NOT EXISTS tags (
  tag TEXT NOT NULL,
  object_id TEXT NOT NULL,
  PRIMARY KEY (tag, object_id),
  FOREIGN KEY (object_id) REFERENCES objects(object_id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_object_chunks_object_position
  ON object_chunks (object_id, position);

CREATE INDEX IF NOT EXISTS idx_object_chunks_chunk
  ON object_chunks (chunk_id);

CREATE INDEX IF NOT EXISTS idx_tags_tag
  ON tags (tag);
"#;

#[cfg(test)]
mod tests {
    use super::*;
    use hivemind_core::{AgentKeypair, ObjectBody};

    fn signed_envelope(payload: Vec<u8>, tags: Vec<String>) -> ObjectEnvelope {
        let keypair = AgentKeypair::from_seed([3_u8; 32]);
        let prepared = ObjectBody::prepare(
            ObjectKind::Fact,
            keypair.agent_id(),
            1_700_000_000_000,
            tags,
            Vec::new(),
            "text/plain",
            payload,
        )
        .unwrap();
        keypair.sign_object(prepared.body).unwrap()
    }

    #[test]
    fn records_inline_object_metadata() {
        let store = SqliteMetadataStore::in_memory().unwrap();
        let envelope = signed_envelope(b"hello".to_vec(), vec!["rust".to_owned()]);

        store
            .record_object(&envelope, "objects/ab/object.cbor", &[], 42)
            .unwrap();

        let metadata = store
            .get_object_metadata(envelope.object_id)
            .unwrap()
            .unwrap();
        assert_eq!(metadata.object_id, envelope.object_id);
        assert_eq!(metadata.object_kind, ObjectKind::Fact);
        assert_eq!(metadata.mime_type, "text/plain");
        assert_eq!(metadata.payload_size, 5);
        assert_eq!(metadata.chunk_count, 0);
        assert_eq!(metadata.received_at_ms, 42);
        assert_eq!(
            store.objects_for_tag("rust").unwrap(),
            vec![envelope.object_id]
        );
    }

    #[test]
    fn records_chunked_object_metadata() {
        let store = SqliteMetadataStore::in_memory().unwrap();
        let keypair = AgentKeypair::from_seed([4_u8; 32]);
        let prepared = ObjectBody::prepare(
            ObjectKind::Fact,
            keypair.agent_id(),
            1_700_000_000_000,
            vec!["networking".to_owned()],
            Vec::new(),
            "text/plain",
            vec![9_u8; hivemind_core::INLINE_OBJECT_THRESHOLD + 1],
        )
        .unwrap();
        let chunks = match &prepared.body.payload {
            Payload::Chunked(payload) => payload
                .chunks
                .iter()
                .enumerate()
                .map(|(position, chunk)| StoredChunk {
                    chunk_id: chunk.chunk_id,
                    position: position as u32,
                    size: chunk.size,
                    path: format!("chunks/{position}"),
                })
                .collect::<Vec<_>>(),
            Payload::Inline(_) => panic!("expected chunked payload"),
        };
        let envelope = keypair.sign_object(prepared.body).unwrap();

        store
            .record_object(&envelope, "objects/object.cbor", &chunks, 99)
            .unwrap();

        let metadata = store
            .get_object_metadata(envelope.object_id)
            .unwrap()
            .unwrap();
        assert_eq!(metadata.chunk_count, chunks.len() as u32);
        assert_eq!(
            metadata.payload_size,
            hivemind_core::INLINE_OBJECT_THRESHOLD as u64 + 1
        );
        assert_eq!(store.chunks_for_object(envelope.object_id).unwrap(), chunks);
        assert_eq!(
            store
                .get_chunk_metadata(chunks[0].chunk_id)
                .unwrap()
                .unwrap()
                .path,
            chunks[0].path
        );
    }

    #[test]
    fn rerecording_same_object_is_idempotent_and_updates_observation_metadata() {
        let store = SqliteMetadataStore::in_memory().unwrap();
        let envelope = signed_envelope(b"hello".to_vec(), vec!["rust".to_owned()]);

        store.record_object(&envelope, "one", &[], 1).unwrap();
        store.record_object(&envelope, "two", &[], 2).unwrap();

        assert_eq!(
            store.objects_for_tag("rust").unwrap(),
            vec![envelope.object_id]
        );
        let metadata = store
            .get_object_metadata(envelope.object_id)
            .unwrap()
            .unwrap();
        assert_eq!(metadata.object_path, "two");
        assert_eq!(metadata.received_at_ms, 2);
    }

    #[test]
    fn rejects_chunk_count_mismatch() {
        let store = SqliteMetadataStore::in_memory().unwrap();
        let envelope = signed_envelope(
            vec![1_u8; hivemind_core::INLINE_OBJECT_THRESHOLD + 1],
            Vec::new(),
        );

        let err = store
            .record_object(&envelope, "object", &[], 1)
            .unwrap_err();

        assert!(matches!(err, SqliteStoreError::InvalidObjectMetadata));
    }

    #[test]
    fn rejects_chunk_metadata_mismatch() {
        let store = SqliteMetadataStore::in_memory().unwrap();
        let keypair = AgentKeypair::from_seed([5_u8; 32]);
        let prepared = ObjectBody::prepare(
            ObjectKind::Fact,
            keypair.agent_id(),
            1_700_000_000_000,
            Vec::new(),
            Vec::new(),
            "text/plain",
            vec![1_u8; hivemind_core::INLINE_OBJECT_THRESHOLD + 1],
        )
        .unwrap();
        let expected_chunk = match &prepared.body.payload {
            Payload::Chunked(payload) => payload.chunks[0].clone(),
            Payload::Inline(_) => panic!("expected chunked payload"),
        };
        let envelope = keypair.sign_object(prepared.body).unwrap();
        let wrong_chunks = vec![StoredChunk {
            chunk_id: ChunkId::from_chunk_bytes(b"wrong"),
            position: expected_chunk.index,
            size: expected_chunk.size,
            path: "chunks/wrong".to_owned(),
        }];

        let err = store
            .record_object(&envelope, "object", &wrong_chunks, 1)
            .unwrap_err();

        assert!(matches!(err, SqliteStoreError::InvalidObjectMetadata));
    }

    #[test]
    fn records_repeated_chunk_at_different_positions() {
        let store = SqliteMetadataStore::in_memory().unwrap();
        let keypair = AgentKeypair::from_seed([6_u8; 32]);
        let repeated_chunk = vec![8_u8; hivemind_core::DEFAULT_CHUNK_SIZE];
        let payload = [repeated_chunk.as_slice(), repeated_chunk.as_slice()].concat();
        let prepared = ObjectBody::prepare(
            ObjectKind::Fact,
            keypair.agent_id(),
            1_700_000_000_000,
            Vec::new(),
            Vec::new(),
            "application/octet-stream",
            payload,
        )
        .unwrap();
        let chunks = match &prepared.body.payload {
            Payload::Chunked(payload) => payload
                .chunks
                .iter()
                .map(|chunk| StoredChunk {
                    chunk_id: chunk.chunk_id,
                    position: chunk.index,
                    size: chunk.size,
                    path: chunk.chunk_id.to_string(),
                })
                .collect::<Vec<_>>(),
            Payload::Inline(_) => panic!("expected chunked payload"),
        };
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].chunk_id, chunks[1].chunk_id);
        let envelope = keypair.sign_object(prepared.body).unwrap();

        store
            .record_object(&envelope, "object", &chunks, 1)
            .unwrap();

        assert_eq!(store.chunks_for_object(envelope.object_id).unwrap(), chunks);
    }
}
