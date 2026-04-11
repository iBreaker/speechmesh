use std::sync::Once;

extern crate self as speechmesh_sdk;

use futures_util::{SinkExt, StreamExt};
use speechmesh_core::{RequestId, SessionId};
use speechmesh_transport::{
    ClientMessage, DiscoverRequest, DiscoverResult, EmptyPayload, HelloRequest, ServerMessage,
    TtsInputAppendPayload, VoiceListRequest, VoiceListResult,
};
use thiserror::Error;
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::protocol::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async};

static RUSTLS_INIT: Once = Once::new();

#[path = "bin/speechmesh-cli.rs"]
pub mod cli_app;

pub use speechmesh_asr::{RecognitionOptions, StreamRequest};
pub use speechmesh_core::{
    AudioEncoding, AudioFormat, Capability, CapabilityDomain, ErrorInfo, ProviderDescriptor,
    ProviderSelector, RuntimeMode,
};
pub use speechmesh_transport::{
    AsrResultPayload, AsrWordPayload, DiscoverResult as ProviderDiscoverResult, HelloResponse,
    ServerMessage as GatewayMessage, SessionEndedPayload, SessionStartedPayload,
    TtsAudioDeltaPayload, TtsAudioDonePayload, VoiceListResult as ProviderVoiceListResult,
};
pub use speechmesh_tts::{
    StreamRequest as TtsStreamRequest, SynthesisInputKind, SynthesisOptions, VoiceDescriptor,
};

#[derive(Debug, Clone)]
pub struct ClientConfig {
    pub url: String,
    pub protocol_version: String,
    pub client_name: String,
}

impl ClientConfig {
    pub fn new(url: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            protocol_version: "v1".to_string(),
            client_name: "speechmesh-rust-sdk".to_string(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct SessionStarted {
    pub request_id: Option<RequestId>,
    pub session_id: SessionId,
    pub payload: speechmesh_transport::SessionStartedPayload,
}

#[derive(Debug)]
pub struct Client {
    config: ClientConfig,
    websocket: WebSocketStream<MaybeTlsStream<TcpStream>>,
    next_request_id: u64,
    active_session_id: Option<SessionId>,
    active_session_domain: Option<CapabilityDomain>,
}

impl Client {
    pub async fn connect(config: ClientConfig) -> Result<Self, ClientError> {
        ensure_rustls_provider();
        let (websocket, _) = connect_async(&config.url)
            .await
            .map_err(|error| ClientError::Connect(error.to_string()))?;
        let mut client = Self {
            config,
            websocket,
            next_request_id: 0,
            active_session_id: None,
            active_session_domain: None,
        };
        client.handshake().await?;
        Ok(client)
    }

    pub fn url(&self) -> &str {
        &self.config.url
    }

    pub fn active_session_id(&self) -> Option<&SessionId> {
        self.active_session_id.as_ref()
    }

    pub fn active_session_domain(&self) -> Option<CapabilityDomain> {
        self.active_session_domain
    }

    pub async fn discover(
        &mut self,
        domains: Vec<CapabilityDomain>,
    ) -> Result<DiscoverResult, ClientError> {
        let request_id = self.next_request_id();
        self.send_json(ClientMessage::Discover {
            request_id: request_id.clone(),
            payload: DiscoverRequest { domains },
        })
        .await?;

        loop {
            match self.recv().await? {
                ServerMessage::DiscoverResult {
                    request_id: response_id,
                    payload,
                } if response_id == request_id => return Ok(payload),
                ServerMessage::Error {
                    request_id: response_id,
                    session_id,
                    payload,
                } if response_id.as_ref() == Some(&request_id) => {
                    return Err(ClientError::Server {
                        request_id: response_id,
                        session_id,
                        error: payload.error,
                    });
                }
                _ => {}
            }
        }
    }

    pub async fn discover_asr(&mut self) -> Result<DiscoverResult, ClientError> {
        self.discover(vec![CapabilityDomain::Asr]).await
    }

    pub async fn discover_tts(&mut self) -> Result<DiscoverResult, ClientError> {
        self.discover(vec![CapabilityDomain::Tts]).await
    }

    pub async fn list_tts_voices(
        &mut self,
        provider: ProviderSelector,
        language: Option<String>,
    ) -> Result<VoiceListResult, ClientError> {
        let request_id = self.next_request_id();
        self.send_json(ClientMessage::TtsVoices {
            request_id: request_id.clone(),
            payload: VoiceListRequest { provider, language },
        })
        .await?;

        loop {
            match self.recv().await? {
                ServerMessage::TtsVoicesResult {
                    request_id: response_id,
                    payload,
                } if response_id == request_id => return Ok(payload),
                ServerMessage::Error {
                    request_id: response_id,
                    session_id,
                    payload,
                } if response_id.as_ref() == Some(&request_id) => {
                    return Err(ClientError::Server {
                        request_id: response_id,
                        session_id,
                        error: payload.error,
                    });
                }
                _ => {}
            }
        }
    }

    pub async fn start_asr(
        &mut self,
        request: speechmesh_asr::StreamRequest,
    ) -> Result<SessionStarted, ClientError> {
        if self.active_session_id.is_some() {
            return Err(ClientError::Protocol(
                "speechmesh server allows only one active session per connection".to_string(),
            ));
        }

        let request_id = self.next_request_id();
        self.send_json(ClientMessage::AsrStart {
            request_id: request_id.clone(),
            payload: request,
        })
        .await?;

        loop {
            match self.recv().await? {
                ServerMessage::SessionStarted {
                    request_id: response_id,
                    session_id,
                    payload,
                } if response_id.as_ref() == Some(&request_id) => {
                    self.active_session_id = Some(session_id.clone());
                    return Ok(SessionStarted {
                        request_id: response_id,
                        session_id,
                        payload,
                    });
                }
                ServerMessage::Error {
                    request_id: response_id,
                    session_id,
                    payload,
                } if response_id.as_ref() == Some(&request_id) => {
                    return Err(ClientError::Server {
                        request_id: response_id,
                        session_id,
                        error: payload.error,
                    });
                }
                _ => {}
            }
        }
    }

    pub async fn start_tts(
        &mut self,
        request: speechmesh_tts::StreamRequest,
    ) -> Result<SessionStarted, ClientError> {
        if self.active_session_id.is_some() {
            return Err(ClientError::Protocol(
                "speechmesh server allows only one active session per connection".to_string(),
            ));
        }

        let request_id = self.next_request_id();
        self.send_json(ClientMessage::TtsStart {
            request_id: request_id.clone(),
            payload: request,
        })
        .await?;

        loop {
            match self.recv().await? {
                ServerMessage::SessionStarted {
                    request_id: response_id,
                    session_id,
                    payload,
                } if response_id.as_ref() == Some(&request_id) => {
                    self.active_session_id = Some(session_id.clone());
                    self.active_session_domain = Some(payload.domain);
                    return Ok(SessionStarted {
                        request_id: response_id,
                        session_id,
                        payload,
                    });
                }
                ServerMessage::Error {
                    request_id: response_id,
                    session_id,
                    payload,
                } if response_id.as_ref() == Some(&request_id) => {
                    return Err(ClientError::Server {
                        request_id: response_id,
                        session_id,
                        error: payload.error,
                    });
                }
                _ => {}
            }
        }
    }

    pub async fn send_audio(&mut self, chunk: &[u8]) -> Result<(), ClientError> {
        self.ensure_active_session_domain(CapabilityDomain::Asr)?;
        self.websocket
            .send(Message::Binary(chunk.to_vec().into()))
            .await
            .map_err(|error| ClientError::Transport(error.to_string()))
    }

    pub async fn commit(&mut self) -> Result<(), ClientError> {
        let session_id = self
            .active_session_id
            .clone()
            .ok_or(ClientError::NoActiveSession)?;
        match self.active_session_domain {
            Some(CapabilityDomain::Asr) => {
                self.send_json(ClientMessage::AsrCommit {
                    session_id,
                    payload: EmptyPayload::default(),
                })
                .await
            }
            Some(CapabilityDomain::Tts) => {
                self.send_json(ClientMessage::TtsCommit {
                    session_id,
                    payload: EmptyPayload::default(),
                })
                .await
            }
            Some(domain) => Err(ClientError::Protocol(format!(
                "commit is unsupported for active session domain {domain:?}"
            ))),
            None => Err(ClientError::NoActiveSession),
        }
    }

    pub async fn append_tts_input(&mut self, delta: impl Into<String>) -> Result<(), ClientError> {
        self.ensure_active_session_domain(CapabilityDomain::Tts)?;
        let session_id = self
            .active_session_id
            .clone()
            .ok_or(ClientError::NoActiveSession)?;
        self.send_json(ClientMessage::TtsInputAppend {
            session_id,
            payload: TtsInputAppendPayload {
                delta: delta.into(),
            },
        })
        .await
    }

    pub async fn stop(&mut self) -> Result<(), ClientError> {
        let session_id = self
            .active_session_id
            .clone()
            .ok_or(ClientError::NoActiveSession)?;
        self.send_json(ClientMessage::SessionStop {
            session_id,
            payload: EmptyPayload::default(),
        })
        .await
    }

    pub async fn recv(&mut self) -> Result<ServerMessage, ClientError> {
        loop {
            let frame = self
                .websocket
                .next()
                .await
                .ok_or(ClientError::Closed)?
                .map_err(|error| ClientError::Transport(error.to_string()))?;
            match frame {
                Message::Text(text) => {
                    let message: ServerMessage = serde_json::from_str(&text)
                        .map_err(|error| ClientError::Protocol(error.to_string()))?;
                    self.observe_message(&message);
                    return Ok(message);
                }
                Message::Close(_) => return Err(ClientError::Closed),
                Message::Ping(payload) => {
                    self.websocket
                        .send(Message::Pong(payload))
                        .await
                        .map_err(|error| ClientError::Transport(error.to_string()))?;
                }
                Message::Pong(_) => {}
                Message::Binary(_) | Message::Frame(_) => {
                    return Err(ClientError::Protocol(
                        "unexpected non-text frame from server".to_string(),
                    ));
                }
            }
        }
    }

    pub async fn close(&mut self) -> Result<(), ClientError> {
        self.websocket
            .close(None)
            .await
            .map_err(|error| ClientError::Transport(error.to_string()))
    }

    async fn handshake(&mut self) -> Result<(), ClientError> {
        self.send_json(ClientMessage::Hello {
            request_id: None,
            payload: HelloRequest {
                protocol_version: self.config.protocol_version.clone(),
                client_name: Some(self.config.client_name.clone()),
            },
        })
        .await?;

        loop {
            match self.recv().await? {
                ServerMessage::HelloOk { .. } => return Ok(()),
                ServerMessage::Error {
                    request_id,
                    session_id,
                    payload,
                } => {
                    return Err(ClientError::Server {
                        request_id,
                        session_id,
                        error: payload.error,
                    });
                }
                _ => {}
            }
        }
    }

    async fn send_json(&mut self, message: ClientMessage) -> Result<(), ClientError> {
        let encoded = serde_json::to_string(&message)
            .map_err(|error| ClientError::Protocol(error.to_string()))?;
        self.websocket
            .send(Message::Text(encoded.into()))
            .await
            .map_err(|error| ClientError::Transport(error.to_string()))
    }

    fn next_request_id(&mut self) -> RequestId {
        self.next_request_id += 1;
        RequestId::new(format!("req_{}", self.next_request_id))
    }

    fn observe_message(&mut self, message: &ServerMessage) {
        match message {
            ServerMessage::SessionStarted {
                session_id,
                payload,
                ..
            } => {
                self.active_session_id = Some(session_id.clone());
                self.active_session_domain = Some(payload.domain);
            }
            ServerMessage::SessionEnded { session_id, .. } => {
                if self.active_session_id.as_ref() == Some(session_id) {
                    self.active_session_id = None;
                    self.active_session_domain = None;
                }
            }
            _ => {}
        }
    }

    fn ensure_active_session_domain(&self, expected: CapabilityDomain) -> Result<(), ClientError> {
        match self.active_session_domain {
            Some(actual) if actual == expected => Ok(()),
            Some(actual) => Err(ClientError::Protocol(format!(
                "active session domain mismatch: expected {expected:?}, got {actual:?}"
            ))),
            None => Err(ClientError::NoActiveSession),
        }
    }
}

#[derive(Debug, Error)]
pub enum ClientError {
    #[error("connect failed: {0}")]
    Connect(String),
    #[error("transport failed: {0}")]
    Transport(String),
    #[error("protocol failed: {0}")]
    Protocol(String),
    #[error("server error request_id={request_id:?} session_id={session_id:?}: {error:?}")]
    Server {
        request_id: Option<RequestId>,
        session_id: Option<SessionId>,
        error: speechmesh_core::ErrorInfo,
    },
    #[error("no active session")]
    NoActiveSession,
    #[error("connection closed")]
    Closed,
}

fn ensure_rustls_provider() {
    RUSTLS_INIT.call_once(|| {
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
}
