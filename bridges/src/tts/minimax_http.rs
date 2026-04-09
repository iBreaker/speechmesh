use std::time::Duration;

use async_trait::async_trait;
// base64 解码由 tts mod.rs 中的 decode_minimax_audio 统一处理
use futures_util::{SinkExt, StreamExt};
use reqwest::Client;
use serde::Deserialize;
use serde_json::{Map, Value, json};
use speechmesh_core::{
    AudioEncoding, AudioFormat, Capability, CapabilityDomain, ProviderDescriptor, RuntimeMode,
    SessionId, StreamMode,
};
use speechmesh_tts::{AudioChunk, StreamRequest, SynthesisInputKind, TtsSession, VoiceDescriptor};
use speechmesh_transport::VoiceListRequest;
use tokio::sync::mpsc;
use tokio_tungstenite::{
    connect_async,
    tungstenite::{client::IntoClientRequest, protocol::Message},
};

use super::{
    BridgeTtsCommand, BridgeTtsEvent, BridgeTtsSessionHandle, TtsBridge,
    audio_encoding_from_name, decode_minimax_audio, emit_audio_chunks,
    ensure_tts_modes_supported, filter_reserved_provider_options, normalize_output_format_name,
    requested_tts_input_mode, requested_tts_output_mode,
};
use crate::BridgeError;

#[derive(Debug, Clone)]
pub struct MiniMaxHttpTtsBridgeConfig {
    pub provider_id: String,
    pub display_name: Option<String>,
    pub base_url: String,
    pub api_key: String,
    pub group_id: String,
    pub default_model: String,
    pub default_voice_id: String,
    pub default_sample_rate_hz: u32,
    pub default_format: String,
    pub request_timeout: Duration,
    pub chunk_size_bytes: usize,
}

pub struct MiniMaxHttpTtsBridge {
    config: MiniMaxHttpTtsBridgeConfig,
    client: Client,
}

impl MiniMaxHttpTtsBridge {
    pub fn new(config: MiniMaxHttpTtsBridgeConfig) -> Result<Self, BridgeError> {
        let client = Client::builder()
            .timeout(config.request_timeout)
            .build()
            .map_err(|error| {
                BridgeError::Unavailable(format!("failed to build MiniMax TTS client: {error}"))
            })?;
        Ok(Self { config, client })
    }
}

// ---- MiniMax 响应类型 ----

#[derive(Debug, Deserialize)]
struct MiniMaxBaseResponse<T> {
    #[serde(default)]
    base_resp: Option<MiniMaxBaseResp>,
    #[serde(default)]
    data: Option<T>,
}

#[derive(Debug, Deserialize)]
struct MiniMaxBaseResp {
    #[serde(default)]
    status_code: Option<i64>,
    #[serde(default)]
    status_msg: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct MiniMaxAudioData {
    #[serde(default)]
    audio: Option<String>,
    #[serde(default)]
    audio_base64: Option<String>,
}

#[derive(Debug, Deserialize)]
struct MiniMaxWsEvent {
    #[serde(default)]
    event: Option<String>,
    #[serde(default)]
    is_final: Option<bool>,
    #[serde(default)]
    data: Option<MiniMaxWsData>,
    #[serde(default)]
    _extra_info: Option<MiniMaxWsExtraInfo>,
    #[serde(default)]
    base_resp: Option<MiniMaxBaseResp>,
}

#[derive(Debug, Deserialize)]
struct MiniMaxWsData {
    #[serde(default)]
    audio: Option<String>,
    #[serde(default)]
    audio_base64: Option<String>,
}

#[derive(Debug, Deserialize)]
struct MiniMaxWsExtraInfo {
    #[serde(default)]
    _audio_length: Option<u64>,
}

// ---- TtsBridge 实现 ----

#[async_trait]
impl TtsBridge for MiniMaxHttpTtsBridge {
    fn descriptors(&self) -> Vec<ProviderDescriptor> {
        vec![
            ProviderDescriptor::new(
                self.config.provider_id.clone(),
                self.config
                    .display_name
                    .clone()
                    .unwrap_or_else(|| "MiniMax Speech".to_string()),
                CapabilityDomain::Tts,
                RuntimeMode::RemoteGateway,
            )
            .with_capability(Capability::enabled("voice-list"))
            .with_capability(Capability::enabled("buffered-input"))
            .with_capability(Capability::enabled("buffered-text-input"))
            .with_capability(Capability::enabled("buffered-output"))
            .with_capability(Capability::enabled("rate-control"))
            .with_capability(Capability::enabled("pitch-control"))
            .with_capability(Capability::enabled("volume-control"))
            .with_capability(Capability::enabled("emotion-control"))
            .with_capability(Capability::enabled("wav-output"))
            .with_capability(Capability::enabled("mp3-output"))
            .with_capability(Capability::enabled("cloud-managed"))
            .with_capability(Capability::enabled("realtime-low-latency")),
        ]
    }

    async fn list_voices(
        &self,
        request: VoiceListRequest,
    ) -> Result<Vec<VoiceDescriptor>, BridgeError> {
        if let Some(language) = request.language.as_deref() {
            if language != "und" && language != "zh" && language != "zh-CN" {
                return Ok(Vec::new());
            }
        }
        Ok(vec![VoiceDescriptor {
            id: self.config.default_voice_id.clone(),
            language: "und".to_string(),
            display_name: self
                .config
                .display_name
                .clone()
                .unwrap_or_else(|| "MiniMax Speech".to_string()),
            gender: None,
            capabilities: vec![
                "rate-control".to_string(),
                "pitch-control".to_string(),
                "volume-control".to_string(),
                "emotion-control".to_string(),
                "wav-output".to_string(),
                "mp3-output".to_string(),
                "cloud-managed".to_string(),
                "realtime-low-latency".to_string(),
            ],
        }])
    }

    async fn start_stream(
        &self,
        request: StreamRequest,
    ) -> Result<BridgeTtsSessionHandle, BridgeError> {
        let input_mode = requested_tts_input_mode(&request.options);
        let output_mode = requested_tts_output_mode(&request.options);
        ensure_tts_modes_supported(input_mode, output_mode, true, true, "MiniMax TTS")?;
        if request.input_kind != SynthesisInputKind::Text {
            return Err(BridgeError::Unavailable(
                "MiniMax TTS bridge currently supports text input only".to_string(),
            ));
        }

        let default_encoding = if matches!(output_mode, StreamMode::Streaming) {
            AudioEncoding::Mp3
        } else {
            AudioEncoding::Wav
        };
        let desired_format = normalize_output_format_name(
            request
                .output_format
                .as_ref()
                .map(|format| format.encoding)
                .or(Some(default_encoding)),
            &self.config.default_format,
        )?;
        if matches!(output_mode, StreamMode::Streaming) && desired_format == "wav" {
            return Err(BridgeError::Unavailable(
                "MiniMax streaming TTS does not support WAV output; use mp3 or flac".to_string(),
            ));
        }
        let accepted_output_format = Some(AudioFormat {
            encoding: audio_encoding_from_name(&desired_format)?,
            sample_rate_hz: self.config.default_sample_rate_hz,
            channels: 1,
        });

        let session = TtsSession {
            id: SessionId::new(),
            provider_id: self.config.provider_id.clone(),
            accepted_output_format,
            input_mode,
            output_mode,
        };
        let (command_tx, mut command_rx) = mpsc::channel::<BridgeTtsCommand>(32);
        let (event_tx, event_rx) = mpsc::channel::<BridgeTtsEvent>(64);
        let output_format = session.accepted_output_format.clone();
        let input_kind = request.input_kind;
        let client = self.client.clone();
        let config = self.config.clone();
        let chunk_size_bytes = self.config.chunk_size_bytes.max(1);
        let options = request.options.clone();
        let output_encoding = desired_format;

        tokio::spawn(async move {
            let mut buffer = String::new();
            if matches!(output_mode, StreamMode::Streaming) {
                let result = run_minimax_ws_session(
                    &config,
                    &options,
                    input_mode,
                    &output_encoding,
                    output_format.clone(),
                    &mut command_rx,
                    &event_tx,
                )
                .await;
                match result {
                    Ok(reason) => {
                        let _ = event_tx.send(BridgeTtsEvent::Ended { reason }).await;
                    }
                    Err(error) => {
                        let _ = event_tx
                            .send(BridgeTtsEvent::Error {
                                message: error.to_string(),
                            })
                            .await;
                        let _ = event_tx
                            .send(BridgeTtsEvent::Ended {
                                reason: Some("provider_error".to_string()),
                            })
                            .await;
                    }
                }
                return;
            }

            while let Some(command) = command_rx.recv().await {
                match command {
                    BridgeTtsCommand::AppendInput(delta) => buffer.push_str(&delta),
                    BridgeTtsCommand::Commit => {
                        let text = buffer.trim().to_string();
                        if text.is_empty() {
                            let _ = event_tx
                                .send(BridgeTtsEvent::Error {
                                    message: "TTS input buffer is empty".to_string(),
                                })
                                .await;
                            let _ = event_tx
                                .send(BridgeTtsEvent::Ended {
                                    reason: Some("empty_input".to_string()),
                                })
                                .await;
                            return;
                        }

                        match synthesize_minimax(&client, &config, &text, &options).await {
                            Ok(audio_bytes) => {
                                if let Err(error) = emit_audio_chunks(
                                    &event_tx,
                                    &audio_bytes,
                                    output_format.clone(),
                                    chunk_size_bytes,
                                )
                                .await
                                {
                                    let _ = event_tx
                                        .send(BridgeTtsEvent::Error {
                                            message: error.to_string(),
                                        })
                                        .await;
                                }
                                let _ =
                                    event_tx.send(BridgeTtsEvent::Ended { reason: None }).await;
                            }
                            Err(error) => {
                                let _ = event_tx
                                    .send(BridgeTtsEvent::Error {
                                        message: error.to_string(),
                                    })
                                    .await;
                                let _ = event_tx
                                    .send(BridgeTtsEvent::Ended {
                                        reason: Some("provider_error".to_string()),
                                    })
                                    .await;
                            }
                        }
                        return;
                    }
                    BridgeTtsCommand::Stop => {
                        let _ = event_tx
                            .send(BridgeTtsEvent::Ended {
                                reason: Some("stopped".to_string()),
                            })
                            .await;
                        return;
                    }
                }
            }
        });

        Ok(BridgeTtsSessionHandle::new(
            session, input_kind, command_tx, event_rx,
        ))
    }
}

// ---- MiniMax REST API 合成 ----

async fn synthesize_minimax(
    client: &Client,
    config: &MiniMaxHttpTtsBridgeConfig,
    text: &str,
    options: &speechmesh_tts::SynthesisOptions,
) -> Result<Vec<u8>, BridgeError> {
    let voice_id = options
        .provider_options
        .get("voice_id")
        .and_then(Value::as_str)
        .or(options.voice.as_deref())
        .unwrap_or(&config.default_voice_id);
    let model = options
        .provider_options
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or(&config.default_model);
    let format = options
        .provider_options
        .get("format")
        .and_then(Value::as_str)
        .unwrap_or(&config.default_format);
    let normalized_format = normalize_output_format_name(None, format)?;
    let url = format!(
        "{}/v1/t2a_v2?GroupId={}",
        config.base_url.trim_end_matches('/'),
        config.group_id
    );

    let mut voice_setting = Map::new();
    voice_setting.insert("voice_id".to_string(), Value::String(voice_id.to_string()));
    if let Some(speed) = options.rate {
        voice_setting.insert("speed".to_string(), json!(speed));
    }
    if let Some(pitch) = options.pitch {
        voice_setting.insert("pitch".to_string(), json!(pitch));
    }
    if let Some(volume) = options.volume {
        voice_setting.insert("vol".to_string(), json!(volume));
    }
    if let Some(emotion) = options
        .provider_options
        .get("emotion")
        .and_then(Value::as_str)
        .or_else(|| {
            options
                .provider_options
                .get("emotion_tag")
                .and_then(Value::as_str)
        })
    {
        voice_setting.insert("emotion".to_string(), Value::String(emotion.to_string()));
    }

    let mut audio_setting = Map::new();
    audio_setting.insert(
        "sample_rate".to_string(),
        json!(config.default_sample_rate_hz),
    );
    audio_setting.insert(
        "format".to_string(),
        Value::String(normalized_format.to_string()),
    );
    audio_setting.insert("channel".to_string(), json!(1));
    match normalized_format.as_str() {
        "mp3" => {
            audio_setting.insert("bitrate".to_string(), json!(128000));
        }
        "wav" | "flac" => {}
        _ => {}
    }

    let extra = filter_reserved_provider_options(
        &options.provider_options,
        &[
            "route",
            "voice_id",
            "model",
            "format",
            "emotion",
            "emotion_tag",
        ],
    );
    let mut body = Map::new();
    body.insert("model".to_string(), Value::String(model.to_string()));
    body.insert("text".to_string(), Value::String(text.to_string()));
    body.insert("stream".to_string(), Value::Bool(false));
    body.insert("voice_setting".to_string(), Value::Object(voice_setting));
    body.insert("audio_setting".to_string(), Value::Object(audio_setting));
    if !extra.is_null() {
        body.insert("extra".to_string(), extra);
    }

    let response = client
        .post(&url)
        .bearer_auth(&config.api_key)
        .json(&Value::Object(body))
        .send()
        .await
        .map_err(|error| {
            BridgeError::Unavailable(format!(
                "failed to call MiniMax TTS endpoint {url}: {error}"
            ))
        })?;
    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(BridgeError::Unavailable(format!(
            "MiniMax TTS endpoint {url} returned status {status}: {body}"
        )));
    }

    let payload = response
        .json::<MiniMaxBaseResponse<MiniMaxAudioData>>()
        .await
        .map_err(|error| {
            BridgeError::Protocol(format!("failed to decode MiniMax TTS response: {error}"))
        })?;
    if let Some(base_resp) = payload.base_resp {
        if base_resp.status_code.unwrap_or(0) != 0 {
            return Err(BridgeError::Unavailable(format!(
                "MiniMax TTS failed: {}",
                base_resp
                    .status_msg
                    .unwrap_or_else(|| "unknown provider error".to_string())
            )));
        }
    }
    let data = payload
        .data
        .ok_or_else(|| BridgeError::Protocol("MiniMax TTS response missing data".to_string()))?;
    if let Some(encoded) = data.audio_base64.or(data.audio) {
        return decode_minimax_audio(&encoded);
    }
    Err(BridgeError::Protocol(
        "MiniMax TTS response missing audio payload".to_string(),
    ))
}

// ---- MiniMax WebSocket 流式合成 ----

async fn run_minimax_ws_session(
    config: &MiniMaxHttpTtsBridgeConfig,
    options: &speechmesh_tts::SynthesisOptions,
    input_mode: StreamMode,
    output_encoding: &str,
    output_format: Option<AudioFormat>,
    command_rx: &mut mpsc::Receiver<BridgeTtsCommand>,
    event_tx: &mpsc::Sender<BridgeTtsEvent>,
) -> Result<Option<String>, BridgeError> {
    let url = minimax_ws_url(&config.base_url, &config.group_id);
    let mut request = url.clone().into_client_request().map_err(|error| {
        BridgeError::Unavailable(format!(
            "failed to build MiniMax TTS websocket request {url}: {error}"
        ))
    })?;
    request.headers_mut().insert(
        "Authorization",
        format!("Bearer {}", config.api_key)
            .parse()
            .map_err(|error| {
                BridgeError::Unavailable(format!(
                    "failed to encode MiniMax authorization header: {error}"
                ))
            })?,
    );
    let (socket, _) = connect_async(request).await.map_err(|error| {
        BridgeError::Unavailable(format!(
            "failed to connect to MiniMax TTS websocket {url}: {error}"
        ))
    })?;
    let (mut sink, mut source) = socket.split();

    let (voice_setting, audio_setting, extra, model) =
        build_minimax_ws_settings(config, options, output_encoding)?;
    let mut started = false;
    let mut task_start_sent = false;
    let mut committed = false;
    let mut buffered = String::new();
    let mut pending_chunks: Vec<String> = Vec::new();
    let mut chunk_sequence = 0_u64;

    if matches!(input_mode, StreamMode::Buffered) {
        send_minimax_task_start(
            &mut sink,
            &model,
            None,
            &voice_setting,
            &audio_setting,
            &extra,
        )
        .await?;
        task_start_sent = true;
    }

    loop {
        tokio::select! {
            maybe_message = source.next() => {
                let message = match maybe_message {
                    Some(Ok(message)) => message,
                    Some(Err(error)) => {
                        return Err(BridgeError::Disconnected(format!("MiniMax websocket read failed: {error}")));
                    }
                    None => {
                        return if committed {
                            Ok(None)
                        } else {
                            Err(BridgeError::Disconnected("MiniMax websocket closed before synthesis completed".to_string()))
                        };
                    }
                };

                match message {
                    Message::Text(text) => {
                        let event: MiniMaxWsEvent = serde_json::from_str(&text).map_err(|error| {
                            BridgeError::Protocol(format!("failed to decode MiniMax websocket event: {error}"))
                        })?;
                        tracing::debug!(
                            event = event.event.as_deref().unwrap_or("unknown"),
                            has_data = event.data.is_some(),
                            "minimax ws event received"
                        );
                        if let Some(base_resp) = event.base_resp {
                            if base_resp.status_code.unwrap_or(0) != 0 {
                                return Err(BridgeError::Unavailable(
                                    base_resp
                                        .status_msg
                                        .unwrap_or_else(|| "MiniMax websocket provider error".to_string()),
                                ));
                            }
                        }

                        match event.event.as_deref() {
                            Some("task_started") => {
                                started = true;
                                if !pending_chunks.is_empty() {
                                    for chunk in pending_chunks.drain(..) {
                                        send_minimax_ws_message(
                                            &mut sink,
                                            json!({
                                                "event": "task_continue",
                                                "text": chunk
                                            }),
                                        ).await?;
                                    }
                                }
                                if committed {
                                    send_minimax_ws_message(
                                        &mut sink,
                                        json!({ "event": "task_finish" }),
                                    ).await?;
                                }
                            }
                            Some("task_finished") => return Ok(None),
                            Some("task_failed") => {
                                return Err(BridgeError::Unavailable("MiniMax websocket task failed".to_string()));
                            }
                            _ => {}
                        }

                        if let Some(data) = event.data {
                            if let Some(audio) = data.audio.or(data.audio_base64) {
                                let audio = audio.trim();
                                if audio.is_empty() {
                                    if event.is_final.unwrap_or(false) {
                                        return Ok(None);
                                    }
                                    continue;
                                }
                                tracing::debug!("minimax ws audio chunk received");
                                chunk_sequence += 1;
                                let bytes = decode_minimax_audio(&audio)?;
                                if bytes.is_empty() {
                                    if event.is_final.unwrap_or(false) {
                                        return Ok(None);
                                    }
                                    continue;
                                }
                                event_tx
                                    .send(BridgeTtsEvent::Audio {
                                        chunk: AudioChunk {
                                            bytes,
                                            sequence: chunk_sequence,
                                            is_final: false,
                                            format: if chunk_sequence == 1 { output_format.clone() } else { None },
                                        }
                                    })
                                    .await
                                    .map_err(|_| BridgeError::Disconnected("bridge event channel closed".to_string()))?;
                            }
                        }
                    }
                    Message::Binary(bytes) => {
                        chunk_sequence += 1;
                        event_tx
                            .send(BridgeTtsEvent::Audio {
                                chunk: AudioChunk {
                                    bytes: bytes.to_vec(),
                                    sequence: chunk_sequence,
                                    is_final: false,
                                    format: if chunk_sequence == 1 { output_format.clone() } else { None },
                                }
                            })
                            .await
                            .map_err(|_| BridgeError::Disconnected("bridge event channel closed".to_string()))?;
                    }
                    Message::Ping(payload) => {
                        sink.send(Message::Pong(payload)).await.map_err(|error| {
                            BridgeError::Disconnected(format!("MiniMax websocket pong failed: {error}"))
                        })?;
                    }
                    Message::Close(_) => {
                        return if committed {
                            Ok(None)
                        } else {
                            Err(BridgeError::Disconnected("MiniMax websocket closed early".to_string()))
                        };
                    }
                    _ => {}
                }
            }
            maybe_command = command_rx.recv() => {
                let Some(command) = maybe_command else {
                    return Ok(Some("stopped".to_string()));
                };
                match command {
                    BridgeTtsCommand::AppendInput(delta) => {
                        if delta.is_empty() {
                            continue;
                        }
                        if matches!(input_mode, StreamMode::Buffered) {
                            buffered.push_str(&delta);
                        } else if started {
                            send_minimax_ws_message(
                                &mut sink,
                                json!({
                                    "event": "task_continue",
                                    "text": delta
                                }),
                            ).await?;
                        } else {
                            let first_chunk = delta;
                            if !task_start_sent {
                                send_minimax_task_start(
                                    &mut sink,
                                    &model,
                                    Some(first_chunk),
                                    &voice_setting,
                                    &audio_setting,
                                    &extra,
                                ).await?;
                                task_start_sent = true;
                            } else {
                                pending_chunks.push(first_chunk);
                            }
                        }
                    }
                    BridgeTtsCommand::Commit => {
                        committed = true;
                        if matches!(input_mode, StreamMode::Buffered) {
                            let text = buffered.trim().to_string();
                            if text.is_empty() {
                                return Err(BridgeError::Unavailable("TTS input buffer is empty".to_string()));
                            }
                            if started {
                                send_minimax_ws_message(&mut sink, json!({
                                    "event": "task_continue",
                                    "text": text
                                })).await?;
                                send_minimax_ws_message(
                                    &mut sink,
                                    json!({ "event": "task_finish" }),
                                ).await?;
                            } else {
                                if !task_start_sent {
                                    send_minimax_task_start(
                                        &mut sink,
                                        &model,
                                        Some(text),
                                        &voice_setting,
                                        &audio_setting,
                                        &extra,
                                    ).await?;
                                    task_start_sent = true;
                                } else {
                                    pending_chunks.push(text);
                                }
                            }
                        } else if started {
                            send_minimax_ws_message(
                                &mut sink,
                                json!({ "event": "task_finish" }),
                            ).await?;
                        }
                    }
                    BridgeTtsCommand::Stop => {
                        let _ = send_minimax_ws_message(
                            &mut sink,
                            json!({ "event": "task_finish" }),
                        ).await;
                        return Ok(Some("stopped".to_string()));
                    }
                }
            }
        }
    }
}

// ---- WebSocket 辅助函数 ----

async fn send_minimax_task_start<S>(
    sink: &mut futures_util::stream::SplitSink<tokio_tungstenite::WebSocketStream<S>, Message>,
    model: &str,
    text: Option<String>,
    voice_setting: &Value,
    audio_setting: &Value,
    extra: &Value,
) -> Result<(), BridgeError>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let mut task_start = Map::new();
    task_start.insert("event".to_string(), Value::String("task_start".to_string()));
    task_start.insert("model".to_string(), Value::String(model.to_string()));
    task_start.insert("stream".to_string(), Value::Bool(true));
    task_start.insert("voice_setting".to_string(), voice_setting.clone());
    task_start.insert("audio_setting".to_string(), audio_setting.clone());
    if let Some(text) = text {
        task_start.insert("text".to_string(), Value::String(text));
    }
    if let Value::Object(extra_fields) = extra {
        for (key, value) in extra_fields {
            task_start.insert(key.clone(), value.clone());
        }
    }
    send_minimax_ws_message(sink, Value::Object(task_start)).await
}

fn minimax_ws_url(base_url: &str, _group_id: &str) -> String {
    let trimmed = base_url.trim_end_matches('/');
    let ws_base = if let Some(rest) = trimmed.strip_prefix("https://") {
        format!("wss://{rest}")
    } else if let Some(rest) = trimmed.strip_prefix("http://") {
        format!("ws://{rest}")
    } else {
        trimmed.to_string()
    };
    format!("{ws_base}/ws/v1/t2a_v2")
}

fn build_minimax_ws_settings(
    config: &MiniMaxHttpTtsBridgeConfig,
    options: &speechmesh_tts::SynthesisOptions,
    output_encoding: &str,
) -> Result<(Value, Value, Value, String), BridgeError> {
    let voice_id = options
        .provider_options
        .get("voice_id")
        .and_then(Value::as_str)
        .or(options.voice.as_deref())
        .unwrap_or(&config.default_voice_id);
    let model = options
        .provider_options
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or(&config.default_model)
        .to_string();

    let mut voice_setting = Map::new();
    voice_setting.insert("voice_id".to_string(), Value::String(voice_id.to_string()));
    if let Some(speed) = options.rate {
        voice_setting.insert("speed".to_string(), json!(speed));
    }
    if let Some(pitch) = options.pitch {
        voice_setting.insert("pitch".to_string(), json!(pitch));
    }
    if let Some(volume) = options.volume {
        voice_setting.insert("vol".to_string(), json!(volume));
    }
    if let Some(emotion) = options
        .provider_options
        .get("emotion")
        .and_then(Value::as_str)
        .or_else(|| {
            options
                .provider_options
                .get("emotion_tag")
                .and_then(Value::as_str)
        })
    {
        voice_setting.insert("emotion".to_string(), Value::String(emotion.to_string()));
    }

    let mut audio_setting = Map::new();
    audio_setting.insert(
        "sample_rate".to_string(),
        json!(config.default_sample_rate_hz),
    );
    audio_setting.insert(
        "format".to_string(),
        Value::String(output_encoding.to_string()),
    );
    audio_setting.insert("channel".to_string(), json!(1));
    if output_encoding == "mp3" {
        audio_setting.insert("bitrate".to_string(), json!(128000));
    }

    let extra = filter_reserved_provider_options(
        &options.provider_options,
        &[
            "route",
            "voice_id",
            "model",
            "format",
            "emotion",
            "emotion_tag",
            "input_mode",
            "output_mode",
        ],
    );
    Ok((
        Value::Object(voice_setting),
        Value::Object(audio_setting),
        extra,
        model,
    ))
}

async fn send_minimax_ws_message<S>(
    sink: &mut futures_util::stream::SplitSink<tokio_tungstenite::WebSocketStream<S>, Message>,
    payload: Value,
) -> Result<(), BridgeError>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    tracing::debug!(payload = %payload, "sending minimax ws message");
    sink.send(Message::Text(payload.to_string().into()))
        .await
        .map_err(|error| {
            BridgeError::Disconnected(format!("MiniMax websocket write failed: {error}"))
        })
}
