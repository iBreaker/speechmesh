use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use speechmesh_core::{
    AudioFormat, EventEnvelope, ProviderDescriptor, ProviderSelector, SessionId,
};
use std::path::PathBuf;
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AudioInput {
    File(PathBuf),
    Bytes(Vec<u8>),
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RecognitionOptions {
    pub language: Option<String>,
    pub hints: Vec<String>,
    pub interim_results: bool,
    pub timestamps: bool,
    pub punctuation: bool,
    pub prefer_on_device: bool,
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub provider_options: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TranscribeRequest {
    pub provider: ProviderSelector,
    pub audio: AudioInput,
    pub audio_format: Option<AudioFormat>,
    pub options: RecognitionOptions,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StreamRequest {
    pub provider: ProviderSelector,
    pub input_format: AudioFormat,
    pub options: RecognitionOptions,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TranscriptSegment {
    pub text: String,
    pub is_final: bool,
    pub start_ms: Option<u64>,
    pub end_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Transcript {
    pub text: String,
    pub language: Option<String>,
    pub segments: Vec<TranscriptSegment>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AsrSession {
    pub id: SessionId,
    pub provider_id: String,
    pub accepted_input_format: AudioFormat,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AsrEvent {
    Partial { text: String },
    Final { transcript: Transcript },
    Ended { reason: Option<String> },
}

pub type AsrEventEnvelope = EventEnvelope<AsrEvent>;

#[derive(Debug, Error)]
pub enum AsrError {
    #[error("provider error: {0}")]
    Provider(String),
    #[error("unsupported capability: {0}")]
    UnsupportedCapability(String),
    #[error("invalid request: {0}")]
    InvalidRequest(String),
}

#[async_trait]
pub trait AsrProvider: Send + Sync {
    fn descriptor(&self) -> ProviderDescriptor;

    async fn transcribe(&self, request: TranscribeRequest) -> Result<Transcript, AsrError>;

    async fn start_stream(&self, request: StreamRequest) -> Result<AsrSession, AsrError>;

    async fn stop_stream(&self, session_id: SessionId) -> Result<(), AsrError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognition_options_default_to_auto_safe_values() {
        let options = RecognitionOptions::default();
        assert!(options.hints.is_empty());
        assert!(!options.interim_results);
        assert!(!options.timestamps);
        assert!(!options.prefer_on_device);
        assert!(options.provider_options.is_null());
    }
}
