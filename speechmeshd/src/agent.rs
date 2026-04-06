use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use speechmesh_asr::{AsrSession, StreamRequest, Transcript};
use speechmesh_core::{
    Capability, CapabilityDomain, ProviderDescriptor, RuntimeMode, SessionId, StreamMode,
};
use tokio::sync::{Mutex, mpsc, oneshot};
use tokio::time::timeout;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::protocol::Message;
use tracing::{debug, info, warn};

use crate::asr_bridge::{
    AsrBridge, BridgeAsrEvent, BridgeAsrSessionController, BridgeAsrSessionHandle, BridgeCommand,
    StdioAsrBridge, StdioAsrBridgeConfig,
};
use crate::bridge_support::BridgeError;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EmptyPayload {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentHelloPayload {
    pub agent_id: String,
    pub agent_name: String,
    pub provider_id: String,
    pub capabilities: Vec<String>,
    pub shared_secret: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentHelloOkPayload {
    pub server_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentAudioPayload {
    pub data_base64: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentFinalPayload {
    pub transcript: Transcript,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentPartialPayload {
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentSessionStartedPayload {
    pub provider_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentSessionEndedPayload {
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentErrorPayload {
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum AgentToGatewayMessage {
    #[serde(rename = "agent.hello")]
    Hello { payload: AgentHelloPayload },
    #[serde(rename = "session.started")]
    SessionStarted {
        session_id: SessionId,
        payload: AgentSessionStartedPayload,
    },
    #[serde(rename = "asr.partial")]
    AsrPartial {
        session_id: SessionId,
        payload: AgentPartialPayload,
    },
    #[serde(rename = "asr.final")]
    AsrFinal {
        session_id: SessionId,
        payload: AgentFinalPayload,
    },
    #[serde(rename = "session.ended")]
    SessionEnded {
        session_id: SessionId,
        payload: AgentSessionEndedPayload,
    },
    #[serde(rename = "error")]
    Error {
        session_id: Option<SessionId>,
        payload: AgentErrorPayload,
    },
    #[serde(rename = "pong")]
    Pong { payload: EmptyPayload },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum GatewayToAgentMessage {
    #[serde(rename = "agent.hello.ok")]
    HelloOk { payload: AgentHelloOkPayload },
    #[serde(rename = "session.start")]
    SessionStart {
        session_id: SessionId,
        payload: StreamRequest,
    },
    #[serde(rename = "session.audio")]
    SessionAudio {
        session_id: SessionId,
        payload: AgentAudioPayload,
    },
    #[serde(rename = "session.commit")]
    SessionCommit {
        session_id: SessionId,
        payload: EmptyPayload,
    },
    #[serde(rename = "session.stop")]
    SessionStop {
        session_id: SessionId,
        payload: EmptyPayload,
    },
    #[serde(rename = "ping")]
    Ping { payload: EmptyPayload },
}

#[derive(Debug, Clone)]
struct RegisteredAgent {
    agent_id: String,
    provider_id: String,
    command_tx: mpsc::Sender<GatewayToAgentMessage>,
}

struct SessionRoute {
    agent_id: String,
    event_tx: mpsc::Sender<BridgeAsrEvent>,
    started_tx: Option<oneshot::Sender<Result<(), BridgeError>>>,
}

#[derive(Default)]
struct AgentRegistryInner {
    agents: HashMap<String, RegisteredAgent>,
    sessions: HashMap<SessionId, SessionRoute>,
}

#[derive(Clone)]
pub struct AgentRegistry {
    expected_shared_secret: Option<String>,
    inner: Arc<Mutex<AgentRegistryInner>>,
}

fn remove_agent_locked(
    inner: &mut AgentRegistryInner,
    agent_id: &str,
) -> Vec<(SessionId, SessionRoute)> {
    inner.agents.remove(agent_id);
    let impacted_ids: Vec<SessionId> = inner
        .sessions
        .iter()
        .filter_map(|(session_id, route)| {
            if route.agent_id == agent_id {
                Some(session_id.clone())
            } else {
                None
            }
        })
        .collect();
    let mut impacted = Vec::new();
    for session_id in impacted_ids {
        if let Some(route) = inner.sessions.remove(&session_id) {
            impacted.push((session_id, route));
        }
    }
    impacted
}

impl AgentRegistry {
    pub fn new(expected_shared_secret: Option<String>) -> Self {
        Self {
            expected_shared_secret,
            inner: Arc::new(Mutex::new(AgentRegistryInner::default())),
        }
    }

    pub async fn register_agent(
        &self,
        hello: AgentHelloPayload,
        command_tx: mpsc::Sender<GatewayToAgentMessage>,
    ) -> Result<(), BridgeError> {
        if let Some(expected) = self.expected_shared_secret.as_deref() {
            if hello.shared_secret.as_deref() != Some(expected) {
                return Err(BridgeError::Unavailable(
                    "agent authentication failed".to_string(),
                ));
            }
        }

        let orphaned_sessions = {
            let mut inner = self.inner.lock().await;
            let orphaned = if inner.agents.contains_key(&hello.agent_id) {
                warn!(
                    "replacing existing agent registration for {}",
                    hello.agent_id
                );
                remove_agent_locked(&mut inner, &hello.agent_id)
            } else {
                Vec::new()
            };
            inner.agents.insert(
                hello.agent_id.clone(),
                RegisteredAgent {
                    agent_id: hello.agent_id,
                    provider_id: hello.provider_id,
                    command_tx,
                },
            );
            orphaned
        };
        self.finish_agent_removal(orphaned_sessions).await;
        Ok(())
    }

    pub async fn unregister_agent(&self, agent_id: &str) {
        let orphaned_sessions = {
            let mut inner = self.inner.lock().await;
            remove_agent_locked(&mut inner, agent_id)
        };

        self.finish_agent_removal(orphaned_sessions).await;
    }

    async fn finish_agent_removal(&self, orphaned_sessions: Vec<(SessionId, SessionRoute)>) {
        for (session_id, mut route) in orphaned_sessions {
            if let Some(started_tx) = route.started_tx.take() {
                let _ = started_tx.send(Err(BridgeError::Disconnected(format!(
                    "agent disconnected before session started: {session_id:?}"
                ))));
            } else {
                let _ = route
                    .event_tx
                    .send(BridgeAsrEvent::Error {
                        message: format!("agent disconnected for session {session_id:?}"),
                    })
                    .await;
                let _ = route
                    .event_tx
                    .send(BridgeAsrEvent::Ended {
                        reason: Some("agent_disconnected".to_string()),
                    })
                    .await;
            }
        }
    }

    async fn select_agent(&self, provider_id: &str) -> Option<RegisteredAgent> {
        let inner = self.inner.lock().await;
        inner
            .agents
            .values()
            .find(|agent| agent.provider_id == provider_id)
            .cloned()
    }

    async fn register_session(
        &self,
        agent_id: String,
        session_id: SessionId,
        event_tx: mpsc::Sender<BridgeAsrEvent>,
        started_tx: oneshot::Sender<Result<(), BridgeError>>,
    ) {
        let mut inner = self.inner.lock().await;
        inner.sessions.insert(
            session_id,
            SessionRoute {
                agent_id,
                event_tx,
                started_tx: Some(started_tx),
            },
        );
    }

    async fn remove_session(&self, session_id: &SessionId) -> Option<SessionRoute> {
        let mut inner = self.inner.lock().await;
        inner.sessions.remove(session_id)
    }

    async fn mark_session_started(&self, session_id: &SessionId) {
        let started_tx = {
            let mut inner = self.inner.lock().await;
            inner
                .sessions
                .get_mut(session_id)
                .and_then(|route| route.started_tx.take())
        };
        if let Some(started_tx) = started_tx {
            let _ = started_tx.send(Ok(()));
        }
    }

    async fn fail_session_start(&self, session_id: &SessionId, error: BridgeError) {
        if let Some(mut route) = self.remove_session(session_id).await {
            if let Some(started_tx) = route.started_tx.take() {
                let _ = started_tx.send(Err(error));
            } else {
                let _ = route
                    .event_tx
                    .send(BridgeAsrEvent::Error {
                        message: "session failed before start acknowledgement".to_string(),
                    })
                    .await;
                let _ = route
                    .event_tx
                    .send(BridgeAsrEvent::Ended {
                        reason: Some("start_failed".to_string()),
                    })
                    .await;
            }
        }
    }

    async fn route_event(&self, session_id: &SessionId, event: BridgeAsrEvent) {
        let event_tx = {
            let inner = self.inner.lock().await;
            inner
                .sessions
                .get(session_id)
                .map(|route| route.event_tx.clone())
        };
        if let Some(event_tx) = event_tx {
            let _ = event_tx.send(event).await;
        }
    }

    async fn route_session_ended(&self, session_id: &SessionId, reason: Option<String>) {
        if let Some(mut route) = self.remove_session(session_id).await {
            if let Some(started_tx) = route.started_tx.take() {
                let _ = started_tx.send(Err(BridgeError::Disconnected(
                    reason.unwrap_or_else(|| "session ended before start ack".to_string()),
                )));
            } else {
                let _ = route.event_tx.send(BridgeAsrEvent::Ended { reason }).await;
            }
        }
    }

    pub async fn handle_agent_message(&self, message: AgentToGatewayMessage) {
        match message {
            AgentToGatewayMessage::Hello { .. } => {
                warn!("unexpected agent.hello after registration");
            }
            AgentToGatewayMessage::SessionStarted { session_id, .. } => {
                self.mark_session_started(&session_id).await;
            }
            AgentToGatewayMessage::AsrPartial {
                session_id,
                payload,
            } => {
                self.route_event(&session_id, BridgeAsrEvent::Partial { text: payload.text })
                    .await;
            }
            AgentToGatewayMessage::AsrFinal {
                session_id,
                payload,
            } => {
                self.route_event(
                    &session_id,
                    BridgeAsrEvent::Final {
                        transcript: payload.transcript,
                    },
                )
                .await;
            }
            AgentToGatewayMessage::SessionEnded {
                session_id,
                payload,
            } => {
                self.route_session_ended(&session_id, payload.reason).await;
            }
            AgentToGatewayMessage::Error {
                session_id: Some(session_id),
                payload,
            } => {
                let has_pending_start = {
                    let inner = self.inner.lock().await;
                    inner
                        .sessions
                        .get(&session_id)
                        .and_then(|route| route.started_tx.as_ref())
                        .is_some()
                };
                if has_pending_start {
                    self.fail_session_start(&session_id, BridgeError::Unavailable(payload.message))
                        .await;
                } else {
                    self.route_event(
                        &session_id,
                        BridgeAsrEvent::Error {
                            message: payload.message,
                        },
                    )
                    .await;
                }
            }
            AgentToGatewayMessage::Error {
                session_id: None,
                payload,
            } => {
                warn!("agent sent global error: {}", payload.message);
            }
            AgentToGatewayMessage::Pong { .. } => {
                debug!("agent pong received");
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct RemoteAgentAsrBridgeConfig {
    pub provider_id: String,
    pub display_name: Option<String>,
    pub start_timeout: Duration,
}

pub struct RemoteAgentAsrBridge {
    config: RemoteAgentAsrBridgeConfig,
    registry: AgentRegistry,
}

impl RemoteAgentAsrBridge {
    pub fn new(config: RemoteAgentAsrBridgeConfig, registry: AgentRegistry) -> Self {
        Self { config, registry }
    }
}

#[async_trait]
impl AsrBridge for RemoteAgentAsrBridge {
    fn descriptors(&self) -> Vec<ProviderDescriptor> {
        vec![
            ProviderDescriptor::new(
                self.config.provider_id.clone(),
                self.config
                    .display_name
                    .clone()
                    .unwrap_or_else(|| "Remote Agent ASR Bridge".to_string()),
                CapabilityDomain::Asr,
                RuntimeMode::RemoteGateway,
            )
            .with_capability(Capability::enabled("streaming-input"))
            .with_capability(Capability::enabled("buffered-input"))
            .with_capability(Capability::enabled("streaming-output"))
            .with_capability(Capability::enabled("buffered-output"))
            .with_capability(Capability::enabled("on-device"))
            .with_capability(Capability::enabled("agent-backhaul")),
        ]
    }

    async fn start_stream(
        &self,
        request: StreamRequest,
    ) -> Result<BridgeAsrSessionHandle, BridgeError> {
        if let Some(provider_id) = request.provider.provider_id.as_deref() {
            if provider_id != self.config.provider_id {
                return Err(BridgeError::Unavailable(format!(
                    "requested provider {provider_id} is not available on this gateway"
                )));
            }
        }

        let agent = self
            .registry
            .select_agent(&self.config.provider_id)
            .await
            .ok_or_else(|| {
                BridgeError::Unavailable(format!(
                    "no registered agent available for provider {}",
                    self.config.provider_id
                ))
            })?;

        let session = AsrSession {
            id: SessionId::new(),
            provider_id: self.config.provider_id.clone(),
            accepted_input_format: request.input_format.clone(),
            input_mode: request
                .options
                .provider_options
                .get("input_mode")
                .and_then(serde_json::Value::as_str)
                .map(|value| value.trim().to_ascii_lowercase())
                .filter(|value| value == "buffered")
                .map(|_| StreamMode::Buffered)
                .unwrap_or(StreamMode::Streaming),
            output_mode: if request
                .options
                .provider_options
                .get("output_mode")
                .and_then(serde_json::Value::as_str)
                .map(|value| value.trim().to_ascii_lowercase())
                .as_deref()
                == Some("streaming")
                || request.options.interim_results
            {
                StreamMode::Streaming
            } else {
                StreamMode::Buffered
            },
        };
        let (command_tx, command_rx) = mpsc::channel::<BridgeCommand>(64);
        let (event_tx, event_rx) = mpsc::channel::<BridgeAsrEvent>(64);
        let (started_tx, started_rx) = oneshot::channel();

        self.registry
            .register_session(
                agent.agent_id.clone(),
                session.id.clone(),
                event_tx.clone(),
                started_tx,
            )
            .await;

        if agent
            .command_tx
            .send(GatewayToAgentMessage::SessionStart {
                session_id: session.id.clone(),
                payload: request,
            })
            .await
            .is_err()
        {
            self.registry
                .fail_session_start(
                    &session.id,
                    BridgeError::Disconnected("agent command channel closed".to_string()),
                )
                .await;
            return Err(BridgeError::Disconnected(
                "agent command channel closed".to_string(),
            ));
        }

        tokio::spawn(forward_commands_to_agent(
            session.id.clone(),
            agent.command_tx.clone(),
            command_rx,
            event_tx,
        ));

        match timeout(self.config.start_timeout, started_rx).await {
            Ok(Ok(Ok(()))) => Ok(BridgeAsrSessionHandle::new(session, command_tx, event_rx)),
            Ok(Ok(Err(error))) => Err(error),
            Ok(Err(_)) => {
                self.registry
                    .fail_session_start(
                        &session.id,
                        BridgeError::Disconnected("session start acknowledgement lost".to_string()),
                    )
                    .await;
                Err(BridgeError::Disconnected(
                    "session start acknowledgement lost".to_string(),
                ))
            }
            Err(_) => {
                self.registry
                    .fail_session_start(
                        &session.id,
                        BridgeError::Unavailable(
                            "timed out waiting for agent session start".to_string(),
                        ),
                    )
                    .await;
                Err(BridgeError::Unavailable(
                    "timed out waiting for agent session start".to_string(),
                ))
            }
        }
    }
}

async fn forward_commands_to_agent(
    session_id: SessionId,
    agent_tx: mpsc::Sender<GatewayToAgentMessage>,
    mut command_rx: mpsc::Receiver<BridgeCommand>,
    event_tx: mpsc::Sender<BridgeAsrEvent>,
) {
    while let Some(command) = command_rx.recv().await {
        let outbound = match command {
            BridgeCommand::PushAudio(chunk) => GatewayToAgentMessage::SessionAudio {
                session_id: session_id.clone(),
                payload: AgentAudioPayload {
                    data_base64: BASE64_STANDARD.encode(chunk),
                },
            },
            BridgeCommand::Commit => GatewayToAgentMessage::SessionCommit {
                session_id: session_id.clone(),
                payload: EmptyPayload {},
            },
            BridgeCommand::Stop => GatewayToAgentMessage::SessionStop {
                session_id: session_id.clone(),
                payload: EmptyPayload {},
            },
        };
        if agent_tx.send(outbound).await.is_err() {
            let _ = event_tx
                .send(BridgeAsrEvent::Error {
                    message: "agent command channel closed".to_string(),
                })
                .await;
            let _ = event_tx
                .send(BridgeAsrEvent::Ended {
                    reason: Some("agent_disconnected".to_string()),
                })
                .await;
            return;
        }
    }
}

pub async fn handle_agent_connection(
    websocket: WebSocketStream<tokio::net::TcpStream>,
    peer: std::net::SocketAddr,
    registry: AgentRegistry,
    server_name: String,
) -> Result<(), BridgeError> {
    let (mut sink, mut source) = websocket.split();
    let first_frame = source
        .next()
        .await
        .ok_or_else(|| BridgeError::Protocol("agent closed before hello".to_string()))
        .and_then(|frame| {
            frame.map_err(|error| BridgeError::Io(format!("read agent hello failed: {error}")))
        })?;

    let hello = match first_frame {
        Message::Text(text) => serde_json::from_str::<AgentToGatewayMessage>(&text)
            .map_err(|error| BridgeError::Protocol(format!("invalid agent hello: {error}")))?,
        other => {
            return Err(BridgeError::Protocol(format!(
                "agent hello must be text frame, got {other:?}"
            )));
        }
    };

    let hello_payload = match hello {
        AgentToGatewayMessage::Hello { payload } => payload,
        other => {
            return Err(BridgeError::Protocol(format!(
                "expected agent.hello first, got {other:?}"
            )));
        }
    };

    let agent_id = hello_payload.agent_id.clone();
    let (command_tx, mut command_rx) = mpsc::channel::<GatewayToAgentMessage>(64);
    registry
        .register_agent(hello_payload, command_tx.clone())
        .await?;
    info!("agent {agent_id} connected from {peer}");

    let writer = tokio::spawn(async move {
        while let Some(message) = command_rx.recv().await {
            let encoded = serde_json::to_string(&message).map_err(|error| {
                BridgeError::Protocol(format!("encode agent message failed: {error}"))
            })?;
            sink.send(Message::Text(encoded.into()))
                .await
                .map_err(|error| BridgeError::Io(format!("write agent frame failed: {error}")))?;
        }
        Result::<(), BridgeError>::Ok(())
    });

    command_tx
        .send(GatewayToAgentMessage::HelloOk {
            payload: AgentHelloOkPayload { server_name },
        })
        .await
        .map_err(|_| BridgeError::Disconnected("agent writer closed".to_string()))?;

    while let Some(frame) = source.next().await {
        let frame =
            frame.map_err(|error| BridgeError::Io(format!("read agent frame failed: {error}")))?;
        match frame {
            Message::Text(text) => {
                let message =
                    serde_json::from_str::<AgentToGatewayMessage>(&text).map_err(|error| {
                        BridgeError::Protocol(format!("invalid agent frame payload: {error}"))
                    })?;
                registry.handle_agent_message(message).await;
            }
            Message::Ping(payload) => {
                let _ = command_tx
                    .send(GatewayToAgentMessage::Ping {
                        payload: EmptyPayload {},
                    })
                    .await;
                debug!("agent ping from {peer} bytes={}", payload.len());
            }
            Message::Close(_) => break,
            Message::Pong(_) | Message::Binary(_) | Message::Frame(_) => {}
        }
    }

    registry.unregister_agent(&agent_id).await;
    drop(command_tx);
    match writer.await {
        Ok(Ok(())) => {}
        Ok(Err(error)) => warn!("agent writer failed for {agent_id}: {error}"),
        Err(error) => warn!("agent writer join failed for {agent_id}: {error}"),
    }
    info!("agent {agent_id} disconnected from {peer}");
    Ok(())
}

#[derive(Debug, Clone)]
pub struct LocalAgentConfig {
    pub gateway_url: String,
    pub agent_id: String,
    pub agent_name: String,
    pub provider_id: String,
    pub shared_secret: Option<String>,
    pub bridge_command: String,
    pub bridge_args: Vec<String>,
    pub reconnect_delay: Duration,
}

pub async fn run_local_agent(config: LocalAgentConfig) -> Result<(), BridgeError> {
    loop {
        match run_local_agent_once(&config).await {
            Ok(()) => {
                warn!(
                    "gateway connection ended; reconnecting agent {}",
                    config.agent_id
                );
            }
            Err(error) => {
                warn!("agent {} connection failed: {}", config.agent_id, error);
            }
        }
        tokio::time::sleep(config.reconnect_delay).await;
    }
}

async fn run_local_agent_once(config: &LocalAgentConfig) -> Result<(), BridgeError> {
    let (websocket, response) = connect_async(&config.gateway_url)
        .await
        .map_err(|error| BridgeError::Unavailable(format!("connect gateway failed: {error}")))?;
    info!(
        "apple agent {} connected to gateway status={}",
        config.agent_id,
        response.status()
    );

    let bridge = Arc::new(StdioAsrBridge::new(StdioAsrBridgeConfig {
        provider_id: config.provider_id.clone(),
        display_name: None,
        command: config.bridge_command.clone(),
        args: config.bridge_args.clone(),
    }));
    let sessions: Arc<Mutex<HashMap<SessionId, BridgeAsrSessionController>>> =
        Arc::new(Mutex::new(HashMap::new()));

    let (mut sink, mut source) = websocket.split();
    let (out_tx, mut out_rx) = mpsc::channel::<AgentToGatewayMessage>(64);
    let writer = tokio::spawn(async move {
        while let Some(message) = out_rx.recv().await {
            let encoded = serde_json::to_string(&message).map_err(|error| {
                BridgeError::Protocol(format!("encode outbound agent frame failed: {error}"))
            })?;
            sink.send(Message::Text(encoded.into()))
                .await
                .map_err(|error| {
                    BridgeError::Io(format!("write outbound agent frame failed: {error}"))
                })?;
        }
        Result::<(), BridgeError>::Ok(())
    });

    out_tx
        .send(AgentToGatewayMessage::Hello {
            payload: AgentHelloPayload {
                agent_id: config.agent_id.clone(),
                agent_name: config.agent_name.clone(),
                provider_id: config.provider_id.clone(),
                capabilities: vec!["streaming-input".to_string(), "interim-results".to_string()],
                shared_secret: config.shared_secret.clone(),
            },
        })
        .await
        .map_err(|_| BridgeError::Disconnected("agent writer channel closed".to_string()))?;

    loop {
        let frame = source
            .next()
            .await
            .ok_or_else(|| BridgeError::Disconnected("gateway closed connection".to_string()))?
            .map_err(|error| BridgeError::Io(format!("read gateway frame failed: {error}")))?;

        match frame {
            Message::Text(text) => {
                let message =
                    serde_json::from_str::<GatewayToAgentMessage>(&text).map_err(|error| {
                        BridgeError::Protocol(format!("invalid gateway frame payload: {error}"))
                    })?;
                match message {
                    GatewayToAgentMessage::HelloOk { .. } => break,
                    other => {
                        warn!("ignoring pre-hello gateway frame: {other:?}");
                    }
                }
            }
            Message::Close(_) => {
                return Err(BridgeError::Disconnected(
                    "gateway closed during hello handshake".to_string(),
                ));
            }
            _ => {}
        }
    }

    while let Some(frame) = source.next().await {
        let frame = frame
            .map_err(|error| BridgeError::Io(format!("read gateway frame failed: {error}")))?;
        match frame {
            Message::Text(text) => {
                let message =
                    serde_json::from_str::<GatewayToAgentMessage>(&text).map_err(|error| {
                        BridgeError::Protocol(format!("invalid gateway frame payload: {error}"))
                    })?;
                handle_gateway_message(message, bridge.clone(), sessions.clone(), out_tx.clone())
                    .await;
            }
            Message::Close(_) => break,
            Message::Ping(_) => {
                let _ = out_tx
                    .send(AgentToGatewayMessage::Pong {
                        payload: EmptyPayload {},
                    })
                    .await;
            }
            Message::Binary(_) | Message::Pong(_) | Message::Frame(_) => {}
        }
    }

    drop(out_tx);
    let _ = writer.await;
    Err(BridgeError::Disconnected(
        "gateway connection closed".to_string(),
    ))
}

async fn handle_gateway_message(
    message: GatewayToAgentMessage,
    bridge: Arc<StdioAsrBridge>,
    sessions: Arc<Mutex<HashMap<SessionId, BridgeAsrSessionController>>>,
    out_tx: mpsc::Sender<AgentToGatewayMessage>,
) {
    match message {
        GatewayToAgentMessage::HelloOk { .. } => {}
        GatewayToAgentMessage::SessionStart {
            session_id,
            payload,
        } => match bridge.start_stream(payload).await {
            Ok(mut handle) => {
                let controller = handle.controller();
                let event_rx = handle.take_event_rx();
                sessions.lock().await.insert(session_id.clone(), controller);
                let _ = out_tx
                    .send(AgentToGatewayMessage::SessionStarted {
                        session_id: session_id.clone(),
                        payload: AgentSessionStartedPayload {
                            provider_id: handle.session.provider_id.clone(),
                        },
                    })
                    .await;
                if let Some(mut event_rx) = event_rx {
                    let out = out_tx.clone();
                    let session_map = sessions.clone();
                    tokio::spawn(async move {
                        while let Some(event) = event_rx.recv().await {
                            let message = match event {
                                BridgeAsrEvent::Partial { text } => {
                                    AgentToGatewayMessage::AsrPartial {
                                        session_id: session_id.clone(),
                                        payload: AgentPartialPayload { text },
                                    }
                                }
                                BridgeAsrEvent::Final { transcript } => {
                                    AgentToGatewayMessage::AsrFinal {
                                        session_id: session_id.clone(),
                                        payload: AgentFinalPayload { transcript },
                                    }
                                }
                                BridgeAsrEvent::Ended { reason } => {
                                    let _ = session_map.lock().await.remove(&session_id);
                                    AgentToGatewayMessage::SessionEnded {
                                        session_id: session_id.clone(),
                                        payload: AgentSessionEndedPayload { reason },
                                    }
                                }
                                BridgeAsrEvent::Error { message } => AgentToGatewayMessage::Error {
                                    session_id: Some(session_id.clone()),
                                    payload: AgentErrorPayload { message },
                                },
                            };
                            if out.send(message).await.is_err() {
                                break;
                            }
                        }
                        let _ = session_map.lock().await.remove(&session_id);
                    });
                }
            }
            Err(error) => {
                let _ = out_tx
                    .send(AgentToGatewayMessage::Error {
                        session_id: Some(session_id),
                        payload: AgentErrorPayload {
                            message: error.to_string(),
                        },
                    })
                    .await;
            }
        },
        GatewayToAgentMessage::SessionAudio {
            session_id,
            payload,
        } => {
            let controller = { sessions.lock().await.get(&session_id).cloned() };
            match controller {
                Some(controller) => {
                    let data = match BASE64_STANDARD.decode(payload.data_base64) {
                        Ok(data) => data,
                        Err(error) => {
                            let _ = out_tx
                                .send(AgentToGatewayMessage::Error {
                                    session_id: Some(session_id),
                                    payload: AgentErrorPayload {
                                        message: format!("invalid audio base64: {error}"),
                                    },
                                })
                                .await;
                            return;
                        }
                    };
                    if let Err(error) = controller.push_audio(data).await {
                        let _ = out_tx
                            .send(AgentToGatewayMessage::Error {
                                session_id: Some(session_id),
                                payload: AgentErrorPayload {
                                    message: error.to_string(),
                                },
                            })
                            .await;
                    }
                }
                None => {
                    let _ = out_tx
                        .send(AgentToGatewayMessage::Error {
                            session_id: Some(session_id),
                            payload: AgentErrorPayload {
                                message: "session not found".to_string(),
                            },
                        })
                        .await;
                }
            }
        }
        GatewayToAgentMessage::SessionCommit { session_id, .. } => {
            if let Some(controller) = { sessions.lock().await.get(&session_id).cloned() } {
                if let Err(error) = controller.commit().await {
                    let _ = out_tx
                        .send(AgentToGatewayMessage::Error {
                            session_id: Some(session_id),
                            payload: AgentErrorPayload {
                                message: error.to_string(),
                            },
                        })
                        .await;
                }
            }
        }
        GatewayToAgentMessage::SessionStop { session_id, .. } => {
            if let Some(controller) = { sessions.lock().await.get(&session_id).cloned() } {
                if let Err(error) = controller.stop().await {
                    let _ = out_tx
                        .send(AgentToGatewayMessage::Error {
                            session_id: Some(session_id),
                            payload: AgentErrorPayload {
                                message: error.to_string(),
                            },
                        })
                        .await;
                }
            }
        }
        GatewayToAgentMessage::Ping { .. } => {
            let _ = out_tx
                .send(AgentToGatewayMessage::Pong {
                    payload: EmptyPayload {},
                })
                .await;
        }
    }
}
