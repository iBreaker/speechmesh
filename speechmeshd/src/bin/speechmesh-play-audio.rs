use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use clap::{Parser, ValueEnum};
use futures_util::{SinkExt, StreamExt};
use speechmesh_core::{AudioEncoding, AudioFormat};
use speechmesh_transport::{ControlPlayAudioPayload, ControlRequest, ControlResponse};
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::protocol::Message;

const AFTER_HELP: &str = "\
Examples:
  speechmesh-play-audio --file /tmp/test.mp3 --device-id mac01
  speechmesh-play-audio --url ws://127.0.0.1:8765/control --file /tmp/test.wav --format wav --sample-rate 24000 --channels 1

Notes:
  - This command sends one play_audio control request to speechmeshd /control.
  - The server routes audio to a matching speaker agent.";

#[derive(Debug, Parser)]
#[command(name = "speechmesh-play-audio")]
#[command(about = "Send a minimal play_audio control task to speechmeshd")]
#[command(after_help = AFTER_HELP)]
struct Cli {
    #[arg(
        long,
        default_value = "ws://127.0.0.1:8765/control",
        help = "speechmeshd control websocket endpoint"
    )]
    url: String,
    #[arg(long, value_name = "PATH", help = "Audio file to send")]
    file: PathBuf,
    #[arg(long, help = "Optional explicit task id")]
    task_id: Option<String>,
    #[arg(long, help = "Optional target device id (for example: mac01)")]
    device_id: Option<String>,
    #[arg(long, help = "Optional target agent id")]
    agent_id: Option<String>,
    #[arg(long, default_value_t = 16 * 1024, help = "Chunk size hint consumed by the /control endpoint")]
    chunk_size_bytes: usize,
    #[arg(
        long,
        value_enum,
        help = "Optional output format hint for the target agent"
    )]
    format: Option<AudioEncodingArg>,
    #[arg(long, default_value_t = 32000, help = "Format hint sample rate")]
    sample_rate: u32,
    #[arg(long, default_value_t = 1, help = "Format hint channel count")]
    channels: u16,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum AudioEncodingArg {
    Mp3,
    Wav,
    Flac,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let bytes = std::fs::read(&cli.file)
        .with_context(|| format!("failed to read audio file {}", cli.file.display()))?;
    if bytes.is_empty() {
        bail!("audio file is empty");
    }

    let format = cli.format.map(|encoding| AudioFormat {
        encoding: match encoding {
            AudioEncodingArg::Mp3 => AudioEncoding::Mp3,
            AudioEncodingArg::Wav => AudioEncoding::Wav,
            AudioEncodingArg::Flac => AudioEncoding::Flac,
        },
        sample_rate_hz: cli.sample_rate,
        channels: cli.channels,
    });

    let message = ControlRequest::PlayAudio {
        payload: ControlPlayAudioPayload {
            task_id: cli.task_id,
            device_id: cli.device_id,
            agent_id: cli.agent_id,
            format,
            audio_base64: BASE64_STANDARD.encode(bytes),
            chunk_size_bytes: Some(cli.chunk_size_bytes.max(1)),
        },
    };
    let encoded = serde_json::to_string(&message).context("failed to encode control payload")?;

    let (mut websocket, response) = connect_async(&cli.url)
        .await
        .with_context(|| format!("failed to connect to {}", cli.url))?;
    if response.status().as_u16() != 101 {
        bail!(
            "control websocket handshake failed with {}",
            response.status()
        );
    }

    websocket
        .send(Message::Text(encoded.into()))
        .await
        .context("failed to send play_audio control request")?;

    while let Some(frame) = websocket.next().await {
        let frame = frame.context("failed to read /control response frame")?;
        match frame {
            Message::Text(text) => {
                let response: ControlResponse =
                    serde_json::from_str(&text).map_err(|error| {
                        anyhow::anyhow!("unexpected /control response ({error}): {text}")
                    })?;
                match response {
                    ControlResponse::PlayAudioAccepted { payload } => {
                        println!("{}", serde_json::to_string_pretty(&payload)?);
                        return Ok(());
                    }
                    ControlResponse::Error { payload } => {
                        bail!("control request failed: {}", payload.message);
                    }
                    ControlResponse::Pong { .. } => {}
                    _ => {}
                }
            }
            Message::Ping(payload) => {
                websocket
                    .send(Message::Pong(payload))
                    .await
                    .context("failed to send pong")?;
            }
            Message::Close(_) => bail!("control websocket closed before response"),
            Message::Binary(_) | Message::Pong(_) | Message::Frame(_) => {}
        }
    }

    bail!("control websocket closed without response")
}
