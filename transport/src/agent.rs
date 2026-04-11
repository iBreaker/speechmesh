/// Agent 协议消息定义
///
/// 定义 Agent 与 Gateway 之间的 WebSocket 通信协议。
/// Agent 可以是 ASR 提供者、设备代理等。

use std::fmt;

use serde::{Deserialize, Serialize};
use speechmesh_asr::{StreamRequest, Transcript};
use speechmesh_core::{AudioFormat, CapabilityDomain, SessionId};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentEmptyPayload {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AgentKind {
    #[default]
    AsrProvider,
    Device,
}

impl fmt::Display for AgentKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AgentKind::AsrProvider => write!(f, "asr_provider"),
            AgentKind::Device => write!(f, "device"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentDeviceIdentity {
    pub device_id: String,
    #[serde(default)]
    pub hostname: Option<String>,
    #[serde(default)]
    pub platform: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentUpdateStatus {
    #[serde(default)]
    pub state: Option<String>,
    #[serde(default)]
    pub current_version: Option<String>,
    #[serde(default)]
    pub target_version: Option<String>,
    #[serde(default)]
    pub checked_at_unix_secs: Option<u64>,
    #[serde(default)]
    pub applied: Option<bool>,
    #[serde(default)]
    pub restart_performed: Option<bool>,
    #[serde(default)]
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentHelloPayload {
    pub agent_id: String,
    pub agent_name: String,
    #[serde(default)]
    pub provider_id: Option<String>,
    #[serde(default)]
    pub capabilities: Vec<String>,
    #[serde(default)]
    pub capability_domains: Vec<CapabilityDomain>,
    #[serde(default)]
    pub agent_kind: AgentKind,
    #[serde(default)]
    pub device: Option<AgentDeviceIdentity>,
    #[serde(default)]
    pub client_version: Option<String>,
    #[serde(default)]
    pub update_status: Option<AgentUpdateStatus>,
    pub shared_secret: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentHelloOkPayload {
    pub server_name: String,
}

/// Agent 快照，用于 Control API 返回已注册 agent 的信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentSnapshot {
    pub agent_id: String,
    pub agent_name: String,
    pub provider_id: Option<String>,
    pub capabilities: Vec<String>,
    pub capability_domains: Vec<CapabilityDomain>,
    pub agent_kind: AgentKind,
    pub device: Option<AgentDeviceIdentity>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub update_status: Option<AgentUpdateStatus>,
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentTaskState {
    Started,
    Finished,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentTaskStatusPayload {
    pub state: AgentTaskState,
    #[serde(default)]
    pub message: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentPlayAudioStartPayload {
    #[serde(default)]
    pub format: Option<AudioFormat>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentPlayAudioChunkPayload {
    pub data_base64: String,
}

/// Agent 发送给 Gateway 的消息
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
    #[serde(rename = "task.status")]
    TaskStatus {
        task_id: String,
        payload: AgentTaskStatusPayload,
    },
    #[serde(rename = "pong")]
    Pong { payload: AgentEmptyPayload },
}

/// Gateway 发送给 Agent 的消息
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
        payload: AgentEmptyPayload,
    },
    #[serde(rename = "session.stop")]
    SessionStop {
        session_id: SessionId,
        payload: AgentEmptyPayload,
    },
    #[serde(rename = "task.play_audio.start")]
    TaskPlayAudioStart {
        task_id: String,
        payload: AgentPlayAudioStartPayload,
    },
    #[serde(rename = "task.play_audio.chunk")]
    TaskPlayAudioChunk {
        task_id: String,
        payload: AgentPlayAudioChunkPayload,
    },
    #[serde(rename = "task.play_audio.finish")]
    TaskPlayAudioFinish {
        task_id: String,
        payload: AgentEmptyPayload,
    },
    #[serde(rename = "ping")]
    Ping { payload: AgentEmptyPayload },
}
