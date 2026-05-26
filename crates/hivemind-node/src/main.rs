use hivemind_adapters::{fs::FsContentStore, sqlite::SqliteMetadataStore};
use hivemind_node::{
    app, load_or_create_token, ApiConfig, AppState, FileIdentity, NodeConfig, SqliteNodeStateStore,
    SystemClock,
};
use std::{
    env,
    net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket as StdUdpSocket},
    path::PathBuf,
    sync::Arc,
};

const DISCOVERY_PORT: u16 = 7748;
const DISCOVERY_QUERY: &[u8] = b"HIVEMIND_DISCOVER_V1";
const DISCOVERY_RESPONSE_PREFIX: &str = "HIVEMIND_NODE_V1 ";

#[derive(Debug, thiserror::Error)]
enum MainError {
    #[error("usage: hivemind-node --config <path>")]
    Usage,

    #[error("config error: {0}")]
    Config(#[from] hivemind_node::ConfigError),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("token error: {0}")]
    Token(#[from] hivemind_node::TokenError),

    #[error("identity error: {0}")]
    Identity(#[from] hivemind_node::FileIdentityError),

    #[error("sqlite error: {0}")]
    Sqlite(#[from] hivemind_adapters::sqlite::SqliteStoreError),

    #[error("state store error: {0}")]
    StateStore(#[from] hivemind_node::NodeStateStoreError),
}

#[tokio::main]
async fn main() -> Result<(), MainError> {
    let config_path = parse_config_path(env::args().skip(1))?;
    let config = NodeConfig::from_file(config_path)?;
    run(config).await
}

async fn run(config: NodeConfig) -> Result<(), MainError> {
    std::fs::create_dir_all(&config.data.dir)?;
    let token = load_or_create_token(&config.api.auth_token_file)?;
    let identity = FileIdentity::load_or_create(&config.identity.agent_key_path)?;
    let node_id = identity.agent_id().to_string();
    let content_store = FsContentStore::new(&config.data.dir);
    let metadata_store = SqliteMetadataStore::open(config.data.dir.join("metadata.sqlite3"))?;
    let state_store = Arc::new(SqliteNodeStateStore::open(
        config.data.dir.join("state.sqlite3"),
    )?);
    let bind_addr = config.api.bind_addr;
    let public_url = config.api.public_url.clone();

    let state = AppState {
        identity: Arc::new(identity),
        clock: Arc::new(SystemClock),
        content_store: Arc::new(content_store),
        metadata_store: Arc::new(metadata_store),
        config: ApiConfig {
            admin_token: token,
            state_store,
        },
    };

    spawn_discovery_responder(bind_addr, public_url, node_id);
    serve(bind_addr, app(state)).await
}

fn spawn_discovery_responder(
    api_bind_addr: SocketAddr,
    public_url: Option<String>,
    node_id: String,
) {
    tokio::spawn(async move {
        if let Err(err) = discovery_responder(api_bind_addr, public_url, node_id).await {
            eprintln!("hivemind discovery disabled: {err}");
        }
    });
}

async fn discovery_responder(
    api_bind_addr: SocketAddr,
    public_url: Option<String>,
    node_id: String,
) -> Result<(), std::io::Error> {
    let socket = tokio::net::UdpSocket::bind((Ipv4Addr::UNSPECIFIED, DISCOVERY_PORT)).await?;
    eprintln!("hivemind discovery listening on udp://0.0.0.0:{DISCOVERY_PORT}");
    let mut buf = [0_u8; 1024];

    loop {
        let (len, peer) = socket.recv_from(&mut buf).await?;
        if &buf[..len] != DISCOVERY_QUERY {
            continue;
        }

        let node_url = public_url
            .clone()
            .unwrap_or_else(|| inferred_node_url(api_bind_addr, peer));
        let response = format!("{DISCOVERY_RESPONSE_PREFIX}{node_url} {node_id}");
        let _ = socket.send_to(response.as_bytes(), peer).await;
    }
}

fn inferred_node_url(api_bind_addr: SocketAddr, peer: SocketAddr) -> String {
    let port = api_bind_addr.port();
    if api_bind_addr.ip().is_loopback() {
        return format!("http://127.0.0.1:{port}");
    }

    let local_ip = outbound_ip_for_peer(peer).unwrap_or(api_bind_addr.ip());
    let host = match local_ip {
        IpAddr::V4(ip) => ip.to_string(),
        IpAddr::V6(ip) => format!("[{ip}]"),
    };
    format!("http://{host}:{port}")
}

fn outbound_ip_for_peer(peer: SocketAddr) -> Option<IpAddr> {
    let bind_addr = match peer {
        SocketAddr::V4(_) => SocketAddr::from((Ipv4Addr::UNSPECIFIED, 0)),
        SocketAddr::V6(_) => "[::]:0".parse().ok()?,
    };
    let socket = StdUdpSocket::bind(bind_addr).ok()?;
    socket.connect(peer).ok()?;
    Some(socket.local_addr().ok()?.ip())
}

async fn serve(bind_addr: SocketAddr, router: axum::Router) -> Result<(), MainError> {
    let listener = tokio::net::TcpListener::bind(bind_addr).await?;
    let local_addr = listener.local_addr()?;
    eprintln!("hivemind-node listening on http://{local_addr}");
    axum::serve(listener, router).await?;
    Ok(())
}

fn parse_config_path(mut args: impl Iterator<Item = String>) -> Result<PathBuf, MainError> {
    match (args.next().as_deref(), args.next(), args.next()) {
        (Some("--config"), Some(path), None) => Ok(PathBuf::from(path)),
        _ => Err(MainError::Usage),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn infers_loopback_discovery_url() {
        let api_bind_addr = "127.0.0.1:7747".parse().unwrap();
        let peer = "127.0.0.1:50000".parse().unwrap();

        assert_eq!(
            inferred_node_url(api_bind_addr, peer),
            "http://127.0.0.1:7747"
        );
    }

    #[test]
    fn parses_config_arg() {
        let path =
            parse_config_path(["--config".to_owned(), "node.toml".to_owned()].into_iter()).unwrap();
        assert_eq!(path, PathBuf::from("node.toml"));
    }

    #[test]
    fn rejects_missing_config_arg() {
        assert!(matches!(
            parse_config_path(std::iter::empty()),
            Err(MainError::Usage)
        ));
    }
}
