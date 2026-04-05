use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SessionId(Uuid);

impl SessionId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for SessionId {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RequestId(String);

impl RequestId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<&str> for RequestId {
    fn from(value: &str) -> Self {
        Self::new(value)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityDomain {
    Asr,
    Tts,
    Transport,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Capability {
    pub key: String,
    pub enabled: bool,
}

impl Capability {
    pub fn enabled(key: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            enabled: true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeMode {
    InProcess,
    LocalDaemon,
    RemoteGateway,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderSelectionMode {
    Auto,
    Provider,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderSelector {
    pub mode: ProviderSelectionMode,
    pub provider_id: Option<String>,
    pub required_capabilities: Vec<String>,
    pub preferred_capabilities: Vec<String>,
}

impl ProviderSelector {
    pub fn provider(provider_id: impl Into<String>) -> Self {
        Self {
            mode: ProviderSelectionMode::Provider,
            provider_id: Some(provider_id.into()),
            required_capabilities: Vec::new(),
            preferred_capabilities: Vec::new(),
        }
    }
}

impl Default for ProviderSelector {
    fn default() -> Self {
        Self {
            mode: ProviderSelectionMode::Auto,
            provider_id: None,
            required_capabilities: Vec::new(),
            preferred_capabilities: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AudioEncoding {
    #[serde(rename = "pcm_s16le")]
    PcmS16Le,
    #[serde(rename = "pcm_f32le")]
    PcmF32Le,
    Opus,
    Mp3,
    Aac,
    Flac,
    Wav,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AudioFormat {
    pub encoding: AudioEncoding,
    pub sample_rate_hz: u32,
    pub channels: u16,
}

impl AudioFormat {
    pub fn pcm_s16le(sample_rate_hz: u32, channels: u16) -> Self {
        Self {
            encoding: AudioEncoding::PcmS16Le,
            sample_rate_hz,
            channels,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderDescriptor {
    pub id: String,
    pub name: String,
    pub domain: CapabilityDomain,
    pub runtime: RuntimeMode,
    pub capabilities: Vec<Capability>,
}

impl ProviderDescriptor {
    pub fn new(
        id: impl Into<String>,
        name: impl Into<String>,
        domain: CapabilityDomain,
        runtime: RuntimeMode,
    ) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            domain,
            runtime,
            capabilities: Vec::new(),
        }
    }

    pub fn with_capability(mut self, capability: Capability) -> Self {
        self.capabilities.push(capability);
        self
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ErrorInfo {
    pub code: String,
    pub message: String,
    pub retryable: bool,
    pub details: Value,
}

impl ErrorInfo {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            retryable: false,
            details: Value::Null,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventEnvelope<T> {
    pub session_id: SessionId,
    pub sequence: u64,
    pub payload: T,
}

impl<T> EventEnvelope<T> {
    pub fn new(session_id: SessionId, sequence: u64, payload: T) -> Self {
        Self {
            session_id,
            sequence,
            payload,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_descriptor_collects_capabilities() {
        let descriptor = ProviderDescriptor::new(
            "apple.asr",
            "Apple ASR",
            CapabilityDomain::Asr,
            RuntimeMode::InProcess,
        )
        .with_capability(Capability::enabled("on-device"));

        assert_eq!(descriptor.capabilities.len(), 1);
        assert_eq!(descriptor.capabilities[0].key, "on-device");
    }

    #[test]
    fn provider_selector_defaults_to_auto_routing() {
        let selector = ProviderSelector::default();
        assert_eq!(selector.mode, ProviderSelectionMode::Auto);
        assert!(selector.provider_id.is_none());
    }
}
