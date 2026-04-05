use std::net::SocketAddr;
use std::process::Stdio;

use anyhow::{Context, Result};
use clap::Parser;
use tokio::io::AsyncWriteExt;
use tokio::net::{TcpListener, TcpStream};
use tokio::process::Command;
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;

#[derive(Debug, Clone, Parser)]
#[command(name = "speechmesh-bridge-tcpd")]
#[command(about = "TCP proxy that exposes a line-based ASR bridge over the network")]
struct Args {
    #[arg(long, default_value = "0.0.0.0:9654")]
    listen: SocketAddr,
    #[arg(long)]
    bridge_command: String,
    #[arg(long)]
    bridge_args: Vec<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    init_tracing();
    let args = Args::parse();
    let listener = TcpListener::bind(args.listen)
        .await
        .with_context(|| format!("failed to bind tcp listener at {}", args.listen))?;
    info!("speechmesh-bridge-tcpd listening on {}", args.listen);

    loop {
        let (stream, peer) = listener.accept().await.context("tcp accept failed")?;
        let config = args.clone();
        tokio::spawn(async move {
            if let Err(error) = handle_connection(stream, peer, config).await {
                warn!("bridge tcp connection {peer} failed: {error:#}");
            }
        });
    }
}

fn init_tracing() {
    let env_filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("speechmesh_bridge_tcpd=info"));
    tracing_subscriber::fmt()
        .with_env_filter(env_filter)
        .compact()
        .init();
}

async fn handle_connection(stream: TcpStream, peer: SocketAddr, args: Args) -> Result<()> {
    info!("accepted bridge tcp connection from {peer}");
    let mut child = Command::new(&args.bridge_command)
        .args(&args.bridge_args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .with_context(|| format!("failed to spawn bridge process {}", args.bridge_command))?;

    let mut child_stdin = child
        .stdin
        .take()
        .context("bridge child stdin unavailable")?;
    let mut child_stdout = child
        .stdout
        .take()
        .context("bridge child stdout unavailable")?;

    let (mut socket_read, mut socket_write) = stream.into_split();
    let client_to_bridge = tokio::spawn(async move {
        tokio::io::copy(&mut socket_read, &mut child_stdin)
            .await
            .context("copy client->bridge failed")?;
        child_stdin
            .shutdown()
            .await
            .context("shutdown bridge stdin failed")?;
        Result::<(), anyhow::Error>::Ok(())
    });
    let bridge_to_client = tokio::spawn(async move {
        tokio::io::copy(&mut child_stdout, &mut socket_write)
            .await
            .context("copy bridge->client failed")?;
        socket_write
            .shutdown()
            .await
            .context("shutdown socket write failed")?;
        Result::<(), anyhow::Error>::Ok(())
    });

    let client_result = client_to_bridge
        .await
        .context("join client->bridge task failed")?;
    let bridge_result = bridge_to_client
        .await
        .context("join bridge->client task failed")?;
    let status = child.wait().await.context("wait bridge child failed")?;

    if let Err(error) = client_result {
        warn!("bridge tcp {peer}: {error:#}");
    }
    if let Err(error) = bridge_result {
        warn!("bridge tcp {peer}: {error:#}");
    }

    info!("bridge tcp connection from {peer} closed with child status {status}");
    Ok(())
}
