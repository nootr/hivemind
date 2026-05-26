#[tokio::main]
async fn main() -> Result<(), hivemind_cli::CliError> {
    hivemind_cli::run().await
}
