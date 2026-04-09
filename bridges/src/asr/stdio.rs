use std::process::Stdio;

use async_trait::async_trait;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use serde::Serialize;
use serde_json::Value;
use speechmesh_asr::{AsrSession, StreamRequest};
use speechmesh_core::{
    Capability, CapabilityDomain, ProviderDescriptor, RuntimeMode, SessionId, StreamMode,
};
use tokio::io::{AsyncBufReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::mpsc;
use tracing::{debug, warn};

use super::{
    AsrBridge, BridgeAsrEvent, BridgeAsrSessionHandle, BridgeAudioFrame, BridgeAudioPayload,
    BridgeCommand, BridgeEmptyFrame, BridgeEmptyPayload, BridgeStartFrame, BridgeStartPayload,
    asr_descriptor_with_io_modes, extract_final_transcript, requested_asr_input_mode,
    requested_asr_output_mode,
};
use crate::BridgeError;

#[derive(Debug, Clone)]
pub struct StdioAsrBridgeConfig {
    pub provider_id: String,
    pub display_name: Option<String>,
    pub command: String,
    pub args: Vec<String>,
}

pub struct StdioAsrBridge {
    config: StdioAsrBridgeConfig,
}

impl StdioAsrBridge {
    pub fn new(config: StdioAsrBridgeConfig) -> Self {
        Self { config }
    }
}

#[async_trait]
impl AsrBridge for StdioAsrBridge {
    fn descriptors(&self) -> Vec<ProviderDescriptor> {
        vec![
            asr_descriptor_with_io_modes(
                ProviderDescriptor::new(
                    self.config.provider_id.clone(),
                    self.config
                        .display_name
                        .clone()
                        .unwrap_or_else(|| "Stdio ASR Bridge".to_string()),
                    CapabilityDomain::Asr,
                    RuntimeMode::LocalDaemon,
                ),
                true,
            )
            .with_capability(Capability::enabled("bridge-stdio")),
        ]
    }

    async fn start_stream(
        &self,
        request: StreamRequest,
    ) -> Result<BridgeAsrSessionHandle, BridgeError> {
        let mut child = Command::new(&self.config.command)
            .args(&self.config.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|e| {
                BridgeError::Unavailable(format!(
                    "failed to spawn bridge process {}: {e}",
                    self.config.command
                ))
            })?;

        let child_stdin = child
            .stdin
            .take()
            .ok_or_else(|| BridgeError::Io("bridge stdin unavailable".to_string()))?;
        let child_stdout = child
            .stdout
            .take()
            .ok_or_else(|| BridgeError::Io("bridge stdout unavailable".to_string()))?;

        let session_id = SessionId::new();
        let session = AsrSession {
            id: session_id,
            provider_id: self.config.provider_id.clone(),
            accepted_input_format: request.input_format.clone(),
            input_mode: requested_asr_input_mode(&request),
            output_mode: requested_asr_output_mode(&request),
        };

        let (command_tx, command_rx) = mpsc::channel::<BridgeCommand>(64);
        let (event_tx, event_rx) = mpsc::channel::<BridgeAsrEvent>(64);
        tokio::spawn(run_bridge_writer(
            session.id.clone(),
            request,
            child_stdin,
            command_rx,
        ));
        tokio::spawn(run_bridge_reader(child_stdout, event_tx));
        tokio::spawn(wait_child_exit(child));

        Ok(BridgeAsrSessionHandle::new(session, command_tx, event_rx))
    }
}

// ---- stdio/tcp 共用的 writer/reader 逻辑 ----

pub(crate) async fn run_bridge_writer<W>(
    session_id: SessionId,
    start_request: StreamRequest,
    mut writer: W,
    mut command_rx: mpsc::Receiver<BridgeCommand>,
) where
    W: AsyncWrite + Unpin,
{
    let start_frame = BridgeStartFrame {
        type_name: "asr.start",
        request_id: Option::<String>::None,
        session_id: Some(session_id.clone()),
        payload: BridgeStartPayload {
            locale: start_request.options.language.clone(),
            should_report_partials: matches!(
                requested_asr_output_mode(&start_request),
                StreamMode::Streaming
            ),
            requires_on_device: start_request.options.prefer_on_device,
            input_format: start_request.input_format,
        },
    };
    if write_frame(&mut writer, &start_frame).await.is_err() {
        return;
    }

    while let Some(command) = command_rx.recv().await {
        let encoded = match command {
            BridgeCommand::PushAudio(chunk) => serde_json::to_string(&BridgeAudioFrame {
                type_name: "asr.audio",
                request_id: Option::<String>::None,
                session_id: Some(session_id.clone()),
                payload: BridgeAudioPayload {
                    data_base64: BASE64_STANDARD.encode(chunk),
                },
            }),
            BridgeCommand::Commit => serde_json::to_string(&BridgeEmptyFrame {
                type_name: "asr.commit",
                request_id: Option::<String>::None,
                session_id: Some(session_id.clone()),
                payload: BridgeEmptyPayload {},
            }),
            BridgeCommand::Stop => serde_json::to_string(&BridgeEmptyFrame {
                type_name: "asr.stop",
                request_id: Option::<String>::None,
                session_id: Some(session_id.clone()),
                payload: BridgeEmptyPayload {},
            }),
        };
        let Ok(encoded) = encoded else {
            break;
        };
        if write_encoded_frame(&mut writer, &encoded).await.is_err() {
            break;
        }
    }
}

pub(crate) async fn run_bridge_reader<R>(
    reader_source: R,
    event_tx: mpsc::Sender<BridgeAsrEvent>,
) where
    R: tokio::io::AsyncRead + Unpin,
{
    let mut reader = BufReader::new(reader_source).lines();
    loop {
        match reader.next_line().await {
            Ok(Some(line)) => match serde_json::from_str::<Value>(&line) {
                Ok(frame) => {
                    let Some(frame_type) = frame.get("type").and_then(Value::as_str) else {
                        warn!("bridge frame missing type: {line}");
                        continue;
                    };
                    let payload = frame.get("payload").cloned().unwrap_or(Value::Null);
                    let mapped = match frame_type {
                        "asr.partial" => payload.get("text").and_then(Value::as_str).map(|text| {
                            BridgeAsrEvent::Partial {
                                text: text.to_string(),
                            }
                        }),
                        "asr.final" => extract_final_transcript(&payload)
                            .map(|transcript| BridgeAsrEvent::Final { transcript }),
                        "asr.ended" => Some(BridgeAsrEvent::Ended {
                            reason: payload
                                .get("reason")
                                .and_then(Value::as_str)
                                .map(ToString::to_string),
                        }),
                        "error" => Some(BridgeAsrEvent::Error {
                            message: payload
                                .get("message")
                                .and_then(Value::as_str)
                                .unwrap_or("bridge error")
                                .to_string(),
                        }),
                        "bridge.ready" | "hello.ok" | "auth.result" | "asr.started"
                        | "asr.committed" | "pong" | "shutdown.ok" => None,
                        other => {
                            warn!("ignoring unknown bridge frame type: {other}");
                            None
                        }
                    };
                    if let Some(mapped) = mapped {
                        if event_tx.send(mapped).await.is_err() {
                            break;
                        }
                    }
                }
                Err(error) => {
                    warn!("failed to parse bridge line as json: {error}");
                }
            },
            Ok(None) => break,
            Err(error) => {
                debug!("bridge stdout read failed: {error}");
                break;
            }
        }
    }
}

async fn write_frame<T, W>(writer: &mut W, frame: &T) -> Result<(), BridgeError>
where
    T: Serialize,
    W: AsyncWrite + Unpin,
{
    let encoded = serde_json::to_string(frame)
        .map_err(|e| BridgeError::Protocol(format!("serialize outbound frame failed: {e}")))?;
    write_encoded_frame(writer, &encoded).await
}

async fn write_encoded_frame<W>(writer: &mut W, encoded: &str) -> Result<(), BridgeError>
where
    W: AsyncWrite + Unpin,
{
    writer
        .write_all(encoded.as_bytes())
        .await
        .map_err(|e| BridgeError::Io(format!("write frame failed: {e}")))?;
    writer
        .write_all(b"\n")
        .await
        .map_err(|e| BridgeError::Io(format!("write frame newline failed: {e}")))?;
    writer
        .flush()
        .await
        .map_err(|e| BridgeError::Io(format!("flush frame failed: {e}")))?;
    Ok(())
}

async fn wait_child_exit(mut child: Child) {
    match child.wait().await {
        Ok(status) => debug!("bridge process exited with status {status}"),
        Err(error) => warn!("bridge process wait failed: {error}"),
    }
}
