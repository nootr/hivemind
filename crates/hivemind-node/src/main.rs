use clap::Parser;
use hivemind_node::{run, NodeConfig};
use std::{net::SocketAddr, path::PathBuf};

#[derive(Debug, Parser)]
struct Args {
    #[arg(long)]
    config: Option<PathBuf>,
    #[arg(long, default_value = "./data")]
    data_dir: PathBuf,
    #[arg(long, default_value = "0.0.0.0:7747")]
    bind_addr: SocketAddr,
    #[arg(long)]
    public_url: Option<String>,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();
    let config = if let Some(path) = args.config {
        NodeConfig::from_file(path)?
    } else {
        NodeConfig {
            data_dir: args.data_dir,
            bind_addr: args.bind_addr,
            public_url: args.public_url,
        }
    };
    run(config).await?;
    Ok(())
}
