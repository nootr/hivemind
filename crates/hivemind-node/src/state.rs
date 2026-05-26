use rusqlite::{params, Connection, OptionalExtension};
use std::{fs::OpenOptions, path::Path, sync::Mutex};

use crate::api::{InviteRecord, PeerRecord, PeerSummary};

#[derive(Debug, thiserror::Error)]
pub enum NodeStateStoreError {
    #[error("sqlite state error: {0}")]
    Sqlite(#[from] rusqlite::Error),

    #[error("state file io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("node state store lock poisoned")]
    LockPoisoned,

    #[error("invalid stored node state")]
    InvalidStoredState,
}

pub type NodeStateStoreResult<T> = Result<T, NodeStateStoreError>;

#[derive(Debug)]
pub struct SqliteNodeStateStore {
    connection: Mutex<Connection>,
}

#[derive(Debug, Eq, PartialEq)]
pub enum ConsumedInvite {
    Active { node_url: String },
    Expired,
    NotFound,
}

impl SqliteNodeStateStore {
    pub fn open(path: impl AsRef<Path>) -> NodeStateStoreResult<Self> {
        prepare_state_file(path.as_ref())?;
        let connection = Connection::open(path)?;
        connection.execute_batch("PRAGMA foreign_keys = ON;")?;
        let store = Self {
            connection: Mutex::new(connection),
        };
        store.migrate()?;
        Ok(store)
    }

    pub fn in_memory() -> NodeStateStoreResult<Self> {
        let connection = Connection::open_in_memory()?;
        connection.execute_batch("PRAGMA foreign_keys = ON;")?;
        let store = Self {
            connection: Mutex::new(connection),
        };
        store.migrate()?;
        Ok(store)
    }

    pub fn migrate(&self) -> NodeStateStoreResult<()> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| NodeStateStoreError::LockPoisoned)?;
        connection.execute_batch(MIGRATIONS)?;
        Ok(())
    }

    pub fn has_client_token(&self, token: &str) -> NodeStateStoreResult<bool> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| NodeStateStoreError::LockPoisoned)?;
        let exists = connection
            .query_row(
                "SELECT 1 FROM client_tokens WHERE token = ?1",
                params![token],
                |_| Ok(()),
            )
            .optional()?
            .is_some();
        Ok(exists)
    }

    pub fn insert_client_token(&self, token: &str) -> NodeStateStoreResult<()> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| NodeStateStoreError::LockPoisoned)?;
        connection.execute(
            "INSERT OR IGNORE INTO client_tokens (token, created_at_ms) VALUES (?1, 0)",
            params![token],
        )?;
        Ok(())
    }

    pub fn insert_invite(
        &self,
        invite_code: &str,
        record: &InviteRecord,
    ) -> NodeStateStoreResult<()> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| NodeStateStoreError::LockPoisoned)?;
        connection.execute(
            "INSERT INTO invites (invite_code, node_url, expires_at_ms, uses_remaining)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(invite_code) DO UPDATE SET
                node_url = excluded.node_url,
                expires_at_ms = excluded.expires_at_ms,
                uses_remaining = excluded.uses_remaining",
            params![
                invite_code,
                record.node_url,
                record.expires_at_ms as i64,
                record.uses_remaining as i64,
            ],
        )?;
        Ok(())
    }

    pub fn consume_invite(
        &self,
        invite_code: &str,
        now_ms: u64,
    ) -> NodeStateStoreResult<ConsumedInvite> {
        let mut connection = self
            .connection
            .lock()
            .map_err(|_| NodeStateStoreError::LockPoisoned)?;
        let tx = connection.transaction()?;
        let consumed = consume_invite_in_tx(&tx, invite_code, now_ms)?;
        tx.commit()?;
        Ok(consumed)
    }

    pub fn exchange_invite_for_client_token(
        &self,
        invite_code: &str,
        now_ms: u64,
        token: &str,
    ) -> NodeStateStoreResult<ConsumedInvite> {
        let mut connection = self
            .connection
            .lock()
            .map_err(|_| NodeStateStoreError::LockPoisoned)?;
        let tx = connection.transaction()?;
        let consumed = consume_invite_in_tx(&tx, invite_code, now_ms)?;
        if matches!(consumed, ConsumedInvite::Active { .. }) {
            tx.execute(
                "INSERT OR IGNORE INTO client_tokens (token, created_at_ms) VALUES (?1, ?2)",
                params![token, now_ms as i64],
            )?;
        }
        tx.commit()?;
        Ok(consumed)
    }

    pub fn upsert_peer(&self, peer: &PeerRecord) -> NodeStateStoreResult<()> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| NodeStateStoreError::LockPoisoned)?;
        connection.execute(
            "INSERT INTO peers (node_id, node_url, trusted)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(node_id) DO UPDATE SET
                node_url = excluded.node_url,
                trusted = excluded.trusted",
            params![peer.node_id, peer.node_url, peer.trusted],
        )?;
        Ok(())
    }

    pub fn peer_summaries(&self, include_trust: bool) -> NodeStateStoreResult<Vec<PeerSummary>> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| NodeStateStoreError::LockPoisoned)?;
        let mut statement = connection.prepare(
            "SELECT node_url, node_id, trusted
             FROM peers
             ORDER BY node_url ASC",
        )?;
        let peers = statement
            .query_map([], |row| {
                let trusted = row.get::<_, bool>(2)?;
                Ok(PeerSummary {
                    node_url: row.get(0)?,
                    node_id: row.get(1)?,
                    trusted: include_trust && trusted,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(peers)
    }
}

fn prepare_state_file(path: &Path) -> std::io::Result<()> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        std::fs::create_dir_all(parent)?;
    }

    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true);

    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }

    options.open(path)?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }

    Ok(())
}

fn consume_invite_in_tx(
    tx: &rusqlite::Transaction<'_>,
    invite_code: &str,
    now_ms: u64,
) -> NodeStateStoreResult<ConsumedInvite> {
    let Some(record) = tx
        .query_row(
            "SELECT node_url, expires_at_ms, uses_remaining
             FROM invites
             WHERE invite_code = ?1",
            params![invite_code],
            |row| {
                Ok(InviteRecord {
                    node_url: row.get(0)?,
                    expires_at_ms: row.get::<_, i64>(1)? as u64,
                    uses_remaining: row.get::<_, i64>(2)? as u32,
                })
            },
        )
        .optional()?
    else {
        return Ok(ConsumedInvite::NotFound);
    };

    if record.expires_at_ms <= now_ms {
        tx.execute(
            "DELETE FROM invites WHERE invite_code = ?1",
            params![invite_code],
        )?;
        return Ok(ConsumedInvite::Expired);
    }

    if record.uses_remaining <= 1 {
        tx.execute(
            "DELETE FROM invites WHERE invite_code = ?1",
            params![invite_code],
        )?;
    } else {
        tx.execute(
            "UPDATE invites SET uses_remaining = uses_remaining - 1 WHERE invite_code = ?1",
            params![invite_code],
        )?;
    }

    Ok(ConsumedInvite::Active {
        node_url: record.node_url,
    })
}

const MIGRATIONS: &str = r#"
CREATE TABLE IF NOT EXISTS client_tokens (
    token TEXT PRIMARY KEY NOT NULL,
    created_at_ms INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS invites (
    invite_code TEXT PRIMARY KEY NOT NULL,
    node_url TEXT NOT NULL,
    expires_at_ms INTEGER NOT NULL,
    uses_remaining INTEGER NOT NULL CHECK (uses_remaining >= 0)
);

CREATE TABLE IF NOT EXISTS peers (
    node_id TEXT PRIMARY KEY NOT NULL,
    node_url TEXT NOT NULL,
    trusted INTEGER NOT NULL CHECK (trusted IN (0, 1))
);
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn state_file_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;

        let tempdir = tempfile::tempdir().unwrap();
        let path = tempdir.path().join("state.sqlite3");
        SqliteNodeStateStore::open(&path).unwrap();

        let mode = std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }

    #[test]
    fn client_tokens_persist() {
        let tempdir = tempfile::tempdir().unwrap();
        let path = tempdir.path().join("state.sqlite3");
        SqliteNodeStateStore::open(&path)
            .unwrap()
            .insert_client_token("client-token")
            .unwrap();

        let store = SqliteNodeStateStore::open(&path).unwrap();

        assert!(store.has_client_token("client-token").unwrap());
        assert!(!store.has_client_token("other-token").unwrap());
    }

    #[test]
    fn invites_persist_and_are_consumed_once() {
        let tempdir = tempfile::tempdir().unwrap();
        let path = tempdir.path().join("state.sqlite3");
        SqliteNodeStateStore::open(&path)
            .unwrap()
            .insert_invite(
                "INVITE",
                &InviteRecord {
                    node_url: "https://hive.example.internal".to_owned(),
                    expires_at_ms: 2000,
                    uses_remaining: 1,
                },
            )
            .unwrap();

        let store = SqliteNodeStateStore::open(&path).unwrap();
        assert_eq!(
            store.consume_invite("INVITE", 1000).unwrap(),
            ConsumedInvite::Active {
                node_url: "https://hive.example.internal".to_owned()
            }
        );

        let store = SqliteNodeStateStore::open(&path).unwrap();
        assert_eq!(
            store.consume_invite("INVITE", 1000).unwrap(),
            ConsumedInvite::NotFound
        );
    }

    #[test]
    fn peers_persist_by_node_id() {
        let tempdir = tempfile::tempdir().unwrap();
        let path = tempdir.path().join("state.sqlite3");
        SqliteNodeStateStore::open(&path)
            .unwrap()
            .upsert_peer(&PeerRecord {
                node_url: "https://node-a.internal".to_owned(),
                node_id: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                    .to_owned(),
                trusted: true,
            })
            .unwrap();

        let store = SqliteNodeStateStore::open(&path).unwrap();
        assert_eq!(
            store.peer_summaries(true).unwrap(),
            vec![PeerSummary {
                node_url: "https://node-a.internal".to_owned(),
                node_id: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                    .to_owned(),
                trusted: true,
            }]
        );
    }
}
