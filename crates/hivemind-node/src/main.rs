use hivemind_adapters::{fs::FsContentStore, sqlite::SqliteMetadataStore};
use hivemind_node::{
    app, load_or_create_token, ApiConfig, AppState, FileIdentity, NodeConfig, SystemClock,
};
use std::{env, net::SocketAddr, path::PathBuf, sync::Arc};

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
    let content_store = FsContentStore::new(&config.data.dir);
    let metadata_store = SqliteMetadataStore::open(config.data.dir.join("metadata.sqlite3"))?;
    let bind_addr = config.api.bind_addr;

    let state = AppState {
        identity: Arc::new(identity),
        clock: Arc::new(SystemClock),
        content_store: Arc::new(content_store),
        metadata_store: Arc::new(metadata_store),
        config: ApiConfig {
            bearer_token: token,
        },
    };

    serve(bind_addr, app(state)).await
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
