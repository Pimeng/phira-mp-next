//! phira-mp 服务端入口。

use clap::Parser;
use phira_mp::server::{run, ServerArgs};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> std::io::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .init();

    let args = ServerArgs::parse();
    run(args).await
}
