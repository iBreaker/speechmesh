use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use speechmesh_core::{
    AudioFormat, EventEnvelope, ProviderDescriptor, ProviderSelector, SessionId,
};
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SynthesisInput {
    Text(String),
    Ssml(String),
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SynthesisOptions {
    pub language: Option<String>,
    pub voice: Option<String>,
    pub stream: bool,
    pub rate: Option<f32>,
    pub pitch: Option<f32>,
    pub volume: Option<f32>,
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub provider_options: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SynthesisRequest {
    pub provider: ProviderSelector,
    pub input: SynthesisInput,
    pub output_format: Option<AudioFormat>,
    pub options: SynthesisOptions,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VoiceDescriptor {
    pub id: String,
    pub language: String,
    pub display_name: String,
    pub gender: Option<String>,
    pub capabilities: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AudioChunk {
    pub bytes: Vec<u8>,
    pub sequence: u64,
    pub is_final: bool,
    pub format: Option<AudioFormat>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TtsSession {
    pub id: SessionId,
    pub provider_id: String,
    pub accepted_output_format: Option<AudioFormat>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TtsEvent {
    Audio(AudioChunk),
    Ended { reason: Option<String> },
}

pub type TtsEventEnvelope = EventEnvelope<TtsEvent>;

#[derive(Debug, Error)]
pub enum TtsError {
    #[error("provider error: {0}")]
    Provider(String),
    #[error("unsupported capability: {0}")]
    UnsupportedCapability(String),
    #[error("invalid request: {0}")]
    InvalidRequest(String),
}

#[async_trait]
pub trait TtsProvider: Send + Sync {
    fn descriptor(&self) -> ProviderDescriptor;

    async fn list_voices(&self) -> Result<Vec<VoiceDescriptor>, TtsError>;

    async fn synthesize(&self, request: SynthesisRequest) -> Result<TtsSession, TtsError>;

    async fn stop(&self, session_id: SessionId) -> Result<(), TtsError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn synthesis_request_defaults_to_non_streaming() {
        let options = SynthesisOptions::default();
        assert!(!options.stream);
        assert!(options.provider_options.is_null());
    }
}
