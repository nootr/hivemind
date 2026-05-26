#[tokio::main]
async fn main() {
    if let Err(err) = hivemind_cli::run_from_env().await {
        eprintln!("error: {err}");
        std::process::exit(1);
    }
}
