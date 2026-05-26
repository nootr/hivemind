use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use hivemind_core::{valid_node_id, ChatMessage, NodeKey, PeerInfo, PeerRecord};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, OpenOptions},
    io::Write,
    net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket as StdUdpSocket},
    path::{Path as FsPath, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::net::UdpSocket;

const DISCOVERY_PORT: u16 = 7748;
const DISCOVERY_PREFIX: &str = "HIVEMIND_NODE_V2 ";
const BEACON_FAST_SECS: u64 = 2;
const BEACON_SLOW_SECS: u64 = 20;
const PEER_FETCH_TIMEOUT_SECS: u64 = 2;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub struct NodeConfig {
    pub data_dir: PathBuf,
    pub bind_addr: SocketAddr,
    pub public_url: Option<String>,
}

impl NodeConfig {
    pub fn from_file(path: impl AsRef<FsPath>) -> Result<Self, NodeError> {
        let input = fs::read_to_string(path)?;
        Ok(toml::from_str(&input)?)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum NodeError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("toml error: {0}")]
    Toml(#[from] toml::de::Error),
    #[error("core error: {0}")]
    Core(#[from] hivemind_core::CoreError),
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
    node_url: String,
    store: Arc<Store>,
}

#[derive(Default)]
struct Store {
    peers: Mutex<BTreeMap<String, PeerRecord>>,
    messages: Mutex<BTreeMap<String, ChatMessage>>,
    untrusted_notices: Mutex<BTreeSet<String>>,
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

#[derive(Debug, Serialize, Deserialize, Eq, PartialEq)]
pub struct NodeInfoResponse {
    pub node_url: String,
    pub node_id: String,
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
    let node_url = config
        .public_url
        .clone()
        .unwrap_or_else(|| local_node_url(config.bind_addr));
    let store = Arc::new(Store::default());
    let state = AppState {
        key: Arc::new(key),
        node_url,
        store,
    };

    spawn_discovery(config.bind_addr, config.public_url, state.clone());
    serve(config.bind_addr, app(state)).await?;
    Ok(())
}

pub fn app(state: AppState) -> Router {
    Router::new()
        .route("/health", get(|| async { "ok" }))
        .route("/v1/node", get(get_node))
        .route("/v1/peers", get(get_peers).post(add_peer))
        .route("/v1/join", post(join_network))
        .route("/v1/peers/{node_id}/trust", post(trust_peer))
        .route("/v1/chat", get(get_messages).post(say))
        .route("/v1/chat/import", post(import_message))
        .with_state(state)
}

async fn get_node(State(state): State<AppState>) -> Json<NodeInfoResponse> {
    Json(NodeInfoResponse {
        node_url: state.node_url,
        node_id: state.key.node_id(),
    })
}

async fn get_peers(State(state): State<AppState>) -> Result<Json<PeersResponse>, ApiError> {
    let peers = state
        .store
        .peers
        .lock()
        .map_err(|_| ApiError::Internal("peer lock".to_owned()))?
        .values()
        .cloned()
        .collect();
    Ok(Json(PeersResponse { peers }))
}

async fn add_peer(
    State(state): State<AppState>,
    Json(peer): Json<PeerInfo>,
) -> Result<Json<PeerRecord>, ApiError> {
    if !valid_node_id(&peer.node_id)
        || !valid_node_url(&peer.node_url)
        || peer.node_id == state.key.node_id()
    {
        return Err(ApiError::InvalidRequest);
    }
    let node_id = peer.node_id.clone();
    remember_peer(&state, peer, "manual");
    let peers = state
        .store
        .peers
        .lock()
        .map_err(|_| ApiError::Internal("peer lock".to_owned()))?;
    Ok(Json(
        peers.get(&node_id).cloned().ok_or(ApiError::NotFound)?,
    ))
}

async fn join_network(
    State(state): State<AppState>,
    Json(peer): Json<PeerInfo>,
) -> Result<Json<PeersResponse>, ApiError> {
    if !valid_node_id(&peer.node_id)
        || !valid_node_url(&peer.node_url)
        || peer.node_id == state.key.node_id()
    {
        return Err(ApiError::InvalidRequest);
    }
    remember_peer(&state, peer, "join");
    let mut peers: Vec<PeerRecord> = state
        .store
        .peers
        .lock()
        .map_err(|_| ApiError::Internal("peer lock".to_owned()))?
        .values()
        .cloned()
        .collect();
    peers.push(PeerRecord {
        node_url: state.node_url.clone(),
        node_id: state.key.node_id(),
        trusted: false,
        source: "self".to_owned(),
    });
    Ok(Json(PeersResponse { peers }))
}

async fn trust_peer(
    State(state): State<AppState>,
    Path(node_id): Path<String>,
) -> Result<Json<PeerRecord>, ApiError> {
    if !valid_node_id(&node_id) {
        return Err(ApiError::InvalidRequest);
    }
    let mut peers = state
        .store
        .peers
        .lock()
        .map_err(|_| ApiError::Internal("peer lock".to_owned()))?;
    let peer = peers.get_mut(&node_id).ok_or(ApiError::NotFound)?;
    peer.trusted = true;
    Ok(Json(peer.clone()))
}

async fn say(
    State(state): State<AppState>,
    Json(request): Json<SayRequest>,
) -> Result<Json<ChatMessage>, ApiError> {
    let message = state.key.sign_chat(&request.room, now_ms(), &request.text);
    store_message(&state, message.clone())?;
    gossip_message(state, message.clone());
    Ok(Json(message))
}

async fn import_message(
    State(state): State<AppState>,
    Json(message): Json<ChatMessage>,
) -> Result<Json<ChatMessage>, ApiError> {
    message.verify().map_err(|_| ApiError::InvalidRequest)?;
    if message.author_node_id != state.key.node_id()
        && !trusted_author(&state, &message.author_node_id)
    {
        store_untrusted_author_notice(&state, &message.author_node_id)?;
        return Err(ApiError::Forbidden);
    }
    store_message(&state, message.clone())?;
    Ok(Json(message))
}

async fn get_messages(
    State(state): State<AppState>,
    Query(query): Query<MessagesQuery>,
) -> Result<Json<MessagesResponse>, ApiError> {
    let after_ms = query.after_ms.unwrap_or(0);
    let messages = state
        .store
        .messages
        .lock()
        .map_err(|_| ApiError::Internal("message lock".to_owned()))?
        .values()
        .filter(|message| message.room == query.room && message.created_at_ms > after_ms)
        .cloned()
        .collect();
    Ok(Json(MessagesResponse { messages }))
}

fn store_message(state: &AppState, message: ChatMessage) -> Result<(), ApiError> {
    state
        .store
        .messages
        .lock()
        .map_err(|_| ApiError::Internal("message lock".to_owned()))?
        .insert(message.id.clone(), message);
    Ok(())
}

fn store_untrusted_author_notice(state: &AppState, author_node_id: &str) -> Result<(), ApiError> {
    let inserted = state
        .store
        .untrusted_notices
        .lock()
        .map_err(|_| ApiError::Internal("notice lock".to_owned()))?
        .insert(author_node_id.to_owned());
    if !inserted {
        return Ok(());
    }

    let known_peer = state
        .store
        .peers
        .lock()
        .map_err(|_| ApiError::Internal("peer lock".to_owned()))?
        .get(author_node_id)
        .cloned();
    let text = if let Some(peer) = known_peer {
        format!(
            "Untrusted peer {} ({}) tried to send a chat message. The message was ignored. Verify the node ID out-of-band; if you trust it, run: hive peer trust {}",
            short_node_id(author_node_id), peer.node_url, author_node_id
        )
    } else {
        format!(
            "Unknown untrusted node {} tried to send a chat message. The message was ignored. Join and verify the peer out-of-band before trusting node ID {}.",
            short_node_id(author_node_id), author_node_id
        )
    };
    let notice = state.key.sign_chat("default", now_ms(), &text);
    store_message(state, notice)
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
                        remember_peer(&state, peer.clone(), "beacon");
                        tokio::spawn(fetch_peer_list(state.clone(), peer.clone()));
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
    }
}

fn beacon_text(peer: &PeerInfo) -> String {
    format!("{DISCOVERY_PREFIX}{} {}", peer.node_url, peer.node_id)
}

fn parse_beacon(input: &str) -> Option<PeerInfo> {
    let rest = input.strip_prefix(DISCOVERY_PREFIX)?;
    let mut parts = rest.split_whitespace();
    let node_url = parts.next()?.trim_end_matches('/').to_owned();
    let node_id = parts.next()?.to_owned();
    if valid_node_url(&node_url) && valid_node_id(&node_id) {
        Some(PeerInfo { node_url, node_id })
    } else {
        None
    }
}

fn remember_peer(state: &AppState, peer: PeerInfo, source: &str) {
    let mut peers = state.store.peers.lock().expect("peer lock");
    peers
        .entry(peer.node_id.clone())
        .and_modify(|existing| existing.node_url = peer.node_url.clone())
        .or_insert(PeerRecord {
            node_url: peer.node_url,
            node_id: peer.node_id,
            trusted: false,
            source: source.to_owned(),
        });
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
            remember_peer(
                &state,
                PeerInfo {
                    node_url: found.node_url,
                    node_id: found.node_id,
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

fn trusted_author(state: &AppState, node_id: &str) -> bool {
    state
        .store
        .peers
        .lock()
        .expect("peer lock")
        .get(node_id)
        .map(|peer| peer.trusted)
        .unwrap_or(false)
}

fn trusted_peers(state: &AppState) -> Vec<PeerRecord> {
    state
        .store
        .peers
        .lock()
        .expect("peer lock")
        .values()
        .filter(|peer| peer.trusted)
        .cloned()
        .collect()
}

fn peer_count(state: &AppState) -> usize {
    state.store.peers.lock().expect("peer lock").len()
}

fn inferred_node_url(bind_addr: SocketAddr, target: SocketAddr) -> String {
    let port = bind_addr.port();
    if bind_addr.ip().is_loopback() {
        return format!("http://127.0.0.1:{port}");
    }
    let ip = outbound_ip_for(target).unwrap_or(bind_addr.ip());
    let host = match ip {
        IpAddr::V4(ip) => ip.to_string(),
        IpAddr::V6(ip) => format!("[{ip}]"),
    };
    format!("http://{host}:{port}")
}

fn outbound_ip_for(peer: SocketAddr) -> Option<IpAddr> {
    let socket = StdUdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).ok()?;
    socket.connect(peer).ok()?;
    Some(socket.local_addr().ok()?.ip())
}

fn valid_node_url(url: &str) -> bool {
    url.starts_with("http://") || url.starts_with("https://")
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

fn load_or_create_key(path: &FsPath) -> Result<NodeKey, NodeError> {
    if path.exists() {
        restrict_key_permissions(path)?;
        return Ok(NodeKey::from_seed_hex(&fs::read_to_string(path)?)?);
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let key = NodeKey::generate()?;
    write_private_key(path, &format!("{}\n", key.seed_hex()))?;
    Ok(key)
}

#[cfg(unix)]
fn write_private_key(path: &FsPath, content: &str) -> std::io::Result<()> {
    use std::os::unix::fs::OpenOptionsExt;

    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(content.as_bytes())?;
    restrict_key_permissions(path)
}

#[cfg(not(unix))]
fn write_private_key(path: &FsPath, content: &str) -> std::io::Result<()> {
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    file.write_all(content.as_bytes())
}

#[cfg(unix)]
fn restrict_key_permissions(path: &FsPath) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let mut permissions = fs::metadata(path)?.permissions();
    permissions.set_mode(0o600);
    fs::set_permissions(path, permissions)
}

#[cfg(not(unix))]
fn restrict_key_permissions(_path: &FsPath) -> std::io::Result<()> {
    Ok(())
}

fn local_node_url(bind_addr: SocketAddr) -> String {
    let host = if bind_addr.ip().is_unspecified() {
        "127.0.0.1".to_owned()
    } else {
        bind_addr.ip().to_string()
    };
    format!("http://{host}:{}", bind_addr.port())
}

async fn serve(bind_addr: SocketAddr, router: Router) -> std::io::Result<()> {
    let listener = tokio::net::TcpListener::bind(bind_addr).await?;
    eprintln!(
        "hivemind node listening on http://{}",
        listener.local_addr()?
    );
    axum::serve(listener, router).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;
    use tower::ServiceExt;

    fn test_state() -> AppState {
        AppState {
            key: Arc::new(NodeKey::from_seed_hex(&"01".repeat(32)).unwrap()),
            node_url: "http://127.0.0.1:7747".to_owned(),
            store: Arc::new(Store::default()),
        }
    }

    #[test]
    fn parses_beacon() {
        let node_id = "a".repeat(64);
        assert_eq!(
            parse_beacon(&format!("{DISCOVERY_PREFIX}http://127.0.0.1:1 {node_id}")),
            Some(PeerInfo {
                node_url: "http://127.0.0.1:1".to_owned(),
                node_id,
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
    fn trusted_peers_excludes_untrusted_candidates() {
        let state = test_state();
        remember_peer(
            &state,
            PeerInfo {
                node_url: "http://untrusted".to_owned(),
                node_id: "b".repeat(64),
            },
            "test",
        );
        remember_peer(
            &state,
            PeerInfo {
                node_url: "http://trusted".to_owned(),
                node_id: "c".repeat(64),
            },
            "test",
        );
        state
            .store
            .peers
            .lock()
            .unwrap()
            .get_mut(&"c".repeat(64))
            .unwrap()
            .trusted = true;
        let peers = trusted_peers(&state);
        assert_eq!(peers.len(), 1);
        assert_eq!(peers[0].node_url, "http://trusted");
    }

    #[test]
    fn remember_peer_does_not_trust() {
        let state = test_state();
        remember_peer(
            &state,
            PeerInfo {
                node_url: "http://peer".to_owned(),
                node_id: "b".repeat(64),
            },
            "test",
        );
        let peers = state.store.peers.lock().unwrap();
        assert!(!peers.values().next().unwrap().trusted);
    }

    #[tokio::test]
    async fn add_peer_stores_untrusted_candidate() {
        let state = test_state();
        let response = app(state)
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/v1/peers")
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
        assert!(!peer.trusted);
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
            .any(|peer| peer.node_id == "b".repeat(64) && !peer.trusted));
    }

    #[tokio::test]
    async fn trust_marks_existing_peer_only() {
        let state = test_state();
        remember_peer(
            &state,
            PeerInfo {
                node_url: "http://peer".to_owned(),
                node_id: "b".repeat(64),
            },
            "test",
        );
        let response = app(state)
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri(format!("/v1/peers/{}/trust", "b".repeat(64)))
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let peer: PeerRecord = serde_json::from_slice(&bytes).unwrap();
        assert!(peer.trusted);
    }

    #[tokio::test]
    async fn import_rejects_untrusted_author_and_stores_notice() {
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

        let messages = state.store.messages.lock().unwrap();
        assert_eq!(messages.len(), 1);
        let notice = messages.values().next().unwrap();
        assert!(notice.text.contains("tried to send a chat message"));
        assert!(notice.text.contains(&author.node_id()));
        assert!(!notice.text.contains("secret text should not be copied"));
        notice.verify().unwrap();
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
            },
            "test",
        );
        state
            .store
            .peers
            .lock()
            .unwrap()
            .get_mut(&author.node_id())
            .unwrap()
            .trusted = true;
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
    async fn say_stores_signed_message() {
        let state = test_state();
        let response = app(state)
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/v1/chat")
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
