pub mod agent;
pub mod control;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use speechmesh_asr::{StreamRequest, TranscribeRequest};
use speechmesh_core::{
    AudioFormat, CapabilityDomain, ErrorInfo, ProviderDescriptor, ProviderSelector, RequestId,
    SessionId, StreamMode,
};
use speechmesh_tts::{StreamRequest as TtsStreamRequest, SynthesisInputKind, VoiceDescriptor};
use thiserror::Error;

pub use agent::{
    AgentDeviceIdentity, AgentEmptyPayload, AgentHelloOkPayload, AgentHelloPayload, AgentKind,
    AgentSnapshot, AgentToGatewayMessage, GatewayToAgentMessage,
};
pub use control::{
    ControlAgentStatusPayload, ControlAgentStatusResultPayload, ControlDevicesListPayload,
    ControlErrorPayload, ControlPlayAudioAcceptedPayload, ControlPlayAudioPayload,
    ControlRequest, ControlResponse,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransportKind {
    Http,
    WebSocket,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EmptyPayload {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HelloRequest {
    pub protocol_version: String,
    pub client_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HelloResponse {
    pub protocol_version: String,
    pub server_name: String,
    pub one_session_per_connection: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiscoverRequest {
    pub domains: Vec<CapabilityDomain>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiscoverResult {
    pub providers: Vec<ProviderDescriptor>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionStartedPayload {
    pub domain: CapabilityDomain,
    pub provider_id: String,
    pub accepted_input_format: Option<AudioFormat>,
    pub accepted_output_format: Option<AudioFormat>,
    pub input_mode: Option<StreamMode>,
    pub output_mode: Option<StreamMode>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AsrWordPayload {
    pub text: String,
    pub start_ms: Option<u64>,
    pub end_ms: Option<u64>,
    pub is_final: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AsrResultPayload {
    pub segment_id: u64,
    pub revision: u64,
    pub text: String,
    pub delta: Option<String>,
    pub is_final: bool,
    pub speech_final: bool,
    pub begin_time_ms: Option<u64>,
    pub end_time_ms: Option<u64>,
    pub words: Vec<AsrWordPayload>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VoiceListRequest {
    pub provider: ProviderSelector,
    pub language: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VoiceListResult {
    pub voices: Vec<VoiceDescriptor>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TtsInputAppendPayload {
    pub delta: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TtsAudioDeltaPayload {
    pub chunk_id: u64,
    pub audio_base64: String,
    pub is_final: bool,
    pub format: Option<AudioFormat>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TtsAudioDonePayload {
    pub input_kind: SynthesisInputKind,
    pub total_chunks: u64,
    pub total_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionEndedPayload {
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ErrorPayload {
    pub error: ErrorInfo,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ClientMessage {
    #[serde(rename = "hello")]
    Hello {
        request_id: Option<RequestId>,
        payload: HelloRequest,
    },
    #[serde(rename = "discover")]
    Discover {
        request_id: RequestId,
        payload: DiscoverRequest,
    },
    #[serde(rename = "asr.transcribe")]
    AsrTranscribe {
        request_id: RequestId,
        payload: TranscribeRequest,
    },
    #[serde(rename = "asr.start")]
    AsrStart {
        request_id: RequestId,
        payload: StreamRequest,
    },
    #[serde(rename = "asr.commit")]
    AsrCommit {
        session_id: SessionId,
        payload: EmptyPayload,
    },
    #[serde(rename = "tts.voices")]
    TtsVoices {
        request_id: RequestId,
        payload: VoiceListRequest,
    },
    #[serde(rename = "tts.start")]
    TtsStart {
        request_id: RequestId,
        payload: TtsStreamRequest,
    },
    #[serde(rename = "tts.input.append")]
    TtsInputAppend {
        session_id: SessionId,
        payload: TtsInputAppendPayload,
    },
    #[serde(rename = "tts.commit")]
    TtsCommit {
        session_id: SessionId,
        payload: EmptyPayload,
    },
    #[serde(rename = "session.stop")]
    SessionStop {
        session_id: SessionId,
        payload: EmptyPayload,
    },
    #[serde(rename = "session.cancel")]
    SessionCancel {
        session_id: SessionId,
        payload: EmptyPayload,
    },
    #[serde(rename = "ping")]
    Ping {
        request_id: Option<RequestId>,
        payload: EmptyPayload,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ServerMessage {
    #[serde(rename = "hello.ok")]
    HelloOk {
        request_id: Option<RequestId>,
        payload: HelloResponse,
    },
    #[serde(rename = "discover.result")]
    DiscoverResult {
        request_id: RequestId,
        payload: DiscoverResult,
    },
    #[serde(rename = "session.started")]
    SessionStarted {
        request_id: Option<RequestId>,
        session_id: SessionId,
        payload: SessionStartedPayload,
    },
    #[serde(rename = "asr.result")]
    AsrResult {
        session_id: SessionId,
        sequence: u64,
        payload: AsrResultPayload,
    },
    #[serde(rename = "tts.voices.result")]
    TtsVoicesResult {
        request_id: RequestId,
        payload: VoiceListResult,
    },
    #[serde(rename = "tts.audio.delta")]
    TtsAudioDelta {
        session_id: SessionId,
        sequence: u64,
        payload: TtsAudioDeltaPayload,
    },
    #[serde(rename = "tts.audio.done")]
    TtsAudioDone {
        session_id: SessionId,
        sequence: u64,
        payload: TtsAudioDonePayload,
    },
    #[serde(rename = "session.ended")]
    SessionEnded {
        session_id: SessionId,
        payload: SessionEndedPayload,
    },
    #[serde(rename = "error")]
    Error {
        request_id: Option<RequestId>,
        session_id: Option<SessionId>,
        payload: ErrorPayload,
    },
    #[serde(rename = "pong")]
    Pong {
        request_id: Option<RequestId>,
        payload: EmptyPayload,
    },
}

#[derive(Debug, Error)]
pub enum TransportError {
    #[error("bind failed: {0}")]
    Bind(String),
    #[error("unsupported request: {0}")]
    Unsupported(String),
}

#[async_trait]
pub trait Transport: Send + Sync {
    fn kind(&self) -> TransportKind;

    async fn bind(&self) -> Result<(), TransportError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transport_kind_is_copyable() {
        let kind = TransportKind::Http;
        assert_eq!(kind, TransportKind::Http);
    }

    #[test]
    fn websocket_messages_use_protocol_type_tags() {
        let message = ClientMessage::Ping {
            request_id: Some(RequestId::from("req_1")),
            payload: EmptyPayload::default(),
        };
        let encoded = serde_json::to_string(&message).expect("serialize message");

        assert!(encoded.contains("\"type\":\"ping\""));
        assert!(encoded.contains("\"request_id\":\"req_1\""));
    }

    #[test]
    fn tts_audio_done_uses_explicit_type_tag() {
        let message = ServerMessage::TtsAudioDone {
            session_id: SessionId::new(),
            sequence: 1,
            payload: TtsAudioDonePayload {
                input_kind: SynthesisInputKind::Text,
                total_chunks: 1,
                total_bytes: 128,
            },
        };
        let encoded = serde_json::to_string(&message).expect("serialize tts audio done");

        assert!(encoded.contains("\"type\":\"tts.audio.done\""));
        assert!(encoded.contains("\"total_bytes\":128"));
    }
}
