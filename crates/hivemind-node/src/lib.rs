mod config;
mod net;
mod secret_file;

pub use config::NodeConfig;

use axum::{
    extract::{ConnectInfo, Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use hivemind_core::{
    inbound_decision, valid_node_id, ChatMessage, InboundDecision, NodeKey, NodeProof, PeerInfo,
    PeerRecord, PeerTrustState,
};
use net::{inferred_node_url, local_node_url, normalized_node_url, valid_node_url};
use rusqlite::{params, Connection, OptionalExtension};
use secret_file::{
    create_private_state_file_if_missing, load_or_create_key, restrict_state_permissions,
};
use serde::{Deserialize, Serialize};
use std::{
    env, fs,
    net::{Ipv4Addr, SocketAddr},
    path::Path as FsPath,
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::net::UdpSocket;

const DISCOVERY_PORT: u16 = 7748;
const DISCOVERY_PREFIX: &str = "HIVEMIND_NODE_V2 ";
const BEACON_FAST_SECS: u64 = 2;
const BEACON_SLOW_SECS: u64 = 20;
const PEER_FETCH_TIMEOUT_SECS: u64 = 2;
const MAX_CHAT_TEXT_BYTES: usize = 64 * 1024;
const MAX_ROOM_BYTES: usize = 64;
const MAX_NONCE_BYTES: usize = 128;
const MAX_PEERS: i64 = 1024;

#[derive(Debug, thiserror::Error)]
pub enum NodeError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("toml error: {0}")]
    Toml(#[from] toml::de::Error),
    #[error("core error: {0}")]
    Core(#[from] hivemind_core::CoreError),
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
}

#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error("not found")]
    NotFound,
    #[error("invalid request")]
    InvalidRequest,
    #[error("forbidden")]
    Forbidden,
    #[error("internal error: {0}")]
    Internal(String),
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = match self {
            ApiError::NotFound => StatusCode::NOT_FOUND,
            ApiError::InvalidRequest => StatusCode::BAD_REQUEST,
            ApiError::Forbidden => StatusCode::FORBIDDEN,
            ApiError::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        };
        (status, self.to_string()).into_response()
    }
}

#[derive(Clone)]
pub struct AppState {
    key: Arc<NodeKey>,
    bind_addr: SocketAddr,
    public_url: Option<String>,
    store: Arc<Store>,
}

struct Store {
    conn: Mutex<Connection>,
}

impl Store {
    fn open(path: &FsPath) -> Result<Self, NodeError> {
        create_private_state_file_if_missing(path)?;
        let conn = Connection::open(path)?;
        migrate_store(&conn)?;
        restrict_state_permissions(path)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    #[cfg(test)]
    fn memory() -> Self {
        let conn = Connection::open_in_memory().expect("in-memory sqlite");
        migrate_store(&conn).expect("migrate in-memory sqlite");
        Self {
            conn: Mutex::new(conn),
        }
    }
}

fn migrate_store(conn: &Connection) -> rusqlite::Result<()> {
    let version: i64 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    if version > 2 {
        return Err(rusqlite::Error::InvalidQuery);
    }
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS peers (
            node_id TEXT PRIMARY KEY NOT NULL,
            node_url TEXT NOT NULL,
            name TEXT,
            last_seen_ms INTEGER NOT NULL,
            trust_state TEXT NOT NULL DEFAULT 'unknown',
            source TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS messages (
            id TEXT PRIMARY KEY NOT NULL,
            room TEXT NOT NULL,
            author_node_id TEXT NOT NULL,
            created_at_ms INTEGER NOT NULL,
            text TEXT NOT NULL,
            signature TEXT NOT NULL
        );
        CREATE INDEX IF NOT EXISTS messages_room_created_idx
            ON messages(room, created_at_ms);
        CREATE TABLE IF NOT EXISTS quarantine_messages (
            id TEXT PRIMARY KEY NOT NULL,
            room TEXT NOT NULL,
            author_node_id TEXT NOT NULL,
            created_at_ms INTEGER NOT NULL,
            text TEXT NOT NULL,
            signature TEXT NOT NULL
        );
        CREATE INDEX IF NOT EXISTS quarantine_author_created_idx
            ON quarantine_messages(author_node_id, created_at_ms);
        CREATE TABLE IF NOT EXISTS unknown_notices (
            node_id TEXT PRIMARY KEY NOT NULL
        );
        "#,
    )?;
    if column_exists(conn, "peers", "trusted")? {
        rebuild_peers_without_legacy_trusted(conn)?;
    } else if version < 2 && !column_exists(conn, "peers", "trust_state")? {
        conn.execute_batch(
            "ALTER TABLE peers ADD COLUMN trust_state TEXT NOT NULL DEFAULT 'unknown';",
        )?;
    }
    conn.execute_batch("PRAGMA user_version = 2;")
}

fn rebuild_peers_without_legacy_trusted(conn: &Connection) -> rusqlite::Result<()> {
    let has_trust_state = column_exists(conn, "peers", "trust_state")?;
    conn.execute_batch(
        r#"
        ALTER TABLE peers RENAME TO peers_legacy_trusted;
        CREATE TABLE peers (
            node_id TEXT PRIMARY KEY NOT NULL,
            node_url TEXT NOT NULL,
            name TEXT,
            last_seen_ms INTEGER NOT NULL,
            trust_state TEXT NOT NULL DEFAULT 'unknown',
            source TEXT NOT NULL
        );
        "#,
    )?;
    if has_trust_state {
        conn.execute_batch(
            r#"
            INSERT INTO peers (node_id, node_url, name, last_seen_ms, trust_state, source)
            SELECT node_id, node_url, name, last_seen_ms,
                   CASE trust_state
                       WHEN 'trusted' THEN 'trusted'
                       WHEN 'blocked' THEN 'blocked'
                       ELSE CASE WHEN trusted != 0 THEN 'trusted' ELSE 'unknown' END
                   END,
                   source
            FROM peers_legacy_trusted;
            "#,
        )?;
    } else {
        conn.execute_batch(
            r#"
            INSERT INTO peers (node_id, node_url, name, last_seen_ms, trust_state, source)
            SELECT node_id, node_url, name, last_seen_ms,
                   CASE WHEN trusted != 0 THEN 'trusted' ELSE 'unknown' END,
                   source
            FROM peers_legacy_trusted;
            "#,
        )?;
    }
    conn.execute_batch("DROP TABLE peers_legacy_trusted;")
}

fn column_exists(conn: &Connection, table: &str, column: &str) -> rusqlite::Result<bool> {
    let mut statement = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let columns = statement
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(columns.iter().any(|name| name == column))
}

fn map_store_error(err: rusqlite::Error) -> ApiError {
    ApiError::Internal(format!("sqlite: {err}"))
}

#[derive(Debug, Deserialize)]
pub struct SayRequest {
    pub text: String,
    #[serde(default = "default_room")]
    pub room: String,
}

#[derive(Debug, Deserialize)]
pub struct MessagesQuery {
    #[serde(default = "default_room")]
    pub room: String,
    #[serde(default)]
    pub after_ms: Option<u64>,
}

#[derive(Debug, Deserialize)]
pub struct NodeProofQuery {
    pub nonce: String,
}

#[derive(Debug, Serialize, Deserialize, Eq, PartialEq)]
pub struct NodeInfoResponse {
    pub node_url: String,
    pub node_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Eq, PartialEq)]
pub struct PeersResponse {
    pub peers: Vec<PeerRecord>,
}

#[derive(Debug, Serialize, Deserialize, Eq, PartialEq)]
pub struct MessagesResponse {
    pub messages: Vec<ChatMessage>,
}

fn default_room() -> String {
    "default".to_owned()
}

pub async fn run(config: NodeConfig) -> Result<(), NodeError> {
    fs::create_dir_all(&config.data_dir)?;
    let key = load_or_create_key(&config.data_dir.join("node.key"))?;
    let store = Arc::new(Store::open(&config.data_dir.join("state.sqlite3"))?);
    let state = AppState {
        key: Arc::new(key),
        bind_addr: config.bind_addr,
        public_url: config.public_url.clone(),
        store,
    };

    spawn_discovery(config.bind_addr, config.public_url, state.clone());
    serve(config.bind_addr, app(state)).await?;
    Ok(())
}

impl AppState {
    fn node_url(&self) -> String {
        self.public_url
            .clone()
            .unwrap_or_else(|| local_node_url(self.bind_addr))
    }
}

pub fn app(state: AppState) -> Router {
    Router::new()
        .route("/health", get(|| async { "ok" }))
        .route("/v1/node", get(get_node))
        .route("/v1/node/proof", get(get_node_proof))
        .route("/v1/peers", get(get_peers).post(add_peer))
        .route("/v1/join", post(join_network))
        .route("/v1/peers/{node_id}/trust", post(trust_peer))
        .route("/v1/peers/{node_id}/deny", post(deny_peer))
        .route("/v1/chat", get(get_messages).post(say))
        .route("/v1/chat/import", post(import_message))
        .with_state(state)
}

async fn get_node(State(state): State<AppState>) -> Json<NodeInfoResponse> {
    Json(NodeInfoResponse {
        node_url: state.node_url(),
        node_id: state.key.node_id(),
        name: local_node_name(),
    })
}

async fn get_node_proof(
    State(state): State<AppState>,
    Query(query): Query<NodeProofQuery>,
) -> Result<Json<NodeProof>, ApiError> {
    if !valid_nonce(&query.nonce) {
        return Err(ApiError::InvalidRequest);
    }
    Ok(Json(state.key.sign_node_proof(
        &state.node_url(),
        local_node_name(),
        &query.nonce,
    )))
}

async fn get_peers(
    ConnectInfo(client_addr): ConnectInfo<SocketAddr>,
    State(state): State<AppState>,
) -> Result<Json<PeersResponse>, ApiError> {
    let mut peers = list_peers(&state)?;
    if !client_addr.ip().is_loopback() {
        for peer in &mut peers {
            peer.trust_state = PeerTrustState::Unknown;
        }
    }
    Ok(Json(PeersResponse { peers }))
}

async fn add_peer(
    ConnectInfo(client_addr): ConnectInfo<SocketAddr>,
    State(state): State<AppState>,
    Json(peer): Json<PeerInfo>,
) -> Result<Json<PeerRecord>, ApiError> {
    require_local_client(client_addr)?;
    if !valid_peer_info(&peer) || peer.node_id == state.key.node_id() {
        return Err(ApiError::InvalidRequest);
    }
    let node_id = peer.node_id.clone();
    remember_peer(&state, peer, "manual")?;
    Ok(Json(get_peer(&state, &node_id)?.ok_or(ApiError::NotFound)?))
}

async fn join_network(
    State(state): State<AppState>,
    Json(peer): Json<PeerInfo>,
) -> Result<Json<PeersResponse>, ApiError> {
    if !valid_peer_info(&peer) || peer.node_id == state.key.node_id() {
        return Err(ApiError::InvalidRequest);
    }
    remember_peer(&state, peer, "join")?;
    let mut peers = list_peers(&state)?;
    peers.push(PeerRecord {
        node_url: state.node_url(),
        node_id: state.key.node_id(),
        name: local_node_name(),
        last_seen_ms: now_ms(),
        trust_state: PeerTrustState::Unknown,
        source: "self".to_owned(),
    });
    Ok(Json(PeersResponse { peers }))
}

async fn trust_peer(
    ConnectInfo(client_addr): ConnectInfo<SocketAddr>,
    State(state): State<AppState>,
    Path(node_id): Path<String>,
) -> Result<Json<PeerRecord>, ApiError> {
    require_local_client(client_addr)?;
    if !valid_node_id(&node_id) {
        return Err(ApiError::InvalidRequest);
    }
    Ok(Json(trust_peer_record(&state, &node_id)?))
}

async fn deny_peer(
    ConnectInfo(client_addr): ConnectInfo<SocketAddr>,
    State(state): State<AppState>,
    Path(node_id): Path<String>,
) -> Result<Json<PeerRecord>, ApiError> {
    require_local_client(client_addr)?;
    if !valid_node_id(&node_id) {
        return Err(ApiError::InvalidRequest);
    }
    Ok(Json(block_peer_record(&state, &node_id)?))
}

async fn say(
    ConnectInfo(client_addr): ConnectInfo<SocketAddr>,
    State(state): State<AppState>,
    Json(request): Json<SayRequest>,
) -> Result<Json<ChatMessage>, ApiError> {
    require_local_client(client_addr)?;
    if !valid_room(&request.room) || !valid_chat_text(&request.text) {
        return Err(ApiError::InvalidRequest);
    }
    let message = state.key.sign_chat(&request.room, now_ms(), &request.text);
    store_message(&state, message.clone())?;
    gossip_message(state, message.clone());
    Ok(Json(message))
}

async fn import_message(
    State(state): State<AppState>,
    Json(message): Json<ChatMessage>,
) -> Result<Json<ChatMessage>, ApiError> {
    if !valid_message_payload(&message) {
        return Err(ApiError::InvalidRequest);
    }
    message.verify().map_err(|_| ApiError::InvalidRequest)?;
    if message.author_node_id != state.key.node_id() {
        match inbound_decision(peer_trust_state(&state, &message.author_node_id)) {
            InboundDecision::Accept => {}
            InboundDecision::Quarantine => {
                quarantine_unknown_message(&state, message.clone())?;
                return Err(ApiError::Forbidden);
            }
            InboundDecision::Drop => return Err(ApiError::Forbidden),
        }
    }
    store_message(&state, message.clone())?;
    Ok(Json(message))
}

async fn get_messages(
    ConnectInfo(client_addr): ConnectInfo<SocketAddr>,
    State(state): State<AppState>,
    Query(query): Query<MessagesQuery>,
) -> Result<Json<MessagesResponse>, ApiError> {
    require_local_client(client_addr)?;
    if !valid_room(&query.room) {
        return Err(ApiError::InvalidRequest);
    }
    let after_ms = query.after_ms.unwrap_or(0);
    let messages = list_messages(&state, &query.room, after_ms)?;
    Ok(Json(MessagesResponse { messages }))
}

fn require_local_client(client_addr: SocketAddr) -> Result<(), ApiError> {
    if client_addr.ip().is_loopback() {
        Ok(())
    } else {
        Err(ApiError::Forbidden)
    }
}

fn row_to_peer(row: &rusqlite::Row<'_>) -> rusqlite::Result<PeerRecord> {
    Ok(PeerRecord {
        node_id: row.get(0)?,
        node_url: row.get(1)?,
        name: row.get(2)?,
        last_seen_ms: row.get::<_, i64>(3)? as u64,
        trust_state: trust_state_from_db(&row.get::<_, String>(4)?)?,
        source: row.get(5)?,
    })
}

fn trust_state_from_db(input: &str) -> rusqlite::Result<PeerTrustState> {
    match input {
        "unknown" => Ok(PeerTrustState::Unknown),
        "trusted" => Ok(PeerTrustState::Trusted),
        "blocked" => Ok(PeerTrustState::Blocked),
        _ => Err(rusqlite::Error::InvalidQuery),
    }
}

fn row_to_message(row: &rusqlite::Row<'_>) -> rusqlite::Result<ChatMessage> {
    Ok(ChatMessage {
        id: row.get(0)?,
        room: row.get(1)?,
        author_node_id: row.get(2)?,
        created_at_ms: row.get::<_, i64>(3)? as u64,
        text: row.get(4)?,
        signature: row.get(5)?,
    })
}

fn list_peers(state: &AppState) -> Result<Vec<PeerRecord>, ApiError> {
    let conn = state
        .store
        .conn
        .lock()
        .map_err(|_| ApiError::Internal("sqlite lock".to_owned()))?;
    let mut statement = conn
        .prepare(
            "SELECT node_id, node_url, name, last_seen_ms, trust_state, source
             FROM peers
             ORDER BY CASE trust_state WHEN 'trusted' THEN 0 WHEN 'unknown' THEN 1 ELSE 2 END,
                      name IS NULL, name, node_url, node_id",
        )
        .map_err(map_store_error)?;
    let peers = statement
        .query_map([], row_to_peer)
        .map_err(map_store_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(map_store_error)?;
    Ok(peers)
}

fn get_peer(state: &AppState, node_id: &str) -> Result<Option<PeerRecord>, ApiError> {
    let conn = state
        .store
        .conn
        .lock()
        .map_err(|_| ApiError::Internal("sqlite lock".to_owned()))?;
    conn.query_row(
        "SELECT node_id, node_url, name, last_seen_ms, trust_state, source FROM peers WHERE node_id = ?1",
        params![node_id],
        row_to_peer,
    )
    .optional()
    .map_err(map_store_error)
}

fn trust_peer_record(state: &AppState, node_id: &str) -> Result<PeerRecord, ApiError> {
    let mut conn = state
        .store
        .conn
        .lock()
        .map_err(|_| ApiError::Internal("sqlite lock".to_owned()))?;
    let tx = conn.transaction().map_err(map_store_error)?;
    let changed = tx
        .execute(
            "UPDATE peers SET trust_state = 'trusted' WHERE node_id = ?1",
            params![node_id],
        )
        .map_err(map_store_error)?;
    if changed == 0 {
        return Err(ApiError::NotFound);
    }
    tx.execute(
        "INSERT OR IGNORE INTO messages (id, room, author_node_id, created_at_ms, text, signature)
         SELECT id, room, author_node_id, created_at_ms, text, signature
         FROM quarantine_messages
         WHERE author_node_id = ?1",
        params![node_id],
    )
    .map_err(map_store_error)?;
    tx.execute(
        "DELETE FROM quarantine_messages WHERE author_node_id = ?1",
        params![node_id],
    )
    .map_err(map_store_error)?;
    tx.execute(
        "DELETE FROM unknown_notices WHERE node_id = ?1",
        params![node_id],
    )
    .map_err(map_store_error)?;
    let peer = tx
        .query_row(
            "SELECT node_id, node_url, name, last_seen_ms, trust_state, source FROM peers WHERE node_id = ?1",
            params![node_id],
            row_to_peer,
        )
        .map_err(map_store_error)?;
    tx.commit().map_err(map_store_error)?;
    Ok(peer)
}

fn block_peer_record(state: &AppState, node_id: &str) -> Result<PeerRecord, ApiError> {
    let mut conn = state
        .store
        .conn
        .lock()
        .map_err(|_| ApiError::Internal("sqlite lock".to_owned()))?;
    let tx = conn.transaction().map_err(map_store_error)?;
    tx.execute(
        r#"
        INSERT INTO peers (node_id, node_url, name, last_seen_ms, trust_state, source)
        VALUES (?1, '', NULL, ?2, 'blocked', 'denied')
        ON CONFLICT(node_id) DO UPDATE SET
            trust_state = 'blocked',
            last_seen_ms = excluded.last_seen_ms,
            source = 'denied'
        "#,
        params![node_id, i64::try_from(now_ms()).unwrap_or(i64::MAX)],
    )
    .map_err(map_store_error)?;
    tx.execute(
        "DELETE FROM quarantine_messages WHERE author_node_id = ?1",
        params![node_id],
    )
    .map_err(map_store_error)?;
    tx.execute(
        "DELETE FROM messages WHERE author_node_id = ?1",
        params![node_id],
    )
    .map_err(map_store_error)?;
    tx.execute(
        "DELETE FROM unknown_notices WHERE node_id = ?1",
        params![node_id],
    )
    .map_err(map_store_error)?;
    let peer = tx
        .query_row(
            "SELECT node_id, node_url, name, last_seen_ms, trust_state, source FROM peers WHERE node_id = ?1",
            params![node_id],
            row_to_peer,
        )
        .map_err(map_store_error)?;
    tx.commit().map_err(map_store_error)?;
    Ok(peer)
}

fn list_messages(
    state: &AppState,
    room: &str,
    after_ms: u64,
) -> Result<Vec<ChatMessage>, ApiError> {
    let after_ms = i64::try_from(after_ms).unwrap_or(i64::MAX);
    let conn = state
        .store
        .conn
        .lock()
        .map_err(|_| ApiError::Internal("sqlite lock".to_owned()))?;
    let mut statement = conn
        .prepare(
            "SELECT id, room, author_node_id, created_at_ms, text, signature
             FROM messages
             WHERE room = ?1 AND created_at_ms > ?2
             ORDER BY created_at_ms ASC, id ASC",
        )
        .map_err(map_store_error)?;
    let messages = statement
        .query_map(params![room, after_ms], row_to_message)
        .map_err(map_store_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(map_store_error)?;
    Ok(messages)
}

#[cfg(test)]
fn list_quarantine_messages(
    state: &AppState,
    author_node_id: &str,
) -> Result<Vec<ChatMessage>, ApiError> {
    let conn = state
        .store
        .conn
        .lock()
        .map_err(|_| ApiError::Internal("sqlite lock".to_owned()))?;
    let mut statement = conn
        .prepare(
            "SELECT id, room, author_node_id, created_at_ms, text, signature
             FROM quarantine_messages
             WHERE author_node_id = ?1
             ORDER BY created_at_ms ASC, id ASC",
        )
        .map_err(map_store_error)?;
    let messages = statement
        .query_map(params![author_node_id], row_to_message)
        .map_err(map_store_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(map_store_error)?;
    Ok(messages)
}

fn store_message(state: &AppState, message: ChatMessage) -> Result<(), ApiError> {
    let conn = state
        .store
        .conn
        .lock()
        .map_err(|_| ApiError::Internal("sqlite lock".to_owned()))?;
    conn.execute(
        "INSERT OR IGNORE INTO messages (id, room, author_node_id, created_at_ms, text, signature)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            message.id,
            message.room,
            message.author_node_id,
            i64::try_from(message.created_at_ms).unwrap_or(i64::MAX),
            message.text,
            message.signature
        ],
    )
    .map_err(map_store_error)?;
    Ok(())
}

fn quarantine_unknown_message(state: &AppState, message: ChatMessage) -> Result<(), ApiError> {
    let mut conn = state
        .store
        .conn
        .lock()
        .map_err(|_| ApiError::Internal("sqlite lock".to_owned()))?;
    let tx = conn.transaction().map_err(map_store_error)?;
    tx.execute(
        r#"
        INSERT INTO peers (node_id, node_url, name, last_seen_ms, trust_state, source)
        VALUES (?1, '', NULL, ?2, 'unknown', 'message')
        ON CONFLICT(node_id) DO UPDATE SET
            last_seen_ms = CASE
                WHEN peers.trust_state = 'unknown' THEN excluded.last_seen_ms
                ELSE peers.last_seen_ms
            END
        "#,
        params![
            &message.author_node_id,
            i64::try_from(now_ms()).unwrap_or(i64::MAX)
        ],
    )
    .map_err(map_store_error)?;

    let (trust_state, peer_url) = tx
        .query_row(
            "SELECT trust_state, node_url FROM peers WHERE node_id = ?1",
            params![&message.author_node_id],
            |row| {
                Ok((
                    trust_state_from_db(&row.get::<_, String>(0)?)?,
                    row.get::<_, String>(1)?,
                ))
            },
        )
        .map_err(map_store_error)?;
    if trust_state != PeerTrustState::Unknown {
        tx.commit().map_err(map_store_error)?;
        return Ok(());
    }

    tx.execute(
        "INSERT OR IGNORE INTO quarantine_messages (id, room, author_node_id, created_at_ms, text, signature)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            &message.id,
            &message.room,
            &message.author_node_id,
            i64::try_from(message.created_at_ms).unwrap_or(i64::MAX),
            &message.text,
            &message.signature
        ],
    )
    .map_err(map_store_error)?;

    let peer_url = (!peer_url.is_empty()).then_some(peer_url);
    let text = if let Some(peer_url) = peer_url {
        format!(
            "Unknown peer {} ({}) sent a chat message. The content is quarantined and hidden. Verify the node ID out-of-band; then ask the user whether to run `hive peer trust {}` or `hive peer deny {}`.",
            short_node_id(&message.author_node_id), peer_url, message.author_node_id, message.author_node_id
        )
    } else {
        format!(
            "Unknown node {} sent a chat message. The content is quarantined and hidden. Verify the node ID out-of-band; then ask the user whether to run `hive peer trust {}` or `hive peer deny {}`.",
            short_node_id(&message.author_node_id), message.author_node_id, message.author_node_id
        )
    };
    let notice = state.key.sign_chat("default", now_ms(), &text);
    let created_at_ms = i64::try_from(notice.created_at_ms).unwrap_or(i64::MAX);
    let changed = tx
        .execute(
            "INSERT OR IGNORE INTO unknown_notices (node_id) VALUES (?1)",
            params![&message.author_node_id],
        )
        .map_err(map_store_error)?;
    if changed > 0 {
        tx.execute(
            "INSERT OR IGNORE INTO messages (id, room, author_node_id, created_at_ms, text, signature)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                notice.id,
                notice.room,
                notice.author_node_id,
                created_at_ms,
                notice.text,
                notice.signature
            ],
        )
        .map_err(map_store_error)?;
    }
    tx.commit().map_err(map_store_error)
}

fn spawn_discovery(bind_addr: SocketAddr, public_url: Option<String>, state: AppState) {
    tokio::spawn(async move {
        if let Err(err) = discovery_loop(bind_addr, public_url, state).await {
            eprintln!("discovery stopped: {err}");
        }
    });
}

async fn discovery_loop(
    bind_addr: SocketAddr,
    public_url: Option<String>,
    state: AppState,
) -> std::io::Result<()> {
    let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, DISCOVERY_PORT)).await?;
    socket.set_broadcast(true)?;
    let mut fast = tokio::time::interval(Duration::from_secs(BEACON_FAST_SECS));
    let mut slow = tokio::time::interval(Duration::from_secs(BEACON_SLOW_SECS));
    let mut buf = [0_u8; 2048];

    loop {
        tokio::select! {
            _ = fast.tick(), if peer_count(&state) == 0 => beacon(&socket, bind_addr, public_url.as_deref(), &state).await,
            _ = slow.tick(), if peer_count(&state) > 0 => beacon(&socket, bind_addr, public_url.as_deref(), &state).await,
            received = socket.recv_from(&mut buf) => {
                let (len, peer_addr) = received?;
                let text = String::from_utf8_lossy(&buf[..len]);
                if let Some(peer) = parse_beacon(&text) {
                    if peer.node_id != state.key.node_id() {
                        if peer_trust_state(&state, &peer.node_id) == PeerTrustState::Trusted {
                            tokio::spawn(verify_and_remember_peer(state.clone(), peer.clone(), "beacon"));
                        } else if remember_peer(&state, peer.clone(), "beacon").is_ok() {
                            tokio::spawn(fetch_peer_list(state.clone(), peer.clone()));
                        }
                    }
                } else if text.trim() == "HIVEMIND_DISCOVER_V2" {
                    let info = self_peer(bind_addr, public_url.as_deref(), &state, peer_addr);
                    let _ = socket.send_to(beacon_text(&info).as_bytes(), peer_addr).await;
                }
            }
        }
    }
}

async fn beacon(
    socket: &UdpSocket,
    bind_addr: SocketAddr,
    public_url: Option<&str>,
    state: &AppState,
) {
    let targets = [
        SocketAddr::from((Ipv4Addr::BROADCAST, DISCOVERY_PORT)),
        SocketAddr::from((Ipv4Addr::LOCALHOST, DISCOVERY_PORT)),
    ];
    for target in targets {
        let peer = self_peer(bind_addr, public_url, state, target);
        let _ = socket.send_to(beacon_text(&peer).as_bytes(), target).await;
    }
}

fn self_peer(
    bind_addr: SocketAddr,
    public_url: Option<&str>,
    state: &AppState,
    target: SocketAddr,
) -> PeerInfo {
    PeerInfo {
        node_url: public_url
            .map(str::to_owned)
            .unwrap_or_else(|| inferred_node_url(bind_addr, target)),
        node_id: state.key.node_id(),
        name: local_node_name(),
    }
}

fn beacon_text(peer: &PeerInfo) -> String {
    match peer.name.as_deref().and_then(safe_peer_name) {
        Some(name) => format!(
            "{DISCOVERY_PREFIX}{} {} {}",
            peer.node_url, peer.node_id, name
        ),
        None => format!("{DISCOVERY_PREFIX}{} {}", peer.node_url, peer.node_id),
    }
}

fn parse_beacon(input: &str) -> Option<PeerInfo> {
    let rest = input.strip_prefix(DISCOVERY_PREFIX)?;
    let mut parts = rest.split_whitespace();
    let node_url = parts.next()?.trim_end_matches('/').to_owned();
    let node_id = parts.next()?.to_owned();
    let name = parts.next().and_then(safe_peer_name).map(str::to_owned);
    if valid_node_url(&node_url) && valid_node_id(&node_id) {
        Some(PeerInfo {
            node_url,
            node_id,
            name,
        })
    } else {
        None
    }
}

fn remember_peer(state: &AppState, peer: PeerInfo, source: &str) -> Result<(), ApiError> {
    upsert_peer(state, peer, source, false)
}

fn remember_verified_peer(state: &AppState, peer: PeerInfo, source: &str) -> Result<(), ApiError> {
    upsert_peer(state, peer, source, true)
}

fn upsert_peer(
    state: &AppState,
    peer: PeerInfo,
    source: &str,
    verified_identity: bool,
) -> Result<(), ApiError> {
    if !valid_peer_info(&peer) {
        return Err(ApiError::InvalidRequest);
    }
    let seen_at = now_ms();
    let conn = state
        .store
        .conn
        .lock()
        .map_err(|_| ApiError::Internal("sqlite lock".to_owned()))?;
    let existing = conn
        .query_row(
            "SELECT trust_state FROM peers WHERE node_id = ?1",
            params![peer.node_id],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(map_store_error)?;
    if existing.is_none() && peer_count_locked(&conn) >= MAX_PEERS {
        return Err(ApiError::InvalidRequest);
    }
    conn.execute(
        r#"
        INSERT INTO peers (node_id, node_url, name, last_seen_ms, trust_state, source)
        VALUES (?1, ?2, ?3, ?4, 'unknown', ?5)
        ON CONFLICT(node_id) DO UPDATE SET
            node_url = CASE
                WHEN peers.trust_state = 'unknown' OR (peers.trust_state = 'trusted' AND ?6) THEN excluded.node_url
                ELSE peers.node_url
            END,
            name = CASE
                WHEN peers.trust_state = 'unknown' OR (peers.trust_state = 'trusted' AND ?6) THEN COALESCE(excluded.name, peers.name)
                ELSE peers.name
            END,
            last_seen_ms = excluded.last_seen_ms,
            source = CASE
                WHEN peers.trust_state = 'unknown' OR (peers.trust_state = 'trusted' AND ?6) THEN excluded.source
                ELSE peers.source
            END
        "#,
        params![
            peer.node_id,
            peer.node_url,
            peer.name,
            i64::try_from(seen_at).unwrap_or(i64::MAX),
            source,
            verified_identity
        ],
    )
    .map_err(map_store_error)?;
    Ok(())
}

async fn verify_and_remember_peer(state: AppState, peer: PeerInfo, source: &'static str) {
    let Ok(client) = reqwest::Client::builder()
        .timeout(Duration::from_secs(PEER_FETCH_TIMEOUT_SECS))
        .build()
    else {
        return;
    };
    if let Some(verified_peer) = verify_peer_identity(&client, &peer).await {
        let _ = remember_verified_peer(&state, verified_peer.clone(), source);
        fetch_peer_list(state, verified_peer).await;
    }
}

async fn verify_peer_identity(client: &reqwest::Client, peer: &PeerInfo) -> Option<PeerInfo> {
    let nonce = format!("{}-{}", short_node_id(&peer.node_id), now_ms());
    let Ok(response) = client
        .get(format!("{}/v1/node/proof?nonce={}", peer.node_url, nonce))
        .send()
        .await
    else {
        return None;
    };
    let Ok(response) = response.error_for_status() else {
        return None;
    };
    let Ok(proof) = response.json::<NodeProof>().await else {
        return None;
    };
    verified_peer_from_proof(peer, &proof, &nonce)
}

fn verified_peer_from_proof(peer: &PeerInfo, proof: &NodeProof, nonce: &str) -> Option<PeerInfo> {
    if proof.node_id != peer.node_id
        || normalized_node_url(&proof.node_url) != normalized_node_url(&peer.node_url)
        || proof.nonce != nonce
        || proof.verify().is_err()
    {
        return None;
    }
    let verified_peer = PeerInfo {
        node_url: normalized_node_url(&proof.node_url),
        node_id: proof.node_id.clone(),
        name: proof.name.clone(),
    };
    valid_peer_info(&verified_peer).then_some(verified_peer)
}

async fn fetch_peer_list(state: AppState, peer: PeerInfo) {
    let Ok(client) = reqwest::Client::builder()
        .timeout(Duration::from_secs(PEER_FETCH_TIMEOUT_SECS))
        .build()
    else {
        return;
    };
    let Ok(response) = client
        .get(format!("{}/v1/peers", peer.node_url))
        .send()
        .await
    else {
        return;
    };
    let Ok(response) = response.error_for_status() else {
        return;
    };
    let Ok(peers) = response.json::<PeersResponse>().await else {
        return;
    };
    for found in peers.peers {
        if found.node_id != state.key.node_id() && found.node_id != peer.node_id {
            let _ = remember_peer(
                &state,
                PeerInfo {
                    node_url: found.node_url,
                    node_id: found.node_id,
                    name: found.name,
                },
                "gossip",
            );
        }
    }
}

fn gossip_message(state: AppState, message: ChatMessage) {
    let peers = trusted_peers(&state);
    tokio::spawn(async move {
        let client = reqwest::Client::new();
        for peer in peers {
            let _ = client
                .post(format!("{}/v1/chat/import", peer.node_url))
                .json(&message)
                .send()
                .await;
        }
    });
}

fn peer_trust_state(state: &AppState, node_id: &str) -> PeerTrustState {
    get_peer(state, node_id)
        .ok()
        .flatten()
        .map(|peer| peer.trust_state)
        .unwrap_or(PeerTrustState::Unknown)
}

fn trusted_peers(state: &AppState) -> Vec<PeerRecord> {
    list_peers(state)
        .unwrap_or_default()
        .into_iter()
        .filter(|peer| peer.trust_state == PeerTrustState::Trusted)
        .collect()
}

fn peer_count(state: &AppState) -> usize {
    state
        .store
        .conn
        .lock()
        .ok()
        .and_then(|conn| peer_count_locked(&conn).try_into().ok())
        .unwrap_or(0)
}

fn peer_count_locked(conn: &Connection) -> i64 {
    conn.query_row("SELECT COUNT(*) FROM peers", [], |row| row.get::<_, i64>(0))
        .unwrap_or(0)
}

fn valid_peer_info(peer: &PeerInfo) -> bool {
    valid_node_id(&peer.node_id)
        && valid_node_url(&peer.node_url)
        && peer.name.as_deref().and_then(safe_peer_name) == peer.name.as_deref()
}

fn valid_message_payload(message: &ChatMessage) -> bool {
    message.id.len() == 64
        && message.id.bytes().all(|byte| byte.is_ascii_hexdigit())
        && valid_node_id(&message.author_node_id)
        && valid_room(&message.room)
        && valid_chat_text(&message.text)
        && message.signature.len() == 128
        && message
            .signature
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
}

fn valid_room(room: &str) -> bool {
    !room.is_empty()
        && room.len() <= MAX_ROOM_BYTES
        && room
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

fn valid_chat_text(text: &str) -> bool {
    !text.trim().is_empty() && text.len() <= MAX_CHAT_TEXT_BYTES
}

fn valid_nonce(nonce: &str) -> bool {
    !nonce.is_empty()
        && nonce.len() <= MAX_NONCE_BYTES
        && nonce
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

fn short_node_id(node_id: &str) -> String {
    node_id.chars().take(8).collect()
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn local_node_name() -> Option<String> {
    env::var("HIVEMIND_NODE_NAME")
        .ok()
        .or_else(|| env::var("HOSTNAME").ok())
        .or_else(|| env::var("COMPUTERNAME").ok())
        .or_else(|| fs::read_to_string("/etc/hostname").ok())
        .and_then(|name| safe_peer_name(name.trim()).map(str::to_owned))
}

fn safe_peer_name(input: &str) -> Option<&str> {
    let trimmed = input.trim();
    if trimmed.is_empty()
        || trimmed.len() > 64
        || !trimmed
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b'@'))
    {
        None
    } else {
        Some(trimmed)
    }
}

async fn serve(bind_addr: SocketAddr, router: Router) -> std::io::Result<()> {
    let listener = tokio::net::TcpListener::bind(bind_addr).await?;
    eprintln!(
        "hivemind node listening on http://{}",
        listener.local_addr()?
    );
    axum::serve(
        listener,
        router.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;
    use tower::ServiceExt;

    fn local_addr() -> SocketAddr {
        "127.0.0.1:50000".parse().unwrap()
    }

    fn remote_addr() -> SocketAddr {
        "192.0.2.10:50000".parse().unwrap()
    }

    fn test_state() -> AppState {
        test_state_with_store(Store::memory())
    }

    fn test_state_with_store(store: Store) -> AppState {
        AppState {
            key: Arc::new(NodeKey::from_seed_hex(&"01".repeat(32)).unwrap()),
            bind_addr: "127.0.0.1:7747".parse().unwrap(),
            public_url: Some("http://127.0.0.1:7747".to_owned()),
            store: Arc::new(store),
        }
    }

    #[test]
    fn node_url_uses_configured_public_url() {
        let state = test_state();
        assert_eq!(state.node_url(), "http://127.0.0.1:7747");
    }

    #[test]
    fn node_url_is_computed_at_runtime_without_public_url() {
        let state = AppState {
            key: Arc::new(NodeKey::from_seed_hex(&"01".repeat(32)).unwrap()),
            bind_addr: "127.0.0.1:18888".parse().unwrap(),
            public_url: None,
            store: Arc::new(Store::memory()),
        };
        assert_eq!(state.node_url(), "http://127.0.0.1:18888");
    }

    #[tokio::test]
    async fn node_proof_route_returns_signed_metadata() {
        let state = test_state();
        let response = app(state)
            .oneshot(
                axum::http::Request::builder()
                    .uri("/v1/node/proof?nonce=abc")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let proof: NodeProof = serde_json::from_slice(&bytes).unwrap();
        proof.verify().unwrap();
        assert_eq!(proof.node_url, "http://127.0.0.1:7747");
        assert_eq!(proof.nonce, "abc");
    }

    #[test]
    fn parses_beacon() {
        let node_id = "a".repeat(64);
        assert_eq!(
            parse_beacon(&format!("{DISCOVERY_PREFIX}http://127.0.0.1:1 {node_id}")),
            Some(PeerInfo {
                node_url: "http://127.0.0.1:1".to_owned(),
                node_id,
                name: None,
            })
        );
    }

    #[test]
    fn parses_beacon_name() {
        let node_id = "a".repeat(64);
        assert_eq!(
            parse_beacon(&format!(
                "{DISCOVERY_PREFIX}http://127.0.0.1:1 {node_id} joris-mac"
            )),
            Some(PeerInfo {
                node_url: "http://127.0.0.1:1".to_owned(),
                node_id,
                name: Some("joris-mac".to_owned()),
            })
        );
    }

    #[test]
    fn rejects_bad_beacon() {
        assert_eq!(parse_beacon("bad"), None);
        assert_eq!(
            parse_beacon(&format!("{DISCOVERY_PREFIX}ftp://x {}", "a".repeat(64))),
            None
        );
    }

    #[test]
    fn trusted_peers_excludes_unknown_candidates() {
        let state = test_state();
        remember_peer(
            &state,
            PeerInfo {
                node_url: "http://unknown".to_owned(),
                node_id: "b".repeat(64),
                name: None,
            },
            "test",
        )
        .unwrap();
        remember_peer(
            &state,
            PeerInfo {
                node_url: "http://trusted".to_owned(),
                node_id: "c".repeat(64),
                name: Some("trusted-host".to_owned()),
            },
            "test",
        )
        .unwrap();
        trust_peer_record(&state, &"c".repeat(64)).unwrap();
        let peers = trusted_peers(&state);
        assert_eq!(peers.len(), 1);
        assert_eq!(peers[0].node_url, "http://trusted");
    }

    #[test]
    fn unverified_update_does_not_change_trusted_peer_url() {
        let state = test_state();
        let node_id = "b".repeat(64);
        remember_peer(
            &state,
            PeerInfo {
                node_url: "http://trusted.example:7747".to_owned(),
                node_id: node_id.clone(),
                name: Some("real-host".to_owned()),
            },
            "test",
        )
        .unwrap();
        trust_peer_record(&state, &node_id).unwrap();

        remember_peer(
            &state,
            PeerInfo {
                node_url: "http://attacker.example:7747".to_owned(),
                node_id: node_id.clone(),
                name: Some("fake-host".to_owned()),
            },
            "beacon",
        )
        .unwrap();

        let peer = get_peer(&state, &node_id).unwrap().unwrap();
        assert_eq!(peer.node_url, "http://trusted.example:7747");
        assert_eq!(peer.name.as_deref(), Some("real-host"));
        assert_eq!(peer.source, "test");
        assert_eq!(peer.trust_state, PeerTrustState::Trusted);
    }

    #[test]
    fn node_proof_conversion_uses_signed_peer_metadata() {
        let key = NodeKey::from_seed_hex(&"02".repeat(32)).unwrap();
        let signed = key.sign_node_proof(
            "http://new.example:7747",
            Some("signed-host".to_owned()),
            "abc",
        );
        let spoofed_beacon = PeerInfo {
            node_url: "http://new.example:7747".to_owned(),
            node_id: key.node_id(),
            name: Some("fake-host".to_owned()),
        };
        let verified = verified_peer_from_proof(&spoofed_beacon, &signed, "abc").unwrap();
        assert_eq!(verified.name.as_deref(), Some("signed-host"));
    }

    #[test]
    fn verified_update_uses_signed_peer_metadata() {
        let state = test_state();
        let node_id = "b".repeat(64);
        remember_peer(
            &state,
            PeerInfo {
                node_url: "http://old.example:7747".to_owned(),
                node_id: node_id.clone(),
                name: Some("old-host".to_owned()),
            },
            "test",
        )
        .unwrap();
        trust_peer_record(&state, &node_id).unwrap();

        remember_verified_peer(
            &state,
            PeerInfo {
                node_url: "http://new.example:7747".to_owned(),
                node_id: node_id.clone(),
                name: Some("signed-host".to_owned()),
            },
            "verified",
        )
        .unwrap();

        let peer = get_peer(&state, &node_id).unwrap().unwrap();
        assert_eq!(peer.node_url, "http://new.example:7747");
        assert_eq!(peer.name.as_deref(), Some("signed-host"));
        assert_eq!(peer.source, "verified");
        assert_eq!(peer.trust_state, PeerTrustState::Trusted);
    }

    #[test]
    fn verified_update_can_change_trusted_peer_url() {
        let state = test_state();
        let node_id = "b".repeat(64);
        remember_peer(
            &state,
            PeerInfo {
                node_url: "http://old.example:7747".to_owned(),
                node_id: node_id.clone(),
                name: Some("old-host".to_owned()),
            },
            "test",
        )
        .unwrap();
        trust_peer_record(&state, &node_id).unwrap();

        remember_verified_peer(
            &state,
            PeerInfo {
                node_url: "http://new.example:7747".to_owned(),
                node_id: node_id.clone(),
                name: Some("new-host".to_owned()),
            },
            "verified",
        )
        .unwrap();

        let peer = get_peer(&state, &node_id).unwrap().unwrap();
        assert_eq!(peer.node_url, "http://new.example:7747");
        assert_eq!(peer.name.as_deref(), Some("new-host"));
        assert_eq!(peer.source, "verified");
        assert_eq!(peer.trust_state, PeerTrustState::Trusted);
    }

    #[test]
    fn validates_node_urls_strictly() {
        assert!(valid_node_url("http://127.0.0.1:7747"));
        assert!(valid_node_url("https://hivemind.jhx.app"));
        assert!(!valid_node_url("ftp://127.0.0.1"));
        assert!(!valid_node_url("http://user:pass@example.com"));
        assert!(!valid_node_url("http://example.com/#fragment"));
        assert!(!valid_node_url("http://example.com/bad url"));
    }

    #[test]
    fn rejects_invalid_room_and_oversized_chat_text() {
        assert!(valid_room("default"));
        assert!(!valid_room("bad room"));
        assert!(!valid_room(""));
        assert!(valid_chat_text("hello"));
        assert!(!valid_chat_text("   "));
        assert!(!valid_chat_text(&"x".repeat(MAX_CHAT_TEXT_BYTES + 1)));
    }

    #[test]
    fn sqlite_store_sets_user_version() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.sqlite3");
        let state = test_state_with_store(Store::open(&path).unwrap());
        let version: i64 = state
            .store
            .conn
            .lock()
            .unwrap()
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, 2);
    }

    #[test]
    fn sqlite_store_uses_trust_state_as_peer_source_of_truth() {
        let state = test_state();
        let conn = state.store.conn.lock().unwrap();
        assert!(column_exists(&conn, "peers", "trust_state").unwrap());
        assert!(!column_exists(&conn, "peers", "trusted").unwrap());
    }

    #[test]
    fn sqlite_migration_removes_legacy_trusted_column() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.sqlite3");
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(
                r#"
                CREATE TABLE peers (
                    node_id TEXT PRIMARY KEY NOT NULL,
                    node_url TEXT NOT NULL,
                    name TEXT,
                    last_seen_ms INTEGER NOT NULL,
                    trusted INTEGER NOT NULL,
                    source TEXT NOT NULL
                );
                INSERT INTO peers (node_id, node_url, name, last_seen_ms, trusted, source)
                VALUES ('bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb', 'http://peer', NULL, 123, 1, 'test');
                PRAGMA user_version = 1;
                "#,
            )
            .unwrap();
        }

        let state = test_state_with_store(Store::open(&path).unwrap());
        {
            let conn = state.store.conn.lock().unwrap();
            assert!(column_exists(&conn, "peers", "trust_state").unwrap());
            assert!(!column_exists(&conn, "peers", "trusted").unwrap());
        }
        let peer = get_peer(&state, &"b".repeat(64)).unwrap().unwrap();
        assert_eq!(peer.trust_state, PeerTrustState::Trusted);
        remember_peer(
            &state,
            PeerInfo {
                node_url: "http://new-peer".to_owned(),
                node_id: "c".repeat(64),
                name: None,
            },
            "test",
        )
        .unwrap();
    }

    #[test]
    fn sqlite_store_creates_private_state_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.sqlite3");
        Store::open(&path).unwrap();
        assert!(path.exists());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }

    #[test]
    fn list_messages_handles_huge_after_ms() {
        let state = test_state();
        store_message(&state, state.key.sign_chat("default", 123, "hello")).unwrap();
        let messages = list_messages(&state, "default", u64::MAX).unwrap();
        assert!(messages.is_empty());
    }

    #[test]
    fn sqlite_store_persists_peers_and_messages() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.sqlite3");
        {
            let state = test_state_with_store(Store::open(&path).unwrap());
            remember_peer(
                &state,
                PeerInfo {
                    node_url: "http://peer".to_owned(),
                    node_id: "b".repeat(64),
                    name: Some("peer-host".to_owned()),
                },
                "test",
            )
            .unwrap();
            trust_peer_record(&state, &"b".repeat(64)).unwrap();
            store_message(&state, state.key.sign_chat("default", 123, "persist me")).unwrap();
        }

        let state = test_state_with_store(Store::open(&path).unwrap());
        let peer = get_peer(&state, &"b".repeat(64)).unwrap().unwrap();
        assert_eq!(peer.trust_state, PeerTrustState::Trusted);
        assert_eq!(peer.name.as_deref(), Some("peer-host"));
        let messages = list_messages(&state, "default", 0).unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].text, "persist me");
    }

    #[test]
    fn sqlite_store_persists_unknown_notice_dedupe() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.sqlite3");
        let author = NodeKey::from_seed_hex(&"02".repeat(32)).unwrap();
        {
            let state = test_state_with_store(Store::open(&path).unwrap());
            quarantine_unknown_message(&state, author.sign_chat("default", 123, "first")).unwrap();
        }

        let state = test_state_with_store(Store::open(&path).unwrap());
        quarantine_unknown_message(&state, author.sign_chat("default", 124, "second")).unwrap();
        let messages = list_messages(&state, "default", 0).unwrap();
        assert_eq!(messages.len(), 1);
    }

    #[test]
    fn remember_peer_does_not_trust() {
        let state = test_state();
        remember_peer(
            &state,
            PeerInfo {
                node_url: "http://peer".to_owned(),
                node_id: "b".repeat(64),
                name: Some("peer-host".to_owned()),
            },
            "test",
        )
        .unwrap();
        let peers = list_peers(&state).unwrap();
        let peer = peers.first().unwrap();
        assert_eq!(peer.trust_state, PeerTrustState::Unknown);
        assert_eq!(peer.name.as_deref(), Some("peer-host"));
    }

    #[tokio::test]
    async fn add_peer_stores_unknown_candidate() {
        let state = test_state();
        let response = app(state)
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/v1/peers")
                    .extension(ConnectInfo(local_addr()))
                    .header("content-type", "application/json")
                    .body(axum::body::Body::from(format!(
                        r#"{{"node_url":"http://peer","node_id":"{}"}}"#,
                        "b".repeat(64)
                    )))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let peer: PeerRecord = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(peer.trust_state, PeerTrustState::Unknown);
        assert_eq!(peer.source, "manual");
    }

    #[tokio::test]
    async fn join_returns_self_and_candidate() {
        let state = test_state();
        let self_id = state.key.node_id();
        let response = app(state)
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/v1/join")
                    .header("content-type", "application/json")
                    .body(axum::body::Body::from(format!(
                        r#"{{"node_url":"http://peer","node_id":"{}"}}"#,
                        "b".repeat(64)
                    )))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let peers: PeersResponse = serde_json::from_slice(&bytes).unwrap();
        assert!(peers.peers.iter().any(|peer| peer.node_id == self_id));
        assert!(peers
            .peers
            .iter()
            .any(|peer| peer.node_id == "b".repeat(64)
                && peer.trust_state == PeerTrustState::Unknown));
    }

    #[tokio::test]
    async fn trust_marks_existing_peer_only() {
        let state = test_state();
        remember_peer(
            &state,
            PeerInfo {
                node_url: "http://peer".to_owned(),
                node_id: "b".repeat(64),
                name: None,
            },
            "test",
        )
        .unwrap();
        let response = app(state)
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri(format!("/v1/peers/{}/trust", "b".repeat(64)))
                    .extension(ConnectInfo(local_addr()))
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let peer: PeerRecord = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(peer.trust_state, PeerTrustState::Trusted);
    }

    #[tokio::test]
    async fn import_quarantines_unknown_author_and_stores_notice() {
        let state = test_state();
        let author = NodeKey::from_seed_hex(&"02".repeat(32)).unwrap();
        let message = author.sign_chat("default", 123, "secret text should not be copied");
        let response = app(state.clone())
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/v1/chat/import")
                    .header("content-type", "application/json")
                    .body(axum::body::Body::from(
                        serde_json::to_vec(&message).unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);

        let messages = list_messages(&state, "default", 0).unwrap();
        assert_eq!(messages.len(), 1);
        let notice = messages.first().unwrap();
        assert!(notice.text.contains("sent a chat message"));
        assert!(notice.text.contains(&author.node_id()));
        assert!(!notice.text.contains("secret text should not be copied"));
        let quarantined = list_quarantine_messages(&state, &author.node_id()).unwrap();
        assert_eq!(quarantined.len(), 1);
        assert_eq!(quarantined[0].text, "secret text should not be copied");
        notice.verify().unwrap();
    }

    #[tokio::test]
    async fn trusting_unknown_author_releases_quarantine() {
        let state = test_state();
        let author = NodeKey::from_seed_hex(&"02".repeat(32)).unwrap();
        let message = author.sign_chat("default", 123, "release me after trust");
        let response = app(state.clone())
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/v1/chat/import")
                    .header("content-type", "application/json")
                    .body(axum::body::Body::from(
                        serde_json::to_vec(&message).unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        assert_eq!(
            list_quarantine_messages(&state, &author.node_id())
                .unwrap()
                .len(),
            1
        );

        let peer = trust_peer_record(&state, &author.node_id()).unwrap();
        assert_eq!(peer.trust_state, PeerTrustState::Trusted);
        assert!(list_quarantine_messages(&state, &author.node_id())
            .unwrap()
            .is_empty());
        let messages = list_messages(&state, "default", 0).unwrap();
        assert!(messages
            .iter()
            .any(|message| message.text == "release me after trust"));
    }

    #[tokio::test]
    async fn blocked_author_is_dropped_without_quarantine() {
        let state = test_state();
        let author = NodeKey::from_seed_hex(&"02".repeat(32)).unwrap();
        block_peer_record(&state, &author.node_id()).unwrap();
        let message = author.sign_chat("default", 123, "drop me");
        let response = app(state.clone())
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/v1/chat/import")
                    .header("content-type", "application/json")
                    .body(axum::body::Body::from(
                        serde_json::to_vec(&message).unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        assert!(list_quarantine_messages(&state, &author.node_id())
            .unwrap()
            .is_empty());
        assert!(list_messages(&state, "default", 0).unwrap().is_empty());
    }

    #[tokio::test]
    async fn import_accepts_trusted_author() {
        let state = test_state();
        let author = NodeKey::from_seed_hex(&"02".repeat(32)).unwrap();
        remember_peer(
            &state,
            PeerInfo {
                node_url: "http://peer".to_owned(),
                node_id: author.node_id(),
                name: None,
            },
            "test",
        )
        .unwrap();
        trust_peer_record(&state, &author.node_id()).unwrap();
        let message = author.sign_chat("default", 123, "hello");
        let response = app(state)
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/v1/chat/import")
                    .header("content-type", "application/json")
                    .body(axum::body::Body::from(
                        serde_json::to_vec(&message).unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[test]
    fn blocking_trusted_author_deletes_accepted_messages() {
        let state = test_state();
        let author = NodeKey::from_seed_hex(&"02".repeat(32)).unwrap();
        remember_peer(
            &state,
            PeerInfo {
                node_url: "http://peer".to_owned(),
                node_id: author.node_id(),
                name: None,
            },
            "test",
        )
        .unwrap();
        trust_peer_record(&state, &author.node_id()).unwrap();
        store_message(&state, author.sign_chat("default", 123, "remove me")).unwrap();
        assert_eq!(list_messages(&state, "default", 0).unwrap().len(), 1);

        let peer = block_peer_record(&state, &author.node_id()).unwrap();

        assert_eq!(peer.trust_state, PeerTrustState::Blocked);
        assert!(list_messages(&state, "default", 0).unwrap().is_empty());
    }

    #[tokio::test]
    async fn import_rejects_wrong_message_id() {
        let state = test_state();
        let mut message = state.key.sign_chat("default", 123, "hello");
        message.id = "wrong".to_owned();
        let response = app(state)
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/v1/chat/import")
                    .header("content-type", "application/json")
                    .body(axum::body::Body::from(
                        serde_json::to_vec(&message).unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[test]
    fn creates_private_key_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("node.key");
        load_or_create_key(&path).unwrap();
        assert!(path.exists());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }

    #[tokio::test]
    async fn remote_peer_listing_masks_trust() {
        let state = test_state();
        remember_peer(
            &state,
            PeerInfo {
                node_url: "http://peer".to_owned(),
                node_id: "b".repeat(64),
                name: Some("peer-host".to_owned()),
            },
            "test",
        )
        .unwrap();
        trust_peer_record(&state, &"b".repeat(64)).unwrap();
        let response = app(state)
            .oneshot(
                axum::http::Request::builder()
                    .method("GET")
                    .uri("/v1/peers")
                    .extension(ConnectInfo(remote_addr()))
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let peers: PeersResponse = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(peers.peers[0].name.as_deref(), Some("peer-host"));
        assert_eq!(peers.peers[0].trust_state, PeerTrustState::Unknown);
    }

    #[tokio::test]
    async fn remote_clients_cannot_post_local_chat() {
        let state = test_state();
        let response = app(state)
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/v1/chat")
                    .extension(ConnectInfo(remote_addr()))
                    .header("content-type", "application/json")
                    .body(axum::body::Body::from(r#"{"text":"hello"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn remote_clients_cannot_read_local_chat() {
        let state = test_state();
        let response = app(state)
            .oneshot(
                axum::http::Request::builder()
                    .method("GET")
                    .uri("/v1/chat")
                    .extension(ConnectInfo(remote_addr()))
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn remote_clients_cannot_trust_peers() {
        let state = test_state();
        remember_peer(
            &state,
            PeerInfo {
                node_url: "http://peer".to_owned(),
                node_id: "b".repeat(64),
                name: None,
            },
            "test",
        )
        .unwrap();
        let response = app(state)
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri(format!("/v1/peers/{}/trust", "b".repeat(64)))
                    .extension(ConnectInfo(remote_addr()))
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn remote_clients_cannot_deny_peers() {
        let state = test_state();
        remember_peer(
            &state,
            PeerInfo {
                node_url: "http://peer".to_owned(),
                node_id: "b".repeat(64),
                name: None,
            },
            "test",
        )
        .unwrap();
        let response = app(state)
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri(format!("/v1/peers/{}/deny", "b".repeat(64)))
                    .extension(ConnectInfo(remote_addr()))
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn say_stores_signed_message() {
        let state = test_state();
        let response = app(state)
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/v1/chat")
                    .extension(ConnectInfo(local_addr()))
                    .header("content-type", "application/json")
                    .body(axum::body::Body::from(r#"{"text":"hello"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let message: ChatMessage = serde_json::from_slice(&bytes).unwrap();
        message.verify().unwrap();
    }
}
