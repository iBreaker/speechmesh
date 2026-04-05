use std::process::Stdio;
use std::sync::Arc;

use async_trait::async_trait;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use serde::Serialize;
use serde_json::Value;
use speechmesh_asr::{AsrSession, StreamRequest, Transcript, TranscriptSegment};
use speechmesh_core::{Capability, CapabilityDomain, ProviderDescriptor, RuntimeMode, SessionId};
use thiserror::Error;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio::process::{Child, Command};
use tokio::sync::mpsc;
use tracing::{debug, warn};

#[derive(Debug, Clone, PartialEq)]
pub enum BridgeAsrEvent {
    Partial { text: String },
    Final { transcript: Transcript },
    Ended { reason: Option<String> },
    Error { message: String },
}

#[derive(Debug, Clone)]
pub(crate) enum BridgeCommand {
    PushAudio(Vec<u8>),
    Commit,
    Stop,
}

#[derive(Debug, Clone)]
pub struct BridgeAsrSessionController {
    command_tx: mpsc::Sender<BridgeCommand>,
}

impl BridgeAsrSessionController {
    pub async fn push_audio(&self, chunk: Vec<u8>) -> Result<(), BridgeError> {
        self.command_tx
            .send(BridgeCommand::PushAudio(chunk))
            .await
            .map_err(|_| BridgeError::Disconnected("bridge command channel closed".to_string()))
    }

    pub async fn commit(&self) -> Result<(), BridgeError> {
        self.command_tx
            .send(BridgeCommand::Commit)
            .await
            .map_err(|_| BridgeError::Disconnected("bridge command channel closed".to_string()))
    }

    pub async fn stop(&self) -> Result<(), BridgeError> {
        self.command_tx
            .send(BridgeCommand::Stop)
            .await
            .map_err(|_| BridgeError::Disconnected("bridge command channel closed".to_string()))
    }
}

#[derive(Debug)]
pub struct BridgeAsrSessionHandle {
    pub session: AsrSession,
    command_tx: mpsc::Sender<BridgeCommand>,
    event_rx: Option<mpsc::Receiver<BridgeAsrEvent>>,
}

impl BridgeAsrSessionHandle {
    pub(crate) fn new(
        session: AsrSession,
        command_tx: mpsc::Sender<BridgeCommand>,
        event_rx: mpsc::Receiver<BridgeAsrEvent>,
    ) -> Self {
        Self {
            session,
            command_tx,
            event_rx: Some(event_rx),
        }
    }

    pub async fn push_audio(&self, chunk: Vec<u8>) -> Result<(), BridgeError> {
        self.command_tx
            .send(BridgeCommand::PushAudio(chunk))
            .await
            .map_err(|_| BridgeError::Disconnected("bridge command channel closed".to_string()))
    }

    pub async fn commit(&self) -> Result<(), BridgeError> {
        self.command_tx
            .send(BridgeCommand::Commit)
            .await
            .map_err(|_| BridgeError::Disconnected("bridge command channel closed".to_string()))
    }

    pub async fn stop(&self) -> Result<(), BridgeError> {
        self.command_tx
            .send(BridgeCommand::Stop)
            .await
            .map_err(|_| BridgeError::Disconnected("bridge command channel closed".to_string()))
    }

    pub fn take_event_rx(&mut self) -> Option<mpsc::Receiver<BridgeAsrEvent>> {
        self.event_rx.take()
    }

    pub fn controller(&self) -> BridgeAsrSessionController {
        BridgeAsrSessionController {
            command_tx: self.command_tx.clone(),
        }
    }
}

#[derive(Debug, Error)]
pub enum BridgeError {
    #[error("bridge unavailable: {0}")]
    Unavailable(String),
    #[error("bridge disconnected: {0}")]
    Disconnected(String),
    #[error("bridge protocol error: {0}")]
    Protocol(String),
    #[error("bridge io error: {0}")]
    Io(String),
}

#[async_trait]
pub trait AsrBridge: Send + Sync {
    fn descriptors(&self) -> Vec<ProviderDescriptor>;

    async fn start_stream(
        &self,
        request: StreamRequest,
    ) -> Result<BridgeAsrSessionHandle, BridgeError>;
}

pub type SharedAsrBridge = Arc<dyn AsrBridge>;

pub struct MockAsrBridge {
    provider_id: String,
}

impl MockAsrBridge {
    pub fn new(provider_id: impl Into<String>) -> Self {
        Self {
            provider_id: provider_id.into(),
        }
    }
}

#[async_trait]
impl AsrBridge for MockAsrBridge {
    fn descriptors(&self) -> Vec<ProviderDescriptor> {
        vec![
            ProviderDescriptor::new(
                self.provider_id.clone(),
                "Mock ASR Bridge",
                CapabilityDomain::Asr,
                RuntimeMode::LocalDaemon,
            )
            .with_capability(Capability::enabled("streaming-input"))
            .with_capability(Capability::enabled("interim-results")),
        ]
    }

    async fn start_stream(
        &self,
        request: StreamRequest,
    ) -> Result<BridgeAsrSessionHandle, BridgeError> {
        let (command_tx, mut command_rx) = mpsc::channel::<BridgeCommand>(64);
        let (event_tx, event_rx) = mpsc::channel::<BridgeAsrEvent>(64);
        let provider_id = self.provider_id.clone();
        let session_id = SessionId::new();
        let session = AsrSession {
            id: session_id,
            provider_id: provider_id.clone(),
            accepted_input_format: request.input_format,
        };

        tokio::spawn(async move {
            let mut buffered_bytes: usize = 0;
            while let Some(command) = command_rx.recv().await {
                match command {
                    BridgeCommand::PushAudio(chunk) => {
                        buffered_bytes += chunk.len();
                        let text = format!("mock partial bytes={buffered_bytes}");
                        if event_tx
                            .send(BridgeAsrEvent::Partial { text })
                            .await
                            .is_err()
                        {
                            return;
                        }
                    }
                    BridgeCommand::Commit => {
                        let transcript = Transcript {
                            text: format!("mock transcript bytes={buffered_bytes}"),
                            language: Some("en-US".to_string()),
                            segments: Vec::new(),
                        };
                        if event_tx
                            .send(BridgeAsrEvent::Final { transcript })
                            .await
                            .is_err()
                        {
                            return;
                        }
                        let _ = event_tx.send(BridgeAsrEvent::Ended { reason: None }).await;
                        return;
                    }
                    BridgeCommand::Stop => {
                        let _ = event_tx
                            .send(BridgeAsrEvent::Ended {
                                reason: Some("stopped".to_string()),
                            })
                            .await;
                        return;
                    }
                }
            }
        });

        Ok(BridgeAsrSessionHandle::new(session, command_tx, event_rx))
    }
}

#[derive(Debug, Clone)]
pub struct StdioAsrBridgeConfig {
    pub provider_id: String,
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
            ProviderDescriptor::new(
                self.config.provider_id.clone(),
                "Stdio ASR Bridge",
                CapabilityDomain::Asr,
                RuntimeMode::LocalDaemon,
            )
            .with_capability(Capability::enabled("streaming-input"))
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

#[derive(Debug, Clone)]
pub struct TcpAsrBridgeConfig {
    pub provider_id: String,
    pub address: String,
}

pub struct TcpAsrBridge {
    config: TcpAsrBridgeConfig,
}

impl TcpAsrBridge {
    pub fn new(config: TcpAsrBridgeConfig) -> Self {
        Self { config }
    }
}

#[async_trait]
impl AsrBridge for TcpAsrBridge {
    fn descriptors(&self) -> Vec<ProviderDescriptor> {
        vec![
            ProviderDescriptor::new(
                self.config.provider_id.clone(),
                "TCP ASR Bridge",
                CapabilityDomain::Asr,
                RuntimeMode::RemoteGateway,
            )
            .with_capability(Capability::enabled("streaming-input"))
            .with_capability(Capability::enabled("bridge-tcp")),
        ]
    }

    async fn start_stream(
        &self,
        request: StreamRequest,
    ) -> Result<BridgeAsrSessionHandle, BridgeError> {
        let stream = TcpStream::connect(&self.config.address)
            .await
            .map_err(|error| {
                BridgeError::Unavailable(format!(
                    "failed to connect to remote bridge {}: {error}",
                    self.config.address
                ))
            })?;

        let session_id = SessionId::new();
        let session = AsrSession {
            id: session_id,
            provider_id: self.config.provider_id.clone(),
            accepted_input_format: request.input_format.clone(),
        };

        let (command_tx, command_rx) = mpsc::channel::<BridgeCommand>(64);
        let (event_tx, event_rx) = mpsc::channel::<BridgeAsrEvent>(64);
        let (read_half, write_half) = tokio::io::split(stream);
        tokio::spawn(run_bridge_writer(
            session.id.clone(),
            request,
            write_half,
            command_rx,
        ));
        tokio::spawn(run_bridge_reader(read_half, event_tx));

        Ok(BridgeAsrSessionHandle::new(session, command_tx, event_rx))
    }
}

async fn run_bridge_writer<W>(
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
            locale: start_request.options.language,
            should_report_partials: start_request.options.interim_results,
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

async fn run_bridge_reader<R>(reader_source: R, event_tx: mpsc::Sender<BridgeAsrEvent>)
where
    R: AsyncRead + Unpin,
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

async fn wait_child_exit(mut child: Child) {
    match child.wait().await {
        Ok(status) => debug!("bridge process exited with status {status}"),
        Err(error) => warn!("bridge process wait failed: {error}"),
    }
}

#[derive(Debug, Clone, Serialize)]
struct BridgeStartFrame {
    #[serde(rename = "type")]
    type_name: &'static str,
    request_id: Option<String>,
    session_id: Option<SessionId>,
    payload: BridgeStartPayload,
}

#[derive(Debug, Clone, Serialize)]
struct BridgeStartPayload {
    locale: Option<String>,
    should_report_partials: bool,
    requires_on_device: bool,
    input_format: speechmesh_core::AudioFormat,
}

#[derive(Debug, Clone, Serialize)]
struct BridgeAudioFrame {
    #[serde(rename = "type")]
    type_name: &'static str,
    request_id: Option<String>,
    session_id: Option<SessionId>,
    payload: BridgeAudioPayload,
}

#[derive(Debug, Clone, Serialize)]
struct BridgeAudioPayload {
    data_base64: String,
}

#[derive(Debug, Clone, Serialize)]
struct BridgeEmptyFrame {
    #[serde(rename = "type")]
    type_name: &'static str,
    request_id: Option<String>,
    session_id: Option<SessionId>,
    payload: BridgeEmptyPayload,
}

#[derive(Debug, Clone, Serialize)]
struct BridgeEmptyPayload {}

fn extract_final_transcript(payload: &Value) -> Option<Transcript> {
    let text = payload.get("text").and_then(Value::as_str)?.to_string();
    let segments = payload
        .get("segments")
        .and_then(Value::as_array)
        .map(|segments| {
            segments
                .iter()
                .filter_map(|segment| {
                    let text = segment
                        .get("substring")
                        .and_then(Value::as_str)?
                        .to_string();
                    let start_ms = segment
                        .get("timestamp_s")
                        .and_then(Value::as_f64)
                        .map(seconds_to_ms);
                    let duration_ms = segment
                        .get("duration_s")
                        .and_then(Value::as_f64)
                        .map(seconds_to_ms);
                    let end_ms = match (start_ms, duration_ms) {
                        (Some(start_ms), Some(duration_ms)) => Some(start_ms + duration_ms),
                        _ => None,
                    };
                    Some(TranscriptSegment {
                        text,
                        is_final: true,
                        start_ms,
                        end_ms,
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    Some(Transcript {
        text,
        language: None,
        segments,
    })
}

fn seconds_to_ms(seconds: f64) -> u64 {
    (seconds * 1000.0).round() as u64
}
