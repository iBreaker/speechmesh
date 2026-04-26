mod composite;
mod melo_http;
mod minimax_http;
mod mock;
mod qwen_http;

pub use composite::CompositeTtsBridge;
pub use melo_http::{MeloHttpTtsBridge, MeloHttpTtsBridgeConfig};
pub use minimax_http::{MiniMaxHttpTtsBridge, MiniMaxHttpTtsBridgeConfig};
pub use mock::MockTtsBridge;
pub use qwen_http::{QwenHttpTtsBridge, QwenHttpTtsBridgeConfig};

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;
use speechmesh_core::StreamMode;
use speechmesh_transport::VoiceListRequest;
use speechmesh_tts::{AudioChunk, StreamRequest, SynthesisInputKind, VoiceDescriptor};
use tokio::sync::mpsc;

use crate::BridgeError;
use speechmesh_core::{AudioFormat, ProviderDescriptor};
use speechmesh_tts::TtsSession;

/// TTS bridge 会话产生的事件
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BridgeTtsEvent {
    Audio { chunk: AudioChunk },
    Ended { reason: Option<String> },
    Error { message: String },
}

/// TTS bridge 内部命令
#[derive(Debug, Clone)]
pub(crate) enum BridgeTtsCommand {
    AppendInput(String),
    Commit,
    Stop,
}

/// TTS bridge 会话控制器（可安全克隆）
#[derive(Debug, Clone)]
pub struct BridgeTtsSessionController {
    command_tx: mpsc::Sender<BridgeTtsCommand>,
}

impl BridgeTtsSessionController {
    pub async fn append_input(&self, delta: String) -> Result<(), BridgeError> {
        self.command_tx
            .send(BridgeTtsCommand::AppendInput(delta))
            .await
            .map_err(|_| BridgeError::Disconnected("bridge command channel closed".to_string()))
    }

    pub async fn commit(&self) -> Result<(), BridgeError> {
        self.command_tx
            .send(BridgeTtsCommand::Commit)
            .await
            .map_err(|_| BridgeError::Disconnected("bridge command channel closed".to_string()))
    }

    pub async fn stop(&self) -> Result<(), BridgeError> {
        self.command_tx
            .send(BridgeTtsCommand::Stop)
            .await
            .map_err(|_| BridgeError::Disconnected("bridge command channel closed".to_string()))
    }
}

/// TTS bridge 会话句柄
#[derive(Debug)]
pub struct BridgeTtsSessionHandle {
    pub session: TtsSession,
    pub input_kind: SynthesisInputKind,
    command_tx: mpsc::Sender<BridgeTtsCommand>,
    event_rx: Option<mpsc::Receiver<BridgeTtsEvent>>,
}

impl BridgeTtsSessionHandle {
    pub(crate) fn new(
        session: TtsSession,
        input_kind: SynthesisInputKind,
        command_tx: mpsc::Sender<BridgeTtsCommand>,
        event_rx: mpsc::Receiver<BridgeTtsEvent>,
    ) -> Self {
        Self {
            session,
            input_kind,
            command_tx,
            event_rx: Some(event_rx),
        }
    }

    pub async fn append_input(&self, delta: String) -> Result<(), BridgeError> {
        self.command_tx
            .send(BridgeTtsCommand::AppendInput(delta))
            .await
            .map_err(|_| BridgeError::Disconnected("bridge command channel closed".to_string()))
    }

    pub async fn commit(&self) -> Result<(), BridgeError> {
        self.command_tx
            .send(BridgeTtsCommand::Commit)
            .await
            .map_err(|_| BridgeError::Disconnected("bridge command channel closed".to_string()))
    }

    pub async fn stop(&self) -> Result<(), BridgeError> {
        self.command_tx
            .send(BridgeTtsCommand::Stop)
            .await
            .map_err(|_| BridgeError::Disconnected("bridge command channel closed".to_string()))
    }

    pub fn take_event_rx(&mut self) -> Option<mpsc::Receiver<BridgeTtsEvent>> {
        self.event_rx.take()
    }

    pub fn controller(&self) -> BridgeTtsSessionController {
        BridgeTtsSessionController {
            command_tx: self.command_tx.clone(),
        }
    }
}

/// TTS bridge trait
#[async_trait]
pub trait TtsBridge: Send + Sync {
    fn descriptors(&self) -> Vec<ProviderDescriptor>;

    async fn list_voices(
        &self,
        request: VoiceListRequest,
    ) -> Result<Vec<VoiceDescriptor>, BridgeError>;

    async fn start_stream(
        &self,
        request: StreamRequest,
    ) -> Result<BridgeTtsSessionHandle, BridgeError>;
}

pub type SharedTtsBridge = Arc<dyn TtsBridge>;

// ---- 公共辅助函数 ----

pub(crate) fn requested_tts_input_mode(options: &speechmesh_tts::SynthesisOptions) -> StreamMode {
    parse_stream_mode(
        options.provider_options.get("input_mode"),
        StreamMode::Buffered,
    )
}

pub(crate) fn requested_tts_output_mode(options: &speechmesh_tts::SynthesisOptions) -> StreamMode {
    if let Some(explicit) = options.provider_options.get("output_mode") {
        return parse_stream_mode(Some(explicit), StreamMode::Buffered);
    }
    if options.stream {
        StreamMode::Streaming
    } else {
        StreamMode::Buffered
    }
}

pub(crate) fn ensure_tts_modes_supported(
    input_mode: StreamMode,
    output_mode: StreamMode,
    supports_streaming_input: bool,
    supports_streaming_output: bool,
    provider_name: &str,
) -> Result<(), BridgeError> {
    if matches!(input_mode, StreamMode::Streaming) && !supports_streaming_input {
        return Err(BridgeError::Unavailable(format!(
            "{provider_name} does not support streaming TTS input"
        )));
    }
    if matches!(output_mode, StreamMode::Streaming) && !supports_streaming_output {
        return Err(BridgeError::Unavailable(format!(
            "{provider_name} does not support streaming TTS output"
        )));
    }
    Ok(())
}

pub(crate) fn parse_stream_mode(value: Option<&Value>, default: StreamMode) -> StreamMode {
    match value
        .and_then(Value::as_str)
        .map(|value| value.trim().to_ascii_lowercase())
    {
        Some(mode) if mode == "streaming" => StreamMode::Streaming,
        Some(mode) if mode == "buffered" => StreamMode::Buffered,
        _ => default,
    }
}

pub(crate) fn filter_reserved_provider_options(options: &Value, reserved_keys: &[&str]) -> Value {
    let Some(map) = options.as_object() else {
        return Value::Null;
    };
    let filtered = map
        .iter()
        .filter(|(key, _)| !reserved_keys.iter().any(|reserved| reserved == key))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect::<serde_json::Map<String, Value>>();
    if filtered.is_empty() {
        Value::Null
    } else {
        Value::Object(filtered)
    }
}

pub(crate) fn normalize_output_format_name(
    encoding: Option<speechmesh_core::AudioEncoding>,
    fallback: &str,
) -> Result<String, BridgeError> {
    let resolved = match encoding {
        Some(speechmesh_core::AudioEncoding::Wav) => "wav",
        Some(speechmesh_core::AudioEncoding::Mp3) => "mp3",
        Some(speechmesh_core::AudioEncoding::Flac) => "flac",
        Some(other) => {
            return Err(BridgeError::Unavailable(format!(
                "unsupported TTS output encoding for upstream provider: {other:?}"
            )));
        }
        None => fallback,
    };
    Ok(resolved.trim().to_ascii_lowercase())
}

pub(crate) fn audio_encoding_from_name(
    name: &str,
) -> Result<speechmesh_core::AudioEncoding, BridgeError> {
    match name.trim().to_ascii_lowercase().as_str() {
        "wav" => Ok(speechmesh_core::AudioEncoding::Wav),
        "mp3" => Ok(speechmesh_core::AudioEncoding::Mp3),
        "flac" => Ok(speechmesh_core::AudioEncoding::Flac),
        other => Err(BridgeError::Unavailable(format!(
            "unsupported audio format requested from upstream provider: {other}"
        ))),
    }
}

/// 将音频字节拆分为多个 chunk 发送到 event channel
pub(crate) async fn emit_audio_chunks(
    event_tx: &mpsc::Sender<BridgeTtsEvent>,
    audio_bytes: &[u8],
    output_format: Option<AudioFormat>,
    chunk_size_bytes: usize,
) -> Result<(), BridgeError> {
    if audio_bytes.is_empty() {
        return Err(BridgeError::Protocol(
            "provider returned empty audio".to_string(),
        ));
    }

    let total_chunks = std::cmp::max(1, audio_bytes.len().div_ceil(chunk_size_bytes));
    for (index, part) in audio_bytes.chunks(chunk_size_bytes).enumerate() {
        let chunk = AudioChunk {
            bytes: part.to_vec(),
            sequence: (index + 1) as u64,
            is_final: index + 1 == total_chunks,
            format: if index == 0 {
                output_format.clone()
            } else {
                None
            },
        };
        event_tx
            .send(BridgeTtsEvent::Audio { chunk })
            .await
            .map_err(|_| BridgeError::Disconnected("bridge event channel closed".to_string()))?;
    }
    Ok(())
}

/// 解码 MiniMax 音频载荷（hex 或 base64）
pub(crate) fn decode_minimax_audio(encoded: &str) -> Result<Vec<u8>, BridgeError> {
    use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64_STANDARD};

    let trimmed = encoded.trim();
    if trimmed.is_empty() {
        return Err(BridgeError::Protocol(
            "MiniMax TTS returned an empty audio payload".to_string(),
        ));
    }
    if trimmed.len() % 2 == 0 && trimmed.chars().all(|ch| ch.is_ascii_hexdigit()) {
        return decode_hex(trimmed).map_err(|error| {
            BridgeError::Protocol(format!("failed to decode MiniMax audio payload: {error}"))
        });
    }
    if let Ok(bytes) = BASE64_STANDARD.decode(trimmed) {
        return Ok(bytes);
    }
    decode_hex(trimmed).map_err(|error| {
        BridgeError::Protocol(format!("failed to decode MiniMax audio payload: {error}"))
    })
}

fn decode_hex(value: &str) -> Result<Vec<u8>, String> {
    let compact: String = value.chars().filter(|ch| !ch.is_whitespace()).collect();
    if compact.len() % 2 != 0 {
        return Err("hex input has an odd number of digits".to_string());
    }
    let mut out = Vec::with_capacity(compact.len() / 2);
    let bytes = compact.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        let pair =
            std::str::from_utf8(&bytes[index..index + 2]).map_err(|error| error.to_string())?;
        let byte = u8::from_str_radix(pair, 16).map_err(|error| error.to_string())?;
        out.push(byte);
        index += 2;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use speechmesh_core::ProviderSelectionMode;
    use speechmesh_tts::SynthesisOptions;

    pub(crate) fn mock_request() -> StreamRequest {
        StreamRequest {
            provider: speechmesh_core::ProviderSelector {
                mode: ProviderSelectionMode::Auto,
                provider_id: None,
                required_capabilities: Vec::new(),
                preferred_capabilities: Vec::new(),
            },
            input_kind: SynthesisInputKind::Text,
            output_format: Some(AudioFormat {
                encoding: speechmesh_core::AudioEncoding::Wav,
                sample_rate_hz: 16_000,
                channels: 1,
            }),
            options: SynthesisOptions::default(),
        }
    }

    #[test]
    fn minimax_decoder_accepts_hex_audio() {
        let decoded = decode_minimax_audio("52494646").expect("hex should decode");
        assert_eq!(decoded, b"RIFF");
    }

    #[test]
    fn minimax_decoder_accepts_base64_audio() {
        let decoded = decode_minimax_audio("UklGRg==").expect("base64 should decode");
        assert_eq!(decoded, b"RIFF");
    }
}
