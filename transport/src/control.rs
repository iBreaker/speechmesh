/// Control 协议消息定义
///
/// 定义 Control WebSocket 端点的通信协议。
/// 用于外部工具向 Gateway 发送控制指令（如播放音频、查询设备等）。

use serde::{Deserialize, Serialize};
use speechmesh_core::AudioFormat;

use crate::agent::AgentSnapshot;

/// 客户端发送给 Control 端点的请求消息
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ControlRequest {
    #[serde(rename = "play_audio")]
    PlayAudio { payload: ControlPlayAudioPayload },
    #[serde(rename = "devices.list")]
    DevicesList,
    #[serde(rename = "agent.status")]
    AgentStatus {
        payload: ControlAgentStatusPayload,
    },
}

/// play_audio 请求的负载
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ControlPlayAudioPayload {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_target: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<AudioFormat>,
    pub audio_base64: String,
    #[serde(default)]
    pub chunk_size_bytes: Option<usize>,
}

/// agent.status 请求的负载
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ControlAgentStatusPayload {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_id: Option<String>,
}

/// Control 端点返回的响应消息
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ControlResponse {
    #[serde(rename = "play_audio.accepted")]
    PlayAudioAccepted {
        payload: ControlPlayAudioAcceptedPayload,
    },
    #[serde(rename = "devices.list")]
    DevicesList {
        payload: ControlDevicesListPayload,
    },
    #[serde(rename = "agent.status")]
    AgentStatus {
        payload: ControlAgentStatusResultPayload,
    },
    #[serde(rename = "error")]
    Error { payload: ControlErrorPayload },
    #[serde(rename = "pong")]
    Pong {},
}

/// play_audio.accepted 响应的负载
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ControlPlayAudioAcceptedPayload {
    pub task_id: String,
    pub routed_agent_id: String,
    pub chunk_count: u64,
    pub total_bytes: u64,
}

/// devices.list 响应的负载
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ControlDevicesListPayload {
    pub agents: Vec<AgentSnapshot>,
}

/// agent.status 响应的负载
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ControlAgentStatusResultPayload {
    pub agent: Option<AgentSnapshot>,
}

/// 错误响应的负载
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ControlErrorPayload {
    pub message: String,
}
