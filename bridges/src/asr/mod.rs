mod composite;
mod minimax_http;
mod mock;
mod stdio;
mod tcp;

pub use composite::CompositeAsrBridge;
pub use minimax_http::{MiniMaxHttpAsrBridge, MiniMaxHttpAsrBridgeConfig};
pub use mock::MockAsrBridge;
pub use stdio::{StdioAsrBridge, StdioAsrBridgeConfig};
pub use tcp::{TcpAsrBridge, TcpAsrBridgeConfig};

use std::sync::Arc;

use async_trait::async_trait;
use serde::Serialize;
use serde_json::Value;
use speechmesh_asr::{AsrSession, StreamRequest, Transcript, TranscriptSegment};
use speechmesh_core::{AudioEncoding, Capability, ProviderDescriptor, StreamMode};
use tokio::sync::mpsc;

use crate::BridgeError;

/// ASR bridge 会话产生的事件
#[derive(Debug, Clone, PartialEq)]
pub enum BridgeAsrEvent {
    Partial { text: String },
    Final { transcript: Transcript },
    Ended { reason: Option<String> },
    Error { message: String },
}

/// ASR bridge 内部命令
#[derive(Debug, Clone)]
pub enum BridgeCommand {
    PushAudio(Vec<u8>),
    Commit,
    Stop,
}

/// ASR bridge 会话控制器（可安全克隆）
#[derive(Debug, Clone)]
pub struct BridgeAsrSessionController {
    command_tx: mpsc::Sender<BridgeCommand>,
}

impl BridgeAsrSessionController {
    pub async fn push_audio(&self, chunk: Vec<u8>) -> Result<(), BridgeError> {
        self.command_tx
            .send(BridgeCommand::PushAudio(chunk))
            .await
            .map_err(|_| BridgeError::Disconnected("bridge command channel closed".to_string()))
    }

    pub async fn commit(&self) -> Result<(), BridgeError> {
        self.command_tx
            .send(BridgeCommand::Commit)
            .await
            .map_err(|_| BridgeError::Disconnected("bridge command channel closed".to_string()))
    }

    pub async fn stop(&self) -> Result<(), BridgeError> {
        self.command_tx
            .send(BridgeCommand::Stop)
            .await
            .map_err(|_| BridgeError::Disconnected("bridge command channel closed".to_string()))
    }
}

/// ASR bridge 会话句柄
#[derive(Debug)]
pub struct BridgeAsrSessionHandle {
    pub session: AsrSession,
    command_tx: mpsc::Sender<BridgeCommand>,
    event_rx: Option<mpsc::Receiver<BridgeAsrEvent>>,
}

impl BridgeAsrSessionHandle {
    pub fn new(
        session: AsrSession,
        command_tx: mpsc::Sender<BridgeCommand>,
        event_rx: mpsc::Receiver<BridgeAsrEvent>,
    ) -> Self {
        Self {
            session,
            command_tx,
            event_rx: Some(event_rx),
        }
    }

    pub async fn push_audio(&self, chunk: Vec<u8>) -> Result<(), BridgeError> {
        self.command_tx
            .send(BridgeCommand::PushAudio(chunk))
            .await
            .map_err(|_| BridgeError::Disconnected("bridge command channel closed".to_string()))
    }

    pub async fn commit(&self) -> Result<(), BridgeError> {
        self.command_tx
            .send(BridgeCommand::Commit)
            .await
            .map_err(|_| BridgeError::Disconnected("bridge command channel closed".to_string()))
    }

    pub async fn stop(&self) -> Result<(), BridgeError> {
        self.command_tx
            .send(BridgeCommand::Stop)
            .await
            .map_err(|_| BridgeError::Disconnected("bridge command channel closed".to_string()))
    }

    pub fn take_event_rx(&mut self) -> Option<mpsc::Receiver<BridgeAsrEvent>> {
        self.event_rx.take()
    }

    pub fn controller(&self) -> BridgeAsrSessionController {
        BridgeAsrSessionController {
            command_tx: self.command_tx.clone(),
        }
    }
}

/// ASR bridge trait
#[async_trait]
pub trait AsrBridge: Send + Sync {
    fn descriptors(&self) -> Vec<ProviderDescriptor>;

    async fn start_stream(
        &self,
        request: StreamRequest,
    ) -> Result<BridgeAsrSessionHandle, BridgeError>;
}

pub type SharedAsrBridge = Arc<dyn AsrBridge>;

// ---- 公共辅助函数 ----

pub(crate) fn requested_asr_input_mode(request: &StreamRequest) -> StreamMode {
    parse_stream_mode(
        request.options.provider_options.get("input_mode"),
        StreamMode::Streaming,
    )
}

pub(crate) fn requested_asr_output_mode(request: &StreamRequest) -> StreamMode {
    if let Some(explicit) = request.options.provider_options.get("output_mode") {
        return parse_stream_mode(Some(explicit), StreamMode::Buffered);
    }
    if request.options.interim_results {
        StreamMode::Streaming
    } else {
        StreamMode::Buffered
    }
}

fn parse_stream_mode(value: Option<&Value>, default: StreamMode) -> StreamMode {
    match value
        .and_then(Value::as_str)
        .map(|value| value.trim().to_ascii_lowercase())
    {
        Some(mode) if mode == "streaming" => StreamMode::Streaming,
        Some(mode) if mode == "buffered" => StreamMode::Buffered,
        _ => default,
    }
}

pub(crate) fn asr_descriptor_with_io_modes(
    descriptor: ProviderDescriptor,
    supports_streaming_output: bool,
) -> ProviderDescriptor {
    let descriptor = descriptor
        .with_capability(Capability::enabled("streaming-input"))
        .with_capability(Capability::enabled("buffered-input"))
        .with_capability(Capability::enabled("buffered-output"));
    if supports_streaming_output {
        descriptor.with_capability(Capability::enabled("streaming-output"))
    } else {
        descriptor
    }
}

pub(crate) fn has_enabled_capability(
    descriptor: &ProviderDescriptor,
    capability_key: &str,
) -> bool {
    descriptor
        .capabilities
        .iter()
        .any(|capability| capability.enabled && capability.key == capability_key)
}

pub(crate) fn seconds_to_ms(seconds: f64) -> u64 {
    (seconds * 1000.0).round() as u64
}

pub(crate) fn extract_final_transcript(payload: &Value) -> Option<Transcript> {
    let text = payload.get("text").and_then(Value::as_str)?.to_string();
    let segments = payload
        .get("segments")
        .and_then(Value::as_array)
        .map(|segments| {
            segments
                .iter()
                .filter_map(|segment| {
                    let text = segment
                        .get("substring")
                        .and_then(Value::as_str)?
                        .to_string();
                    let start_ms = segment
                        .get("timestamp_s")
                        .and_then(Value::as_f64)
                        .map(seconds_to_ms);
                    let duration_ms = segment
                        .get("duration_s")
                        .and_then(Value::as_f64)
                        .map(seconds_to_ms);
                    let end_ms = match (start_ms, duration_ms) {
                        (Some(start_ms), Some(duration_ms)) => Some(start_ms + duration_ms),
                        _ => None,
                    };
                    Some(TranscriptSegment {
                        text,
                        is_final: true,
                        start_ms,
                        end_ms,
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    Some(Transcript {
        text,
        language: None,
        segments,
    })
}

/// 根据音频格式确定流式 partial 最小触发字节数
pub(crate) fn streaming_partial_trigger_bytes(
    format: &speechmesh_core::AudioFormat,
    configured_min_bytes: usize,
) -> usize {
    if configured_min_bytes > 0 {
        return configured_min_bytes;
    }
    let inferred = match format.encoding {
        AudioEncoding::PcmS16Le => {
            Some(format.sample_rate_hz as usize * usize::from(format.channels) * 2)
        }
        AudioEncoding::PcmF32Le => {
            Some(format.sample_rate_hz as usize * usize::from(format.channels) * 4)
        }
        _ => None,
    };
    inferred.unwrap_or(32 * 1024)
}

/// 将 PCM S16LE 原始数据封装为 WAV
pub(crate) fn encode_pcm_s16le_wav(
    pcm: &[u8],
    sample_rate_hz: u32,
    channels: u16,
) -> Result<Vec<u8>, BridgeError> {
    if pcm.len() > u32::MAX as usize - 44 {
        return Err(BridgeError::Protocol(
            "PCM payload too large to wrap in WAV".to_string(),
        ));
    }
    let byte_rate = sample_rate_hz
        .checked_mul(u32::from(channels))
        .and_then(|value| value.checked_mul(2))
        .ok_or_else(|| BridgeError::Protocol("invalid WAV byte rate".to_string()))?;
    let block_align = channels
        .checked_mul(2)
        .ok_or_else(|| BridgeError::Protocol("invalid WAV block align".to_string()))?;
    let data_size = pcm.len() as u32;
    let riff_size = 36_u32
        .checked_add(data_size)
        .ok_or_else(|| BridgeError::Protocol("invalid WAV size".to_string()))?;

    let mut wav = Vec::with_capacity(44 + pcm.len());
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&riff_size.to_le_bytes());
    wav.extend_from_slice(b"WAVE");
    wav.extend_from_slice(b"fmt ");
    wav.extend_from_slice(&16_u32.to_le_bytes());
    wav.extend_from_slice(&1_u16.to_le_bytes());
    wav.extend_from_slice(&channels.to_le_bytes());
    wav.extend_from_slice(&sample_rate_hz.to_le_bytes());
    wav.extend_from_slice(&byte_rate.to_le_bytes());
    wav.extend_from_slice(&block_align.to_le_bytes());
    wav.extend_from_slice(&16_u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data_size.to_le_bytes());
    wav.extend_from_slice(pcm);
    Ok(wav)
}

/// 音频编码格式名
pub(crate) fn audio_encoding_name(encoding: AudioEncoding) -> &'static str {
    match encoding {
        AudioEncoding::PcmS16Le => "pcm_s16le",
        AudioEncoding::PcmF32Le => "pcm_f32le",
        AudioEncoding::Opus => "opus",
        AudioEncoding::Mp3 => "mp3",
        AudioEncoding::Aac => "aac",
        AudioEncoding::Flac => "flac",
        AudioEncoding::Wav => "wav",
    }
}

/// 音频 MIME 类型
pub(crate) fn audio_mime_type(
    format: &speechmesh_core::AudioFormat,
) -> Result<&'static str, BridgeError> {
    match format.encoding {
        AudioEncoding::PcmS16Le => Ok("audio/wav"),
        AudioEncoding::Mp3 => Ok("audio/mpeg"),
        AudioEncoding::Flac => Ok("audio/flac"),
        AudioEncoding::Wav => Ok("audio/wav"),
        AudioEncoding::Opus | AudioEncoding::Aac | AudioEncoding::PcmF32Le => {
            Err(BridgeError::Unavailable(format!(
                "MiniMax ASR does not support {} input yet",
                audio_encoding_name(format.encoding)
            )))
        }
    }
}

/// 编码用于 MiniMax 上传的音频数据
pub(crate) fn encode_minimax_upload_audio(
    format: &speechmesh_core::AudioFormat,
    audio: &[u8],
) -> Result<Vec<u8>, BridgeError> {
    match format.encoding {
        AudioEncoding::PcmS16Le => {
            encode_pcm_s16le_wav(audio, format.sample_rate_hz, format.channels)
        }
        AudioEncoding::Mp3 | AudioEncoding::Flac | AudioEncoding::Wav => Ok(audio.to_vec()),
        AudioEncoding::Opus | AudioEncoding::Aac | AudioEncoding::PcmF32Le => {
            Err(BridgeError::Unavailable(format!(
                "MiniMax ASR does not support {} input yet",
                audio_encoding_name(format.encoding)
            )))
        }
    }
}

/// 默认音频文件名
pub(crate) fn default_audio_filename(format: &speechmesh_core::AudioFormat) -> &'static str {
    match format.encoding {
        AudioEncoding::PcmS16Le => "audio.wav",
        AudioEncoding::Mp3 => "audio.mp3",
        AudioEncoding::Flac => "audio.flac",
        AudioEncoding::Wav => "audio.wav",
        _ => "audio.bin",
    }
}

/// 过滤保留的 provider options
pub(crate) fn filter_reserved_provider_options(
    options: &Value,
    reserved: &[&str],
) -> Value {
    let Some(object) = options.as_object() else {
        return Value::Null;
    };
    let filtered = object
        .iter()
        .filter(|(key, _)| !reserved.iter().any(|reserved_key| reserved_key == key))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect::<serde_json::Map<_, _>>();
    if filtered.is_empty() {
        Value::Null
    } else {
        Value::Object(filtered)
    }
}

/// 将 provider option 值转为表单字符串
pub(crate) fn provider_option_to_form_value(value: Value) -> String {
    match value {
        Value::Null => "null".to_string(),
        Value::Bool(value) => value.to_string(),
        Value::Number(value) => value.to_string(),
        Value::String(value) => value,
        other => other.to_string(),
    }
}

/// 构建 multipart form body
pub(crate) fn build_multipart_form_body(
    boundary: &str,
    fields: &[(String, String)],
    filename: &str,
    mime_type: &str,
    file_bytes: &[u8],
) -> Vec<u8> {
    let mut body = Vec::new();
    for (name, value) in fields {
        body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
        body.extend_from_slice(
            format!("Content-Disposition: form-data; name=\"{name}\"\r\n\r\n").as_bytes(),
        );
        body.extend_from_slice(value.as_bytes());
        body.extend_from_slice(b"\r\n");
    }
    body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
    body.extend_from_slice(
        format!("Content-Disposition: form-data; name=\"file\"; filename=\"{filename}\"\r\n")
            .as_bytes(),
    );
    body.extend_from_slice(format!("Content-Type: {mime_type}\r\n\r\n").as_bytes());
    body.extend_from_slice(file_bytes);
    body.extend_from_slice(b"\r\n");
    body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    body
}

// ---- 用于 stdio/tcp bridge 的帧类型 ----

#[derive(Debug, Clone, Serialize)]
pub(crate) struct BridgeStartFrame {
    #[serde(rename = "type")]
    pub type_name: &'static str,
    pub request_id: Option<String>,
    pub session_id: Option<speechmesh_core::SessionId>,
    pub payload: BridgeStartPayload,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct BridgeStartPayload {
    pub locale: Option<String>,
    pub should_report_partials: bool,
    pub requires_on_device: bool,
    pub input_format: speechmesh_core::AudioFormat,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct BridgeAudioFrame {
    #[serde(rename = "type")]
    pub type_name: &'static str,
    pub request_id: Option<String>,
    pub session_id: Option<speechmesh_core::SessionId>,
    pub payload: BridgeAudioPayload,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct BridgeAudioPayload {
    pub data_base64: String,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct BridgeEmptyFrame {
    #[serde(rename = "type")]
    pub type_name: &'static str,
    pub request_id: Option<String>,
    pub session_id: Option<speechmesh_core::SessionId>,
    pub payload: BridgeEmptyPayload,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct BridgeEmptyPayload {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pcm_s16le_is_wrapped_into_wav() {
        let wav = encode_pcm_s16le_wav(&[0x01, 0x00, 0x02, 0x00], 16_000, 1).expect("wav");
        assert_eq!(&wav[..4], b"RIFF");
        assert_eq!(&wav[8..12], b"WAVE");
        assert_eq!(&wav[36..40], b"data");
        assert_eq!(&wav[40..44], 4_u32.to_le_bytes());
        assert_eq!(&wav[44..], &[0x01, 0x00, 0x02, 0x00]);
    }
}
