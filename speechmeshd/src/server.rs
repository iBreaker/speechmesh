use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use futures_util::{SinkExt, StreamExt};
use speechmesh_core::{CapabilityDomain, ErrorInfo, RequestId, SessionId};
use speechmesh_transport::{
    AsrResultPayload, AsrWordPayload, ClientMessage, ControlAgentStatusPayload,
    ControlAgentStatusResultPayload, ControlDevicesListPayload, ControlErrorPayload,
    ControlPlayAudioAcceptedPayload, ControlPlayAudioPayload, ControlRequest, ControlResponse,
    DiscoverRequest, DiscoverResult, EmptyPayload, ErrorPayload, HelloResponse, ServerMessage,
    SessionEndedPayload, SessionStartedPayload, TransportKind, TtsAudioDeltaPayload,
    TtsAudioDonePayload, VoiceListResult,
};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio::time::timeout;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::accept_hdr_async;
use tokio_tungstenite::tungstenite::handshake::server::{Request, Response};
use tokio_tungstenite::tungstenite::protocol::Message;
use tracing::{error, info, warn};

use crate::agent::{
    AgentRegistry, AgentSnapshotFilter, PlayAudioRouteRequest, handle_agent_connection,
};
use crate::asr_bridge::{BridgeAsrEvent, BridgeAsrSessionHandle, SharedAsrBridge};
use crate::bridge_support::BridgeError;
use crate::tts_bridge::{BridgeTtsEvent, BridgeTtsSessionHandle, SharedTtsBridge};

const CONTROL_PLAY_AUDIO_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Clone)]
pub struct ServerConfig {
    pub listen: SocketAddr,
    pub protocol_version: String,
    pub server_name: String,
}

static CONTROL_TASK_COUNTER: AtomicU64 = AtomicU64::new(1);

pub async fn run_server(
    config: ServerConfig,
    asr_bridge: SharedAsrBridge,
    tts_bridge: Option<SharedTtsBridge>,
    agent_registry: Option<AgentRegistry>,
) -> Result<()> {
    let listener = TcpListener::bind(config.listen)
        .await
        .with_context(|| format!("failed to bind websocket listener at {}", config.listen))?;
    info!("speechmeshd listening on {}", config.listen);

    loop {
        let (stream, peer) = listener.accept().await.context("accept failed")?;
        if let Err(error) =
            crate::tcp_keepalive::configure(&stream, crate::tcp_keepalive::DEFAULT_INTERVAL)
        {
            warn!("failed to set TCP keepalive for {peer}: {error}");
        }
        let asr = asr_bridge.clone();
        let tts = tts_bridge.clone();
        let registry = agent_registry.clone();
        let per_connection_config = config.clone();
        tokio::spawn(async move {
            if let Err(error) =
                handle_connection(stream, peer, per_connection_config, asr, tts, registry).await
            {
                warn!("connection {peer} failed: {error:?}");
            }
        });
    }
}

enum ActiveSession {
    Asr {
        session_id: SessionId,
        bridge: BridgeAsrSessionHandle,
    },
    Tts {
        session_id: SessionId,
        bridge: BridgeTtsSessionHandle,
    },
}

impl ActiveSession {
    fn session_id(&self) -> &SessionId {
        match self {
            ActiveSession::Asr { session_id, .. } | ActiveSession::Tts { session_id, .. } => {
                session_id
            }
        }
    }

    async fn stop(&self) -> Result<(), BridgeError> {
        match self {
            ActiveSession::Asr { bridge, .. } => bridge.stop().await,
            ActiveSession::Tts { bridge, .. } => bridge.stop().await,
        }
    }
}

async fn handle_connection(
    stream: TcpStream,
    peer: SocketAddr,
    config: ServerConfig,
    asr_bridge: SharedAsrBridge,
    tts_bridge: Option<SharedTtsBridge>,
    agent_registry: Option<AgentRegistry>,
) -> Result<()> {
    let (websocket, path) = accept_websocket_with_path(stream)
        .await
        .context("websocket handshake failed")?;
    let path = normalize_path(&path);
    info!("websocket connection opened from {peer} path={path}");

    if path == "/agent" {
        let registry = agent_registry
            .ok_or_else(|| anyhow::anyhow!("agent endpoint is disabled for this bridge mode"))?;
        return handle_agent_connection(websocket, peer, registry, config.server_name.clone())
            .await
            .map_err(anyhow::Error::from);
    }
    if path == "/control" {
        let registry = agent_registry
            .ok_or_else(|| anyhow::anyhow!("control endpoint requires agent registry"))?;
        return handle_control_websocket(websocket, peer, registry)
            .await
            .map_err(anyhow::Error::from);
    }

    handle_client_websocket(websocket, peer, config, asr_bridge, tts_bridge).await
}

async fn handle_control_websocket(
    mut websocket: WebSocketStream<TcpStream>,
    peer: SocketAddr,
    agent_registry: AgentRegistry,
) -> Result<(), BridgeError> {
    info!("control websocket opened from {peer}");
    while let Some(frame) = websocket.next().await {
        let frame =
            frame.map_err(|error| BridgeError::Io(format!("control read failed: {error}")))?;
        match frame {
            Message::Text(text) => {
                let incoming = serde_json::from_str::<ControlRequest>(&text);
                match incoming {
                    Ok(ControlRequest::PlayAudio { payload }) => {
                        if let Err(error) =
                            handle_control_play_audio(&mut websocket, &agent_registry, payload)
                                .await
                        {
                            send_control_error(&mut websocket, error.to_string()).await?;
                        }
                    }
                    Ok(ControlRequest::DevicesList) => {
                        if let Err(error) =
                            handle_control_devices_list(&mut websocket, &agent_registry).await
                        {
                            send_control_error(&mut websocket, error.to_string()).await?;
                        }
                    }
                    Ok(ControlRequest::AgentStatus { payload }) => {
                        if let Err(error) =
                            handle_control_agent_status(&mut websocket, &agent_registry, payload)
                                .await
                        {
                            send_control_error(&mut websocket, error.to_string()).await?;
                        }
                    }
                    Err(error) => {
                        send_control_error(
                            &mut websocket,
                            format!("invalid control payload: {error}"),
                        )
                        .await?;
                    }
                }
            }
            Message::Ping(_) => {
                send_control_message(&mut websocket, ControlResponse::Pong {}).await?;
            }
            Message::Binary(_) => {
                send_control_error(
                    &mut websocket,
                    "binary frames are unsupported on /control".to_string(),
                )
                .await?;
            }
            Message::Close(_) => break,
            Message::Pong(_) | Message::Frame(_) => {}
        }
    }
    info!("control websocket closed from {peer}");
    Ok(())
}

async fn handle_control_play_audio(
    websocket: &mut WebSocketStream<TcpStream>,
    agent_registry: &AgentRegistry,
    payload: ControlPlayAudioPayload,
) -> Result<(), BridgeError> {
    let task_id = payload
        .task_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .unwrap_or_else(next_control_task_id);
    let bytes = BASE64_STANDARD
        .decode(payload.audio_base64.as_bytes())
        .map_err(|error| BridgeError::Protocol(format!("invalid audio_base64 payload: {error}")))?;
    if bytes.is_empty() {
        return Err(BridgeError::Protocol(
            "play_audio payload is empty".to_string(),
        ));
    }

    let task_waiter = agent_registry.subscribe_task_waiter(task_id.clone()).await;
    let routed_agent_id = match agent_registry
        .route_play_audio_start(PlayAudioRouteRequest {
            task_id: task_id.clone(),
            device_id: payload.device_id,
            output_target: payload.output_target,
            agent_id: payload.agent_id,
            format: payload.format,
        })
        .await
    {
        Ok(agent_id) => agent_id,
        Err(error) => {
            agent_registry.remove_task_waiter(&task_id).await;
            return Err(error);
        }
    };

    let chunk_size = payload
        .chunk_size_bytes
        .filter(|value| *value > 0)
        .unwrap_or(16 * 1024);
    let mut chunk_count = 0_u64;
    for chunk in bytes.chunks(chunk_size) {
        chunk_count += 1;
        agent_registry
            .route_play_audio_chunk(&task_id, BASE64_STANDARD.encode(chunk))
            .await?;
    }
    agent_registry.route_play_audio_finish(&task_id).await?;

    match timeout(CONTROL_PLAY_AUDIO_TIMEOUT, task_waiter).await {
        Ok(Ok(Ok(_))) => {}
        Ok(Ok(Err(error))) => return Err(error),
        Ok(Err(_)) => {
            return Err(BridgeError::Disconnected(format!(
                "task waiter dropped before playback completed: {task_id}"
            )));
        }
        Err(_) => {
            agent_registry.remove_task_waiter(&task_id).await;
            return Err(BridgeError::Unavailable(format!(
                "timed out waiting for playback completion: {task_id}"
            )));
        }
    }

    send_control_message(
        websocket,
        ControlResponse::PlayAudioAccepted {
            payload: ControlPlayAudioAcceptedPayload {
                task_id,
                routed_agent_id,
                chunk_count,
                total_bytes: bytes.len() as u64,
            },
        },
    )
    .await
}

async fn handle_control_devices_list(
    websocket: &mut WebSocketStream<TcpStream>,
    agent_registry: &AgentRegistry,
) -> Result<(), BridgeError> {
    let snapshot = agent_registry
        .snapshot(AgentSnapshotFilter::default())
        .await;
    send_control_message(
        websocket,
        ControlResponse::DevicesList {
            payload: ControlDevicesListPayload { agents: snapshot },
        },
    )
    .await
}

async fn handle_control_agent_status(
    websocket: &mut WebSocketStream<TcpStream>,
    agent_registry: &AgentRegistry,
    payload: ControlAgentStatusPayload,
) -> Result<(), BridgeError> {
    let filter = AgentSnapshotFilter {
        agent_id: payload.agent_id.clone(),
        device_id: payload.device_id.clone(),
    };
    let mut snapshot = agent_registry.snapshot(filter).await;
    let agent = snapshot.pop();
    send_control_message(
        websocket,
        ControlResponse::AgentStatus {
            payload: ControlAgentStatusResultPayload { agent },
        },
    )
    .await
}

async fn send_control_error(
    websocket: &mut WebSocketStream<TcpStream>,
    message: String,
) -> Result<(), BridgeError> {
    send_control_message(
        websocket,
        ControlResponse::Error {
            payload: ControlErrorPayload { message },
        },
    )
    .await
}

async fn send_control_message(
    websocket: &mut WebSocketStream<TcpStream>,
    message: ControlResponse,
) -> Result<(), BridgeError> {
    let encoded = serde_json::to_string(&message).map_err(|error| {
        BridgeError::Protocol(format!("failed to encode control frame: {error}"))
    })?;
    websocket
        .send(Message::Text(encoded.into()))
        .await
        .map_err(|error| BridgeError::Io(format!("control write failed: {error}")))
}

fn next_control_task_id() -> String {
    let sequence = CONTROL_TASK_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("play-audio-{sequence}")
}

async fn handle_client_websocket(
    websocket: WebSocketStream<TcpStream>,
    peer: SocketAddr,
    config: ServerConfig,
    asr_bridge: SharedAsrBridge,
    tts_bridge: Option<SharedTtsBridge>,
) -> Result<()> {
    let (mut sink, mut source) = websocket.split();
    let (out_tx, mut out_rx) = mpsc::channel::<ServerMessage>(128);
    let writer = tokio::spawn(async move {
        while let Some(event) = out_rx.recv().await {
            let encoded = serde_json::to_string(&event)?;
            sink.send(Message::Text(encoded.into())).await?;
        }
        Result::<(), anyhow::Error>::Ok(())
    });

    let mut active_session: Option<ActiveSession> = None;
    while let Some(frame) = source.next().await {
        let frame = frame.context("read websocket frame failed")?;
        match frame {
            Message::Text(text) => {
                let incoming: Result<ClientMessage, _> = serde_json::from_str(&text);
                match incoming {
                    Ok(message) => {
                        if let Err(error) = handle_client_message(
                            message,
                            &config,
                            &asr_bridge,
                            tts_bridge.as_ref(),
                            &out_tx,
                            &mut active_session,
                        )
                        .await
                        {
                            send_error(
                                &out_tx,
                                None,
                                active_session
                                    .as_ref()
                                    .map(|session| session.session_id().clone()),
                                error,
                            )
                            .await;
                        }
                    }
                    Err(error) => {
                        send_error(
                            &out_tx,
                            None,
                            active_session
                                .as_ref()
                                .map(|session| session.session_id().clone()),
                            ServerError::InvalidRequest(format!("invalid json frame: {error}")),
                        )
                        .await;
                    }
                }
            }
            Message::Binary(bytes) => match active_session.as_ref() {
                Some(ActiveSession::Asr { session_id, bridge }) => {
                    if let Err(error) = bridge.push_audio(bytes.to_vec()).await {
                        send_error(
                            &out_tx,
                            None,
                            Some(session_id.clone()),
                            ServerError::Bridge(error),
                        )
                        .await;
                    }
                }
                Some(ActiveSession::Tts { session_id, .. }) => {
                    send_error(
                        &out_tx,
                        None,
                        Some(session_id.clone()),
                        ServerError::Unsupported(
                            "binary frames are reserved for ASR audio input".to_string(),
                        ),
                    )
                    .await;
                }
                None => {
                    send_error(
                        &out_tx,
                        None,
                        None,
                        ServerError::SessionNotFound("no active ASR session".to_string()),
                    )
                    .await;
                }
            },
            Message::Close(_) => break,
            Message::Ping(payload) => {
                let _ = out_tx
                    .send(ServerMessage::Pong {
                        request_id: None,
                        payload: EmptyPayload::default(),
                    })
                    .await;
                if !payload.is_empty() {
                    info!("connection {peer} sent ping bytes {}", payload.len());
                }
            }
            Message::Pong(_) => {}
            Message::Frame(_) => {}
        }
    }

    if let Some(session) = active_session.take() {
        if let Err(error) = session.stop().await {
            warn!("failed to stop active session on disconnect: {error}");
        }
    }

    drop(out_tx);
    match writer.await {
        Ok(Ok(())) => {}
        Ok(Err(error)) => error!("writer task failed: {error:?}"),
        Err(error) => error!("writer task join failed: {error}"),
    }
    info!("websocket connection closed from {peer}");
    Ok(())
}

async fn accept_websocket_with_path(
    stream: TcpStream,
) -> Result<(WebSocketStream<TcpStream>, String)> {
    let request_path = Arc::new(Mutex::new(String::new()));
    let request_path_slot = request_path.clone();
    let websocket = accept_hdr_async(stream, move |request: &Request, response: Response| {
        if let Ok(mut slot) = request_path_slot.lock() {
            *slot = request.uri().path().to_string();
        }
        Ok(response)
    })
    .await?;
    let path = request_path
        .lock()
        .map(|value| value.clone())
        .unwrap_or_else(|_| "/ws".to_string());
    Ok((websocket, path))
}

fn normalize_path(path: &str) -> &str {
    if path.is_empty() || path == "/" {
        "/ws"
    } else {
        path
    }
}

async fn handle_client_message(
    message: ClientMessage,
    config: &ServerConfig,
    asr_bridge: &SharedAsrBridge,
    tts_bridge: Option<&SharedTtsBridge>,
    out_tx: &mpsc::Sender<ServerMessage>,
    active_session: &mut Option<ActiveSession>,
) -> Result<(), ServerError> {
    match message {
        ClientMessage::Hello {
            request_id,
            payload,
        } => {
            out_tx
                .send(ServerMessage::HelloOk {
                    request_id,
                    payload: HelloResponse {
                        protocol_version: payload.protocol_version,
                        server_name: config.server_name.clone(),
                        one_session_per_connection: true,
                    },
                })
                .await
                .map_err(|_| ServerError::Disconnected)?;
            Ok(())
        }
        ClientMessage::Discover {
            request_id,
            payload,
        } => {
            let discovered = handle_discover(payload, asr_bridge, tts_bridge);
            out_tx
                .send(ServerMessage::DiscoverResult {
                    request_id,
                    payload: discovered,
                })
                .await
                .map_err(|_| ServerError::Disconnected)?;
            Ok(())
        }
        ClientMessage::AsrStart {
            request_id,
            payload,
        } => {
            if active_session.is_some() {
                return Err(ServerError::Unsupported(
                    "one active session per connection".to_string(),
                ));
            }
            let mut bridge_session = asr_bridge.start_stream(payload).await?;
            let session = bridge_session.session.clone();
            let mut event_rx = bridge_session.take_event_rx().ok_or_else(|| {
                ServerError::Bridge(BridgeError::Disconnected(
                    "missing event stream".to_string(),
                ))
            })?;

            let event_out = out_tx.clone();
            let session_id = session.id.clone();
            tokio::spawn(async move {
                let mut sequence = 0_u64;
                let mut revision = 0_u64;
                let mut previous_text = String::new();
                let segment_id = 0_u64;
                while let Some(event) = event_rx.recv().await {
                    sequence += 1;
                    let outgoing = match event {
                        BridgeAsrEvent::Partial { text } => {
                            revision += 1;
                            let payload = AsrResultPayload {
                                segment_id,
                                revision,
                                delta: compute_delta(&previous_text, &text),
                                text: text.clone(),
                                is_final: false,
                                speech_final: false,
                                begin_time_ms: None,
                                end_time_ms: None,
                                words: Vec::new(),
                            };
                            previous_text = text;
                            ServerMessage::AsrResult {
                                session_id: session_id.clone(),
                                sequence,
                                payload,
                            }
                        }
                        BridgeAsrEvent::Final { transcript } => {
                            revision += 1;
                            let payload = AsrResultPayload {
                                segment_id,
                                revision,
                                delta: compute_delta(&previous_text, &transcript.text),
                                text: transcript.text.clone(),
                                is_final: true,
                                speech_final: true,
                                begin_time_ms: transcript
                                    .segments
                                    .first()
                                    .and_then(|segment| segment.start_ms),
                                end_time_ms: transcript
                                    .segments
                                    .last()
                                    .and_then(|segment| segment.end_ms),
                                words: transcript
                                    .segments
                                    .iter()
                                    .map(|segment| AsrWordPayload {
                                        text: segment.text.clone(),
                                        start_ms: segment.start_ms,
                                        end_ms: segment.end_ms,
                                        is_final: segment.is_final,
                                    })
                                    .collect(),
                            };
                            previous_text = transcript.text;
                            ServerMessage::AsrResult {
                                session_id: session_id.clone(),
                                sequence,
                                payload,
                            }
                        }
                        BridgeAsrEvent::Ended { reason } => ServerMessage::SessionEnded {
                            session_id: session_id.clone(),
                            payload: SessionEndedPayload { reason },
                        },
                        BridgeAsrEvent::Error { message } => ServerMessage::Error {
                            request_id: None,
                            session_id: Some(session_id.clone()),
                            payload: ErrorPayload {
                                error: ErrorInfo::new("provider_error", message),
                            },
                        },
                    };
                    if event_out.send(outgoing).await.is_err() {
                        return;
                    }
                }
            });

            out_tx
                .send(ServerMessage::SessionStarted {
                    request_id: Some(request_id),
                    session_id: session.id.clone(),
                    payload: SessionStartedPayload {
                        domain: CapabilityDomain::Asr,
                        provider_id: session.provider_id.clone(),
                        accepted_input_format: Some(session.accepted_input_format),
                        accepted_output_format: None,
                        input_mode: Some(session.input_mode),
                        output_mode: Some(session.output_mode),
                    },
                })
                .await
                .map_err(|_| ServerError::Disconnected)?;

            *active_session = Some(ActiveSession::Asr {
                session_id: session.id,
                bridge: bridge_session,
            });
            Ok(())
        }
        ClientMessage::AsrCommit { session_id, .. } => {
            let session = expect_active_asr(active_session.as_ref(), &session_id)?;
            session.commit().await?;
            Ok(())
        }
        ClientMessage::TtsVoices {
            request_id,
            payload,
        } => {
            let bridge = tts_bridge.ok_or_else(|| {
                ServerError::Unsupported("tts is not enabled in speechmeshd".to_string())
            })?;
            let voices = bridge.list_voices(payload).await?;
            out_tx
                .send(ServerMessage::TtsVoicesResult {
                    request_id,
                    payload: VoiceListResult { voices },
                })
                .await
                .map_err(|_| ServerError::Disconnected)?;
            Ok(())
        }
        ClientMessage::TtsStart {
            request_id,
            payload,
        } => {
            if active_session.is_some() {
                return Err(ServerError::Unsupported(
                    "one active session per connection".to_string(),
                ));
            }
            let bridge = tts_bridge.ok_or_else(|| {
                ServerError::Unsupported("tts is not enabled in speechmeshd".to_string())
            })?;

            let mut bridge_session = bridge.start_stream(payload).await?;
            let session = bridge_session.session.clone();
            let input_kind = bridge_session.input_kind;
            let mut event_rx = bridge_session.take_event_rx().ok_or_else(|| {
                ServerError::Bridge(BridgeError::Disconnected(
                    "missing TTS event stream".to_string(),
                ))
            })?;

            let event_out = out_tx.clone();
            let session_id = session.id.clone();
            tokio::spawn(async move {
                let mut sequence = 0_u64;
                let mut total_chunks = 0_u64;
                let mut total_bytes = 0_u64;
                while let Some(event) = event_rx.recv().await {
                    match event {
                        BridgeTtsEvent::Audio { chunk } => {
                            sequence += 1;
                            total_chunks += 1;
                            total_bytes += chunk.bytes.len() as u64;
                            let outgoing = ServerMessage::TtsAudioDelta {
                                session_id: session_id.clone(),
                                sequence,
                                payload: TtsAudioDeltaPayload {
                                    chunk_id: total_chunks,
                                    audio_base64: BASE64_STANDARD.encode(chunk.bytes),
                                    format: chunk.format,
                                    is_final: chunk.is_final,
                                },
                            };
                            if event_out.send(outgoing).await.is_err() {
                                return;
                            }
                        }
                        BridgeTtsEvent::Ended { reason } => {
                            sequence += 1;
                            let done = ServerMessage::TtsAudioDone {
                                session_id: session_id.clone(),
                                sequence,
                                payload: TtsAudioDonePayload {
                                    input_kind,
                                    total_chunks,
                                    total_bytes,
                                },
                            };
                            if event_out.send(done).await.is_err() {
                                return;
                            }
                            let _ = event_out
                                .send(ServerMessage::SessionEnded {
                                    session_id: session_id.clone(),
                                    payload: SessionEndedPayload { reason },
                                })
                                .await;
                            return;
                        }
                        BridgeTtsEvent::Error { message } => {
                            if event_out
                                .send(ServerMessage::Error {
                                    request_id: None,
                                    session_id: Some(session_id.clone()),
                                    payload: ErrorPayload {
                                        error: ErrorInfo::new("provider_error", message),
                                    },
                                })
                                .await
                                .is_err()
                            {
                                return;
                            }
                        }
                    }
                }
            });

            out_tx
                .send(ServerMessage::SessionStarted {
                    request_id: Some(request_id),
                    session_id: session.id.clone(),
                    payload: SessionStartedPayload {
                        domain: CapabilityDomain::Tts,
                        provider_id: session.provider_id.clone(),
                        accepted_input_format: None,
                        accepted_output_format: session.accepted_output_format.clone(),
                        input_mode: Some(session.input_mode),
                        output_mode: Some(session.output_mode),
                    },
                })
                .await
                .map_err(|_| ServerError::Disconnected)?;

            *active_session = Some(ActiveSession::Tts {
                session_id: session.id,
                bridge: bridge_session,
            });
            Ok(())
        }
        ClientMessage::TtsInputAppend {
            session_id,
            payload,
        } => {
            let session = expect_active_tts(active_session.as_ref(), &session_id)?;
            session.append_input(payload.delta).await?;
            Ok(())
        }
        ClientMessage::TtsCommit { session_id, .. } => {
            let session = expect_active_tts(active_session.as_ref(), &session_id)?;
            session.commit().await?;
            Ok(())
        }
        ClientMessage::SessionStop { session_id, .. }
        | ClientMessage::SessionCancel { session_id, .. } => {
            let current = active_session
                .as_ref()
                .ok_or_else(|| ServerError::SessionNotFound("no active session".to_string()))?;
            if current.session_id() != &session_id {
                return Err(ServerError::SessionNotFound(
                    "session id does not match active session".to_string(),
                ));
            }
            current.stop().await?;
            *active_session = None;
            Ok(())
        }
        ClientMessage::Ping { request_id, .. } => {
            out_tx
                .send(ServerMessage::Pong {
                    request_id,
                    payload: EmptyPayload::default(),
                })
                .await
                .map_err(|_| ServerError::Disconnected)?;
            Ok(())
        }
        ClientMessage::AsrTranscribe { .. } => Err(ServerError::Unsupported(
            "asr.transcribe is not implemented in speechmeshd v0".to_string(),
        )),
    }
}

fn expect_active_asr<'a>(
    active_session: Option<&'a ActiveSession>,
    session_id: &SessionId,
) -> Result<&'a BridgeAsrSessionHandle, ServerError> {
    match active_session {
        Some(ActiveSession::Asr {
            session_id: active_id,
            bridge,
        }) if active_id == session_id => Ok(bridge),
        Some(ActiveSession::Tts { .. }) => Err(ServerError::Unsupported(
            "the active session is a TTS session, not ASR".to_string(),
        )),
        Some(_) => Err(ServerError::SessionNotFound(
            "session id does not match active session".to_string(),
        )),
        None => Err(ServerError::SessionNotFound(
            "no active session".to_string(),
        )),
    }
}

fn expect_active_tts<'a>(
    active_session: Option<&'a ActiveSession>,
    session_id: &SessionId,
) -> Result<&'a BridgeTtsSessionHandle, ServerError> {
    match active_session {
        Some(ActiveSession::Tts {
            session_id: active_id,
            bridge,
        }) if active_id == session_id => Ok(bridge),
        Some(ActiveSession::Asr { .. }) => Err(ServerError::Unsupported(
            "the active session is an ASR session, not TTS".to_string(),
        )),
        Some(_) => Err(ServerError::SessionNotFound(
            "session id does not match active session".to_string(),
        )),
        None => Err(ServerError::SessionNotFound(
            "no active session".to_string(),
        )),
    }
}

fn handle_discover(
    payload: DiscoverRequest,
    asr_bridge: &SharedAsrBridge,
    tts_bridge: Option<&SharedTtsBridge>,
) -> DiscoverResult {
    let requested_domains = payload.domains;
    let want_asr = requested_domains.is_empty()
        || requested_domains
            .iter()
            .any(|domain| matches!(domain, CapabilityDomain::Asr));
    let want_tts = requested_domains.is_empty()
        || requested_domains
            .iter()
            .any(|domain| matches!(domain, CapabilityDomain::Tts));

    let mut providers = Vec::new();
    if want_asr {
        providers.extend(asr_bridge.descriptors());
    }
    if want_tts {
        if let Some(bridge) = tts_bridge {
            providers.extend(bridge.descriptors());
        }
    }

    DiscoverResult { providers }
}

async fn send_error(
    out_tx: &mpsc::Sender<ServerMessage>,
    request_id: Option<RequestId>,
    session_id: Option<SessionId>,
    error: ServerError,
) {
    let info = error.to_error_info();
    let _ = out_tx
        .send(ServerMessage::Error {
            request_id,
            session_id,
            payload: ErrorPayload { error: info },
        })
        .await;
}

#[derive(Debug, thiserror::Error)]
enum ServerError {
    #[error("invalid request: {0}")]
    InvalidRequest(String),
    #[error("unsupported request: {0}")]
    Unsupported(String),
    #[error("session not found: {0}")]
    SessionNotFound(String),
    #[error("bridge error: {0}")]
    Bridge(#[from] BridgeError),
    #[error("connection closed")]
    Disconnected,
}

impl ServerError {
    fn to_error_info(&self) -> ErrorInfo {
        match self {
            ServerError::InvalidRequest(message) => ErrorInfo::new("invalid_request", message),
            ServerError::Unsupported(message) => ErrorInfo::new("unsupported_request", message),
            ServerError::SessionNotFound(message) => ErrorInfo::new("session_not_found", message),
            ServerError::Bridge(error) => ErrorInfo::new("provider_error", error.to_string()),
            ServerError::Disconnected => ErrorInfo::new("connection_closed", "connection closed"),
        }
    }
}

fn compute_delta(previous: &str, current: &str) -> Option<String> {
    if current == previous {
        return None;
    }
    if previous.is_empty() {
        return Some(current.to_string());
    }
    if let Some(suffix) = current.strip_prefix(previous) {
        return Some(suffix.to_string());
    }
    Some(current.to_string())
}

pub fn parse_transport_kind(value: &str) -> Option<TransportKind> {
    match value {
        "websocket" | "ws" => Some(TransportKind::WebSocket),
        "http" => Some(TransportKind::Http),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discover_filters_non_registered_domains() {
        let asr_bridge: crate::asr_bridge::SharedAsrBridge =
            std::sync::Arc::new(crate::asr_bridge::MockAsrBridge::new("mock.asr"));
        let result = handle_discover(
            DiscoverRequest {
                domains: vec![CapabilityDomain::Transport],
            },
            &asr_bridge,
            None,
        );
        assert!(result.providers.is_empty());
    }

    #[test]
    fn discover_returns_asr_provider_when_requested() {
        let asr_bridge: crate::asr_bridge::SharedAsrBridge =
            std::sync::Arc::new(crate::asr_bridge::MockAsrBridge::new("mock.asr"));
        let result = handle_discover(
            DiscoverRequest {
                domains: vec![CapabilityDomain::Asr],
            },
            &asr_bridge,
            None,
        );
        assert_eq!(result.providers.len(), 1);
        assert_eq!(result.providers[0].id, "mock.asr");
    }

    #[test]
    fn discover_returns_tts_provider_when_requested() {
        let asr_bridge: crate::asr_bridge::SharedAsrBridge =
            std::sync::Arc::new(crate::asr_bridge::MockAsrBridge::new("mock.asr"));
        let tts_bridge: crate::tts_bridge::SharedTtsBridge =
            std::sync::Arc::new(crate::tts_bridge::MockTtsBridge::new("mock.tts"));
        let result = handle_discover(
            DiscoverRequest {
                domains: vec![CapabilityDomain::Tts],
            },
            &asr_bridge,
            Some(&tts_bridge),
        );
        assert_eq!(result.providers.len(), 1);
        assert_eq!(result.providers[0].id, "mock.tts");
    }

    #[test]
    fn parse_transport_kind_accepts_websocket_aliases() {
        assert_eq!(parse_transport_kind("ws"), Some(TransportKind::WebSocket));
        assert_eq!(
            parse_transport_kind("websocket"),
            Some(TransportKind::WebSocket)
        );
        assert_eq!(parse_transport_kind("http"), Some(TransportKind::Http));
        assert!(parse_transport_kind("grpc").is_none());
    }

    #[test]
    fn compute_delta_uses_suffix_for_simple_append() {
        assert_eq!(
            compute_delta("hello from", "hello from speech mesh"),
            Some(" speech mesh".to_string())
        );
    }

    #[test]
    fn compute_delta_falls_back_to_full_text_for_revisions() {
        assert_eq!(
            compute_delta(
                "hello from speech mesh this is an end to end",
                "hello from speech mesh this is an N to N"
            ),
            Some("hello from speech mesh this is an N to N".to_string())
        );
    }
}
