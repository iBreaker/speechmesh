use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Result, anyhow};
use clap::{Parser, ValueEnum};
use speechmeshd::agent::{AgentRegistry, RemoteAgentAsrBridge, RemoteAgentAsrBridgeConfig};
use speechmeshd::bridge::{
    MockAsrBridge, SharedAsrBridge, StdioAsrBridge, StdioAsrBridgeConfig, TcpAsrBridge,
    TcpAsrBridgeConfig,
};
use speechmeshd::server::{ServerConfig, run_server};
use tracing_subscriber::EnvFilter;

#[derive(Debug, Clone, Copy, ValueEnum)]
enum AsrBridgeMode {
    Mock,
    Stdio,
    Tcp,
    Agent,
}

#[derive(Debug, Parser)]
#[command(name = "speechmeshd")]
#[command(about = "SpeechMesh WebSocket daemon")]
struct Args {
    #[arg(long, default_value = "127.0.0.1:8765")]
    listen: SocketAddr,
    #[arg(long, default_value = "v1")]
    protocol_version: String,
    #[arg(long, default_value = "speechmeshd")]
    server_name: String,
    #[arg(long, value_enum, default_value_t = AsrBridgeMode::Mock)]
    asr_bridge_mode: AsrBridgeMode,
    #[arg(long, default_value = "bridge.asr")]
    asr_provider_id: String,
    #[arg(long)]
    asr_bridge_command: Option<String>,
    #[arg(long)]
    asr_bridge_args: Vec<String>,
    #[arg(long)]
    asr_bridge_address: Option<String>,
    #[arg(long)]
    agent_shared_secret: Option<String>,
    #[arg(long, default_value_t = 10)]
    agent_start_timeout_secs: u64,
}

#[tokio::main]
async fn main() -> Result<()> {
    init_tracing();
    let args = Args::parse();
    let selection = select_asr_bridge(&args)?;
    let config = ServerConfig {
        listen: args.listen,
        protocol_version: args.protocol_version,
        server_name: args.server_name,
    };
    run_server(config, selection.bridge, selection.agent_registry).await
}

fn init_tracing() {
    let env_filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("speechmeshd=info"));
    tracing_subscriber::fmt()
        .with_env_filter(env_filter)
        .compact()
        .init();
}

struct BridgeSelection {
    bridge: SharedAsrBridge,
    agent_registry: Option<AgentRegistry>,
}

fn select_asr_bridge(args: &Args) -> Result<BridgeSelection> {
    let mut agent_registry = None;
    let bridge: SharedAsrBridge = match args.asr_bridge_mode {
        AsrBridgeMode::Mock => Arc::new(MockAsrBridge::new(args.asr_provider_id.clone())),
        AsrBridgeMode::Stdio => {
            let command = args
                .asr_bridge_command
                .clone()
                .ok_or_else(|| anyhow!("--asr-bridge-command is required for stdio mode"))?;
            Arc::new(StdioAsrBridge::new(StdioAsrBridgeConfig {
                provider_id: args.asr_provider_id.clone(),
                command,
                args: args.asr_bridge_args.clone(),
            }))
        }
        AsrBridgeMode::Tcp => {
            let address = args
                .asr_bridge_address
                .clone()
                .ok_or_else(|| anyhow!("--asr-bridge-address is required for tcp mode"))?;
            Arc::new(TcpAsrBridge::new(TcpAsrBridgeConfig {
                provider_id: args.asr_provider_id.clone(),
                address,
            }))
        }
        AsrBridgeMode::Agent => {
            let registry = AgentRegistry::new(args.agent_shared_secret.clone());
            agent_registry = Some(registry.clone());
            Arc::new(RemoteAgentAsrBridge::new(
                RemoteAgentAsrBridgeConfig {
                    provider_id: args.asr_provider_id.clone(),
                    start_timeout: Duration::from_secs(args.agent_start_timeout_secs),
                },
                registry,
            ))
        }
    };
    Ok(BridgeSelection {
        bridge,
        agent_registry,
    })
}
