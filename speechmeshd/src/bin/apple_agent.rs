use std::time::Duration;

use anyhow::Result;
use clap::Parser;
use speechmeshd::agent::{LocalAgentConfig, run_local_agent};
use tracing_subscriber::EnvFilter;

#[derive(Debug, Parser)]
#[command(name = "speechmesh-apple-agent")]
#[command(about = "SpeechMesh macOS Apple Speech agent")]
struct Args {
    #[arg(long, default_value = "ws://127.0.0.1:8765/agent")]
    gateway_url: String,
    #[arg(long, default_value = "apple-agent-1")]
    agent_id: String,
    #[arg(long, default_value = "macOS Apple ASR Agent")]
    agent_name: String,
    #[arg(long, default_value = "apple.asr")]
    provider_id: String,
    #[arg(long)]
    shared_secret: Option<String>,
    #[arg(long)]
    bridge_command: String,
    #[arg(long)]
    bridge_args: Vec<String>,
    #[arg(long, default_value_t = 2)]
    reconnect_delay_secs: u64,
}

#[tokio::main]
async fn main() -> Result<()> {
    init_rustls();
    init_tracing();
    let args = Args::parse();
    run_local_agent(LocalAgentConfig {
        gateway_url: args.gateway_url,
        agent_id: args.agent_id,
        agent_name: args.agent_name,
        provider_id: args.provider_id,
        shared_secret: args.shared_secret,
        bridge_command: args.bridge_command,
        bridge_args: args.bridge_args,
        reconnect_delay: Duration::from_secs(args.reconnect_delay_secs),
    })
    .await
    .map_err(anyhow::Error::from)
}

fn init_tracing() {
    let env_filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("speechmeshd::agent=info,speechmesh_apple_agent=info"));
    tracing_subscriber::fmt()
        .with_env_filter(env_filter)
        .compact()
        .init();
}

fn init_rustls() {
    let _ = rustls::crypto::ring::default_provider().install_default();
}
