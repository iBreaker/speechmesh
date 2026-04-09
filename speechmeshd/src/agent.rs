use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use futures_util::{SinkExt, StreamExt};
use speechmesh_asr::{AsrSession, StreamRequest};
use speechmesh_core::{
    AudioFormat, Capability, CapabilityDomain, ProviderDescriptor, RuntimeMode, SessionId,
    StreamMode,
};
pub use speechmesh_transport::agent::*;
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

// 从 device crate 引入注册表和设备模型类型
pub use speechmesh_device::registry::{
    AgentRemovalResult, AgentSnapshot as DeviceAgentSnapshot, AgentSnapshotFilter,
    RegisteredAgent as DeviceRegisteredAgent,
};

/// 播放音频路由请求
#[derive(Debug, Clone)]
pub struct PlayAudioRouteRequest {
    pub task_id: String,
    pub device_id: Option<String>,
    pub agent_id: Option<String>,
    pub format: Option<AudioFormat>,
}

// ── 类型转换：transport 协议类型 <-> device 注册表类型 ──

/// 将 transport 层的 AgentKind 转换为 device 层的 AgentKind
fn to_device_agent_kind(
    kind: AgentKind,
) -> speechmesh_device::registry::AgentKind {
    match kind {
        AgentKind::AsrProvider => speechmesh_device::registry::AgentKind::AsrProvider,
        AgentKind::Device => speechmesh_device::registry::AgentKind::Device,
    }
}

/// 将 device 层的 AgentKind 转换为 transport 层的 AgentKind
fn from_device_agent_kind(
    kind: speechmesh_device::registry::AgentKind,
) -> AgentKind {
    match kind {
        speechmesh_device::registry::AgentKind::AsrProvider => AgentKind::AsrProvider,
        speechmesh_device::registry::AgentKind::Device => AgentKind::Device,
    }
}

/// 将 transport 层的 AgentDeviceIdentity 转换为 device 层的类型
fn to_device_identity(
    identity: &AgentDeviceIdentity,
) -> speechmesh_device::registry::AgentDeviceIdentity {
    speechmesh_device::registry::AgentDeviceIdentity {
        device_id: identity.device_id.clone(),
        hostname: identity.hostname.clone(),
        platform: identity.platform.clone(),
    }
}

/// 将 device 层的 AgentDeviceIdentity 转换为 transport 层的类型
fn from_device_identity(
    identity: &speechmesh_device::registry::AgentDeviceIdentity,
) -> AgentDeviceIdentity {
    AgentDeviceIdentity {
        device_id: identity.device_id.clone(),
        hostname: identity.hostname.clone(),
        platform: identity.platform.clone(),
    }
}

/// 将 DeviceAgentSnapshot 转换为 transport 层的 AgentSnapshot
pub fn to_transport_snapshot(snapshot: &DeviceAgentSnapshot) -> AgentSnapshot {
    AgentSnapshot {
        agent_id: snapshot.agent_id.clone(),
        agent_name: snapshot.agent_name.clone(),
        provider_id: snapshot.provider_id.clone(),
        capabilities: snapshot.capabilities.clone(),
        capability_domains: snapshot.capability_domains.clone(),
        agent_kind: from_device_agent_kind(snapshot.agent_kind),
        device: snapshot.device.as_ref().map(from_device_identity),
    }
}

// ── 会话路由（保留在 speechmeshd 中，因为依赖 BridgeAsrEvent 等协议类型）──

struct SessionRoute {
    agent_id: String,
    event_tx: mpsc::Sender<BridgeAsrEvent>,
    started_tx: Option<oneshot::Sender<Result<(), BridgeError>>>,
}

/// Agent 注册表包装器
///
/// 在 device crate 的泛型注册表之上，增加会话路由管理（因为会话路由依赖 speechmeshd 的具体类型）。
#[derive(Clone)]
pub struct AgentRegistry {
    device_registry: speechmesh_device::registry::AgentRegistry<GatewayToAgentMessage>,
    sessions: Arc<Mutex<HashMap<SessionId, SessionRoute>>>,
}

fn remove_orphaned_sessions(
    sessions: &mut HashMap<SessionId, SessionRoute>,
    agent_id: &str,
) -> Vec<(SessionId, SessionRoute)> {
    let impacted_ids: Vec<SessionId> = sessions
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
        if let Some(route) = sessions.remove(&session_id) {
            impacted.push((session_id, route));
        }
    }
    impacted
}

impl AgentRegistry {
    pub fn new(expected_shared_secret: Option<String>) -> Self {
        Self {
            device_registry:
                speechmesh_device::registry::AgentRegistry::new(expected_shared_secret),
            sessions: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub async fn register_agent(
        &self,
        hello: AgentHelloPayload,
        command_tx: mpsc::Sender<GatewayToAgentMessage>,
    ) -> Result<(), BridgeError> {
        if let Some(expected) = self.device_registry.expected_shared_secret() {
            if hello.shared_secret.as_deref() != Some(expected) {
                return Err(BridgeError::Unavailable(
                    "agent authentication failed".to_string(),
                ));
            }
        }

        // 收集会话孤儿（如果替换旧注册）
        let orphaned_sessions = {
            let mut sessions = self.sessions.lock().await;
            remove_orphaned_sessions(&mut sessions, &hello.agent_id)
        };

        // 构造 device crate 注册记录
        let agent = DeviceRegisteredAgent {
            agent_id: hello.agent_id,
            agent_name: hello.agent_name,
            provider_id: hello.provider_id,
            capabilities: hello.capabilities,
            capability_domains: hello.capability_domains,
            agent_kind: to_device_agent_kind(hello.agent_kind),
            device: hello.device.as_ref().map(to_device_identity),
            device_info: None, // 旧版 agent 没有 device_info
            command_tx,
        };

        let removal = self.device_registry.register_agent(agent).await;

        // 处理孤儿
        self.finish_agent_removal(orphaned_sessions, removal.orphaned_task_ids)
            .await;
        Ok(())
    }

    pub async fn unregister_agent(&self, agent_id: &str) {
        let orphaned_sessions = {
            let mut sessions = self.sessions.lock().await;
            remove_orphaned_sessions(&mut sessions, agent_id)
        };

        let removal = self.device_registry.unregister_agent(agent_id).await;
        self.finish_agent_removal(orphaned_sessions, removal.orphaned_task_ids)
            .await;
    }

    async fn finish_agent_removal(
        &self,
        orphaned_sessions: Vec<(SessionId, SessionRoute)>,
        orphaned_tasks: Vec<String>,
    ) {
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
        for task_id in orphaned_tasks {
            warn!("agent disconnected before task completion task_id={task_id}");
        }
    }

    async fn select_agent(
        &self,
        provider_id: &str,
    ) -> Option<DeviceRegisteredAgent<GatewayToAgentMessage>> {
        self.device_registry.select_agent(provider_id).await
    }

    async fn select_speaker_agent(
        &self,
        device_id: Option<&str>,
        agent_id: Option<&str>,
    ) -> Option<DeviceRegisteredAgent<GatewayToAgentMessage>> {
        self.device_registry
            .select_speaker_agent(device_id, agent_id)
            .await
    }

    async fn register_session(
        &self,
        agent_id: String,
        session_id: SessionId,
        event_tx: mpsc::Sender<BridgeAsrEvent>,
        started_tx: oneshot::Sender<Result<(), BridgeError>>,
    ) {
        let mut sessions = self.sessions.lock().await;
        sessions.insert(
            session_id,
            SessionRoute {
                agent_id,
                event_tx,
                started_tx: Some(started_tx),
            },
        );
    }

    async fn remove_session(&self, session_id: &SessionId) -> Option<SessionRoute> {
        let mut sessions = self.sessions.lock().await;
        sessions.remove(session_id)
    }

    async fn mark_session_started(&self, session_id: &SessionId) {
        let started_tx = {
            let mut sessions = self.sessions.lock().await;
            sessions
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
            let sessions = self.sessions.lock().await;
            sessions
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

    async fn register_task(&self, task_id: String, agent_id: String) -> Result<(), BridgeError> {
        self.device_registry
            .register_task(task_id.clone(), agent_id)
            .await
            .map_err(|msg| BridgeError::Unavailable(msg))
    }

    async fn remove_task(&self, task_id: &str) {
        self.device_registry.remove_task(task_id).await;
    }

    async fn task_agent_id(&self, task_id: &str) -> Option<String> {
        self.device_registry.task_agent_id(task_id).await
    }

    pub async fn route_play_audio_start(
        &self,
        request: PlayAudioRouteRequest,
    ) -> Result<String, BridgeError> {
        let task_id = request.task_id.trim().to_string();
        if task_id.is_empty() {
            return Err(BridgeError::Unavailable(
                "task_id is required for play_audio routing".to_string(),
            ));
        }
        let agent = self
            .select_speaker_agent(request.device_id.as_deref(), request.agent_id.as_deref())
            .await
            .ok_or_else(|| {
                BridgeError::Unavailable(
                    "no registered speaker agent matches the play_audio route".to_string(),
                )
            })?;
        self.register_task(task_id.clone(), agent.agent_id.clone())
            .await?;
        let send_result = agent
            .command_tx
            .send(GatewayToAgentMessage::TaskPlayAudioStart {
                task_id: task_id.clone(),
                payload: AgentPlayAudioStartPayload {
                    format: request.format,
                },
            })
            .await;
        if send_result.is_err() {
            self.remove_task(&task_id).await;
            return Err(BridgeError::Disconnected(format!(
                "failed to deliver play_audio start to agent {}",
                agent.agent_name
            )));
        }
        Ok(agent.agent_id)
    }

    pub async fn route_play_audio_chunk(
        &self,
        task_id: &str,
        data_base64: String,
    ) -> Result<(), BridgeError> {
        let agent_id = self.task_agent_id(task_id).await.ok_or_else(|| {
            BridgeError::Unavailable(format!("unknown play_audio task_id {task_id}"))
        })?;
        let command_tx = self.device_registry.agent_command_tx(&agent_id).await;
        let Some(command_tx) = command_tx else {
            self.remove_task(task_id).await;
            return Err(BridgeError::Disconnected(format!(
                "agent {agent_id} is no longer available for task {task_id}"
            )));
        };
        if command_tx
            .send(GatewayToAgentMessage::TaskPlayAudioChunk {
                task_id: task_id.to_string(),
                payload: AgentPlayAudioChunkPayload { data_base64 },
            })
            .await
            .is_err()
        {
            self.remove_task(task_id).await;
            return Err(BridgeError::Disconnected(format!(
                "failed to deliver play_audio chunk for task {task_id}"
            )));
        }
        Ok(())
    }

    pub async fn route_play_audio_finish(&self, task_id: &str) -> Result<(), BridgeError> {
        let agent_id = self.task_agent_id(task_id).await.ok_or_else(|| {
            BridgeError::Unavailable(format!("unknown play_audio task_id {task_id}"))
        })?;
        let command_tx = self.device_registry.agent_command_tx(&agent_id).await;
        let Some(command_tx) = command_tx else {
            self.remove_task(task_id).await;
            return Err(BridgeError::Disconnected(format!(
                "agent {agent_id} is no longer available for task {task_id}"
            )));
        };
        if command_tx
            .send(GatewayToAgentMessage::TaskPlayAudioFinish {
                task_id: task_id.to_string(),
                payload: AgentEmptyPayload {},
            })
            .await
            .is_err()
        {
            self.remove_task(task_id).await;
            return Err(BridgeError::Disconnected(format!(
                "failed to deliver play_audio finish for task {task_id}"
            )));
        }
        Ok(())
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
                    let sessions = self.sessions.lock().await;
                    sessions
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
            AgentToGatewayMessage::TaskStatus { task_id, payload } => {
                let route = self.task_agent_id(&task_id).await;
                if route.is_none() {
                    warn!(
                        "agent task status for unknown task_id={task_id} state={:?} message={:?}",
                        payload.state, payload.message
                    );
                    return;
                }
                debug!(
                    "agent task status update task_id={task_id} state={:?} message={:?}",
                    payload.state, payload.message
                );
                if matches!(
                    payload.state,
                    AgentTaskState::Finished | AgentTaskState::Failed
                ) {
                    self.remove_task(&task_id).await;
                }
            }
            AgentToGatewayMessage::Pong { .. } => {
                debug!("agent pong received");
            }
        }
    }

    pub async fn snapshot(&self, filter: AgentSnapshotFilter) -> Vec<AgentSnapshot> {
        let device_snapshots = self.device_registry.snapshot(filter).await;
        device_snapshots.iter().map(to_transport_snapshot).collect()
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
                payload: AgentEmptyPayload {},
            },
            BridgeCommand::Stop => GatewayToAgentMessage::SessionStop {
                session_id: session_id.clone(),
                payload: AgentEmptyPayload {},
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
    let agent_kind = hello_payload.agent_kind;
    let provider_id = hello_payload.provider_id.clone();
    let capability_count = hello_payload.capabilities.len();
    let domain_count = hello_payload.capability_domains.len();
    let device_id = hello_payload
        .device
        .as_ref()
        .map(|device| device.device_id.clone());
    let (command_tx, mut command_rx) = mpsc::channel::<GatewayToAgentMessage>(64);
    registry
        .register_agent(hello_payload, command_tx.clone())
        .await?;
    info!(
        "agent {agent_id} connected from {peer} kind={agent_kind:?} provider_id={provider_id:?} capabilities={capability_count} capability_domains={domain_count} device_id={device_id:?}"
    );

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
                        payload: AgentEmptyPayload {},
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
        "local ASR agent {} connected to gateway status={}",
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
                provider_id: Some(config.provider_id.clone()),
                capabilities: vec!["streaming-input".to_string(), "interim-results".to_string()],
                capability_domains: vec![CapabilityDomain::Asr],
                agent_kind: AgentKind::AsrProvider,
                device: Some(AgentDeviceIdentity {
                    device_id: std::env::var("HOSTNAME")
                        .ok()
                        .filter(|value| !value.trim().is_empty())
                        .unwrap_or_else(|| config.agent_id.clone()),
                    hostname: std::env::var("HOSTNAME")
                        .ok()
                        .filter(|value| !value.trim().is_empty()),
                    platform: Some(format!(
                        "{}-{}",
                        std::env::consts::OS,
                        std::env::consts::ARCH
                    )),
                }),
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
                        payload: AgentEmptyPayload {},
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
        GatewayToAgentMessage::TaskPlayAudioStart { task_id, .. }
        | GatewayToAgentMessage::TaskPlayAudioChunk { task_id, .. }
        | GatewayToAgentMessage::TaskPlayAudioFinish { task_id, .. } => {
            let _ = out_tx
                .send(AgentToGatewayMessage::TaskStatus {
                    task_id,
                    payload: AgentTaskStatusPayload {
                        state: AgentTaskState::Failed,
                        message: Some(
                            "play_audio tasks are not supported by this ASR-only local agent"
                                .to_string(),
                        ),
                    },
                })
                .await;
        }
        GatewayToAgentMessage::Ping { .. } => {
            let _ = out_tx
                .send(AgentToGatewayMessage::Pong {
                    payload: AgentEmptyPayload {},
                })
                .await;
        }
    }
}
