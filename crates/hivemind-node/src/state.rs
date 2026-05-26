use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::{fs::OpenOptions, path::Path, sync::Mutex};

use crate::api::{InviteRecord, PeerRecord, PeerSummary};

pub const CLIENT_TOKEN_SCOPE_MEMORY: &str = "memory";
pub const CLIENT_TOKEN_SCOPE_MEMORY_READ: &str = "memory:read";
pub const CLIENT_TOKEN_SCOPE_MEMORY_WRITE: &str = "memory:write";
pub const CLIENT_TOKEN_SCOPE_MEMORY_IMPORT: &str = "memory:import";
pub const DEFAULT_CLIENT_TOKEN_SCOPES: &str = "memory:read memory:write memory:import";
pub const DEFAULT_CLIENT_TOKEN_TTL_MS: u64 = 30 * 24 * 60 * 60 * 1000;

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
pub enum ClientTokenStatus {
    Valid,
    NotFound,
    Expired,
    Revoked,
    WrongScope,
}

#[derive(Debug, Eq, PartialEq)]
pub enum ConsumedInvite {
    Active { node_url: String },
    Expired,
    NotFound,
}

#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq)]
pub struct AuditEvent {
    pub id: i64,
    pub created_at_ms: u64,
    pub event_type: String,
    pub subject: Option<String>,
    pub detail: String,
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
        add_column_if_missing(&connection, "client_tokens", "expires_at_ms", "INTEGER")?;
        add_column_if_missing(&connection, "client_tokens", "revoked_at_ms", "INTEGER")?;
        add_column_if_missing(
            &connection,
            "client_tokens",
            "scope",
            "TEXT NOT NULL DEFAULT 'memory'",
        )?;
        connection.execute(
            "UPDATE client_tokens
             SET expires_at_ms = created_at_ms + ?1
             WHERE expires_at_ms IS NULL",
            params![DEFAULT_CLIENT_TOKEN_TTL_MS as i64],
        )?;
        Ok(())
    }

    pub fn client_token_status(
        &self,
        token: &str,
        now_ms: u64,
        required_scope: &str,
    ) -> NodeStateStoreResult<ClientTokenStatus> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| NodeStateStoreError::LockPoisoned)?;
        let Some((expires_at_ms, revoked_at_ms, scope)) = connection
            .query_row(
                "SELECT expires_at_ms, revoked_at_ms, scope
                 FROM client_tokens
                 WHERE token = ?1",
                params![token],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)? as u64,
                        row.get::<_, Option<i64>>(1)?.map(|value| value as u64),
                        row.get::<_, String>(2)?,
                    ))
                },
            )
            .optional()?
        else {
            return Ok(ClientTokenStatus::NotFound);
        };

        if revoked_at_ms.is_some() {
            return Ok(ClientTokenStatus::Revoked);
        }
        if expires_at_ms <= now_ms {
            return Ok(ClientTokenStatus::Expired);
        }
        if !scope_allows(&scope, required_scope) {
            return Ok(ClientTokenStatus::WrongScope);
        }
        Ok(ClientTokenStatus::Valid)
    }

    pub fn has_client_token(
        &self,
        token: &str,
        now_ms: u64,
        required_scope: &str,
    ) -> NodeStateStoreResult<bool> {
        Ok(self.client_token_status(token, now_ms, required_scope)? == ClientTokenStatus::Valid)
    }

    pub fn insert_client_token(
        &self,
        token: &str,
        created_at_ms: u64,
        expires_at_ms: u64,
        scope: &str,
    ) -> NodeStateStoreResult<()> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| NodeStateStoreError::LockPoisoned)?;
        connection.execute(
            "INSERT OR IGNORE INTO client_tokens (
                token, created_at_ms, expires_at_ms, revoked_at_ms, scope
             ) VALUES (?1, ?2, ?3, NULL, ?4)",
            params![token, created_at_ms as i64, expires_at_ms as i64, scope],
        )?;
        Ok(())
    }

    pub fn revoke_client_token(
        &self,
        token: &str,
        revoked_at_ms: u64,
    ) -> NodeStateStoreResult<bool> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| NodeStateStoreError::LockPoisoned)?;
        let updated = connection.execute(
            "UPDATE client_tokens
             SET revoked_at_ms = COALESCE(revoked_at_ms, ?2)
             WHERE token = ?1",
            params![token, revoked_at_ms as i64],
        )?;
        Ok(updated > 0)
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
        token_expires_at_ms: u64,
        scope: &str,
    ) -> NodeStateStoreResult<ConsumedInvite> {
        let mut connection = self
            .connection
            .lock()
            .map_err(|_| NodeStateStoreError::LockPoisoned)?;
        let tx = connection.transaction()?;
        let consumed = consume_invite_in_tx(&tx, invite_code, now_ms)?;
        if matches!(consumed, ConsumedInvite::Active { .. }) {
            tx.execute(
                "INSERT OR IGNORE INTO client_tokens (
                    token, created_at_ms, expires_at_ms, revoked_at_ms, scope
                 ) VALUES (?1, ?2, ?3, NULL, ?4)",
                params![token, now_ms as i64, token_expires_at_ms as i64, scope,],
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

    pub fn record_audit_event(
        &self,
        created_at_ms: u64,
        event_type: &str,
        subject: Option<&str>,
        detail: &str,
    ) -> NodeStateStoreResult<()> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| NodeStateStoreError::LockPoisoned)?;
        connection.execute(
            "INSERT INTO audit_events (created_at_ms, event_type, subject, detail)
             VALUES (?1, ?2, ?3, ?4)",
            params![created_at_ms as i64, event_type, subject, detail],
        )?;
        Ok(())
    }

    pub fn audit_events(&self, limit: u32) -> NodeStateStoreResult<Vec<AuditEvent>> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| NodeStateStoreError::LockPoisoned)?;
        let mut statement = connection.prepare(
            "SELECT id, created_at_ms, event_type, subject, detail
             FROM audit_events
             ORDER BY id DESC
             LIMIT ?1",
        )?;
        let events = statement
            .query_map(params![limit.clamp(1, 500) as i64], |row| {
                Ok(AuditEvent {
                    id: row.get(0)?,
                    created_at_ms: row.get::<_, i64>(1)? as u64,
                    event_type: row.get(2)?,
                    subject: row.get(3)?,
                    detail: row.get(4)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(events)
    }

    pub fn trusted_peer_by_node_id(
        &self,
        node_id: &str,
    ) -> NodeStateStoreResult<Option<PeerRecord>> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| NodeStateStoreError::LockPoisoned)?;
        let peer = connection
            .query_row(
                "SELECT node_url, node_id, trusted
                 FROM peers
                 WHERE node_id = ?1 AND trusted = 1",
                params![node_id],
                |row| {
                    Ok(PeerRecord {
                        node_url: row.get(0)?,
                        node_id: row.get(1)?,
                        trusted: row.get(2)?,
                    })
                },
            )
            .optional()?;
        Ok(peer)
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

fn scope_allows(granted_scope: &str, required_scope: &str) -> bool {
    granted_scope
        .split(|ch: char| ch == ',' || ch.is_whitespace())
        .any(|scope| scope == required_scope || scope == CLIENT_TOKEN_SCOPE_MEMORY)
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

fn add_column_if_missing(
    connection: &Connection,
    table: &str,
    column: &str,
    definition: &str,
) -> NodeStateStoreResult<()> {
    let mut statement = connection.prepare(&format!("PRAGMA table_info({table})"))?;
    let exists = statement
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<Result<Vec<_>, _>>()?
        .iter()
        .any(|name| name == column);
    if !exists {
        connection.execute_batch(&format!(
            "ALTER TABLE {table} ADD COLUMN {column} {definition};"
        ))?;
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
    created_at_ms INTEGER NOT NULL DEFAULT 0,
    expires_at_ms INTEGER,
    revoked_at_ms INTEGER,
    scope TEXT NOT NULL DEFAULT 'memory'
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

CREATE TABLE IF NOT EXISTS audit_events (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    created_at_ms INTEGER NOT NULL,
    event_type TEXT NOT NULL,
    subject TEXT,
    detail TEXT NOT NULL
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
    fn client_tokens_persist_with_expiry_scope_and_revocation() {
        let tempdir = tempfile::tempdir().unwrap();
        let path = tempdir.path().join("state.sqlite3");
        SqliteNodeStateStore::open(&path)
            .unwrap()
            .insert_client_token("client-token", 1000, 2000, DEFAULT_CLIENT_TOKEN_SCOPES)
            .unwrap();

        let store = SqliteNodeStateStore::open(&path).unwrap();

        assert_eq!(
            store
                .client_token_status("client-token", 1500, CLIENT_TOKEN_SCOPE_MEMORY_READ)
                .unwrap(),
            ClientTokenStatus::Valid
        );
        assert_eq!(
            store
                .client_token_status("client-token", 2000, CLIENT_TOKEN_SCOPE_MEMORY_READ)
                .unwrap(),
            ClientTokenStatus::Expired
        );
        assert_eq!(
            store
                .client_token_status("client-token", 1500, "admin")
                .unwrap(),
            ClientTokenStatus::WrongScope
        );
        assert_eq!(
            store
                .client_token_status("other-token", 1500, CLIENT_TOKEN_SCOPE_MEMORY_READ)
                .unwrap(),
            ClientTokenStatus::NotFound
        );

        assert!(store.revoke_client_token("client-token", 1600).unwrap());
        assert_eq!(
            store
                .client_token_status("client-token", 1700, CLIENT_TOKEN_SCOPE_MEMORY_READ)
                .unwrap(),
            ClientTokenStatus::Revoked
        );
    }

    #[test]
    fn legacy_client_token_rows_get_default_expiry_and_scope() {
        let tempdir = tempfile::tempdir().unwrap();
        let path = tempdir.path().join("state.sqlite3");
        let connection = Connection::open(&path).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE client_tokens (
                    token TEXT PRIMARY KEY NOT NULL,
                    created_at_ms INTEGER NOT NULL DEFAULT 0
                );",
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO client_tokens (token, created_at_ms) VALUES (?1, ?2)",
                params!["legacy-token", 1000_i64],
            )
            .unwrap();
        drop(connection);

        let store = SqliteNodeStateStore::open(&path).unwrap();

        assert_eq!(
            store
                .client_token_status("legacy-token", 1001, CLIENT_TOKEN_SCOPE_MEMORY)
                .unwrap(),
            ClientTokenStatus::Valid
        );
        assert_eq!(
            store
                .client_token_status(
                    "legacy-token",
                    1000 + DEFAULT_CLIENT_TOKEN_TTL_MS,
                    CLIENT_TOKEN_SCOPE_MEMORY,
                )
                .unwrap(),
            ClientTokenStatus::Expired
        );
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
    fn trusted_peer_lookup_requires_trust() {
        let store = SqliteNodeStateStore::in_memory().unwrap();
        let node_id = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        store
            .upsert_peer(&PeerRecord {
                node_url: "https://node-a.internal".to_owned(),
                node_id: node_id.to_owned(),
                trusted: false,
            })
            .unwrap();
        assert_eq!(store.trusted_peer_by_node_id(node_id).unwrap(), None);

        store
            .upsert_peer(&PeerRecord {
                node_url: "https://node-a.internal".to_owned(),
                node_id: node_id.to_owned(),
                trusted: true,
            })
            .unwrap();
        assert_eq!(
            store.trusted_peer_by_node_id(node_id).unwrap(),
            Some(PeerRecord {
                node_url: "https://node-a.internal".to_owned(),
                node_id: node_id.to_owned(),
                trusted: true,
            })
        );
    }

    #[test]
    fn audit_events_persist_newest_first() {
        let tempdir = tempfile::tempdir().unwrap();
        let path = tempdir.path().join("state.sqlite3");
        let store = SqliteNodeStateStore::open(&path).unwrap();
        store
            .record_audit_event(1000, "invite_created", Some("invite:abcd"), "node_url=x")
            .unwrap();
        store
            .record_audit_event(2000, "join_exchanged", Some("invite:abcd"), "scope=memory")
            .unwrap();

        let store = SqliteNodeStateStore::open(&path).unwrap();
        let events = store.audit_events(10).unwrap();

        assert_eq!(events.len(), 2);
        assert_eq!(events[0].event_type, "join_exchanged");
        assert_eq!(events[0].created_at_ms, 2000);
        assert_eq!(events[0].subject, Some("invite:abcd".to_owned()));
        assert_eq!(events[1].event_type, "invite_created");
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
