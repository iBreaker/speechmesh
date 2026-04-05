use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use futures_util::{SinkExt, StreamExt};
use speechmesh_core::{CapabilityDomain, ErrorInfo, RequestId, SessionId};
use speechmesh_transport::{
    AsrResultPayload, AsrWordPayload, ClientMessage, DiscoverRequest, DiscoverResult, EmptyPayload,
    ErrorPayload, HelloResponse, ServerMessage, SessionEndedPayload, SessionStartedPayload,
    TransportKind,
};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::accept_hdr_async;
use tokio_tungstenite::tungstenite::handshake::server::{Request, Response};
use tokio_tungstenite::tungstenite::protocol::Message;
use tracing::{error, info, warn};

use crate::agent::{AgentRegistry, handle_agent_connection};
use crate::bridge::{BridgeAsrEvent, BridgeAsrSessionHandle, BridgeError, SharedAsrBridge};

#[derive(Debug, Clone)]
pub struct ServerConfig {
    pub listen: SocketAddr,
    pub protocol_version: String,
    pub server_name: String,
}

pub async fn run_server(
    config: ServerConfig,
    asr_bridge: SharedAsrBridge,
    agent_registry: Option<AgentRegistry>,
) -> Result<()> {
    let listener = TcpListener::bind(config.listen)
        .await
        .with_context(|| format!("failed to bind websocket listener at {}", config.listen))?;
    info!("speechmeshd listening on {}", config.listen);

    loop {
        let (stream, peer) = listener.accept().await.context("accept failed")?;
        let bridge = asr_bridge.clone();
        let registry = agent_registry.clone();
        let per_connection_config = config.clone();
        tokio::spawn(async move {
            if let Err(error) =
                handle_connection(stream, peer, per_connection_config, bridge, registry).await
            {
                warn!("connection {peer} failed: {error:?}");
            }
        });
    }
}

struct ActiveAsrSession {
    session_id: SessionId,
    bridge: BridgeAsrSessionHandle,
}

async fn handle_connection(
    stream: TcpStream,
    peer: SocketAddr,
    config: ServerConfig,
    asr_bridge: SharedAsrBridge,
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

    handle_client_websocket(websocket, peer, config, asr_bridge).await
}

async fn handle_client_websocket(
    websocket: WebSocketStream<TcpStream>,
    peer: SocketAddr,
    config: ServerConfig,
    asr_bridge: SharedAsrBridge,
) -> Result<()> {
    let (mut sink, mut source) = websocket.split();
    let (out_tx, mut out_rx) = mpsc::channel::<ServerMessage>(64);
    let writer = tokio::spawn(async move {
        while let Some(event) = out_rx.recv().await {
            let encoded = serde_json::to_string(&event)?;
            sink.send(Message::Text(encoded.into())).await?;
        }
        Result::<(), anyhow::Error>::Ok(())
    });

    let mut active_asr: Option<ActiveAsrSession> = None;
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
                            &out_tx,
                            &mut active_asr,
                        )
                        .await
                        {
                            send_error(
                                &out_tx,
                                None,
                                active_asr.as_ref().map(|s| s.session_id.clone()),
                                error,
                            )
                            .await;
                        }
                    }
                    Err(error) => {
                        send_error(
                            &out_tx,
                            None,
                            active_asr.as_ref().map(|s| s.session_id.clone()),
                            ServerError::InvalidRequest(format!("invalid json frame: {error}")),
                        )
                        .await;
                    }
                }
            }
            Message::Binary(bytes) => {
                if let Some(session) = active_asr.as_ref() {
                    if let Err(error) = session.bridge.push_audio(bytes.to_vec()).await {
                        send_error(
                            &out_tx,
                            None,
                            Some(session.session_id.clone()),
                            ServerError::Bridge(error),
                        )
                        .await;
                    }
                } else {
                    send_error(
                        &out_tx,
                        None,
                        None,
                        ServerError::SessionNotFound("no active ASR session".to_string()),
                    )
                    .await;
                }
            }
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

    if let Some(session) = active_asr.take() {
        if let Err(error) = session.bridge.stop().await {
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
    out_tx: &mpsc::Sender<ServerMessage>,
    active_asr: &mut Option<ActiveAsrSession>,
) -> Result<(), ServerError> {
    match message {
        ClientMessage::Hello {
            request_id,
            payload,
        } => {
            let hello = ServerMessage::HelloOk {
                request_id,
                payload: HelloResponse {
                    protocol_version: payload.protocol_version,
                    server_name: config.server_name.clone(),
                    one_session_per_connection: true,
                },
            };
            out_tx
                .send(hello)
                .await
                .map_err(|_| ServerError::Disconnected)?;
            Ok(())
        }
        ClientMessage::Discover {
            request_id,
            payload,
        } => {
            let discovered = handle_discover(payload, asr_bridge);
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
            if active_asr.is_some() {
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
                    },
                })
                .await
                .map_err(|_| ServerError::Disconnected)?;

            *active_asr = Some(ActiveAsrSession {
                session_id: session.id,
                bridge: bridge_session,
            });
            Ok(())
        }
        ClientMessage::AsrCommit { session_id, .. } => {
            let session = active_asr
                .as_ref()
                .ok_or_else(|| ServerError::SessionNotFound("no active session".to_string()))?;
            if session.session_id != session_id {
                return Err(ServerError::SessionNotFound(
                    "session id does not match active session".to_string(),
                ));
            }
            session.bridge.commit().await?;
            Ok(())
        }
        ClientMessage::SessionStop { session_id, .. }
        | ClientMessage::SessionCancel { session_id, .. } => {
            let session = active_asr
                .as_ref()
                .ok_or_else(|| ServerError::SessionNotFound("no active session".to_string()))?;
            if session.session_id != session_id {
                return Err(ServerError::SessionNotFound(
                    "session id does not match active session".to_string(),
                ));
            }
            session.bridge.stop().await?;
            *active_asr = None;
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
        ClientMessage::TtsVoices { .. } | ClientMessage::TtsStart { .. } => Err(
            ServerError::Unsupported("tts is not implemented in speechmeshd v0".to_string()),
        ),
    }
}

fn handle_discover(payload: DiscoverRequest, asr_bridge: &SharedAsrBridge) -> DiscoverResult {
    let requested_domains = payload.domains;
    let want_asr = requested_domains.is_empty()
        || requested_domains
            .iter()
            .any(|domain| matches!(domain, CapabilityDomain::Asr));
    if !want_asr {
        return DiscoverResult {
            providers: Vec::new(),
        };
    }

    DiscoverResult {
        providers: asr_bridge.descriptors(),
    }
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
    use speechmesh_core::ProviderSelector;

    #[test]
    fn discover_filters_non_asr_domains() {
        let bridge: crate::bridge::SharedAsrBridge =
            std::sync::Arc::new(crate::bridge::MockAsrBridge::new("mock.asr"));
        let result = handle_discover(
            DiscoverRequest {
                domains: vec![CapabilityDomain::Tts],
            },
            &bridge,
        );
        assert!(result.providers.is_empty());
    }

    #[test]
    fn discover_returns_asr_provider_when_requested() {
        let bridge: crate::bridge::SharedAsrBridge =
            std::sync::Arc::new(crate::bridge::MockAsrBridge::new("mock.asr"));
        let result = handle_discover(
            DiscoverRequest {
                domains: vec![CapabilityDomain::Asr],
            },
            &bridge,
        );
        assert_eq!(result.providers.len(), 1);
        assert_eq!(result.providers[0].id, "mock.asr");
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
    fn provider_selector_is_compatible_with_discover_and_start_paths() {
        let selector = ProviderSelector::default();
        assert!(selector.provider_id.is_none());
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
