use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64_STANDARD};
use futures_util::{SinkExt, StreamExt};
use reqwest::Client;
use serde::Deserialize;
use serde_json::{Map, Value, json};
use speechmesh_core::{
    AudioEncoding, AudioFormat, Capability, CapabilityDomain, ProviderDescriptor,
    ProviderSelectionMode, RuntimeMode, SessionId, StreamMode,
};
use speechmesh_transport::VoiceListRequest;
use speechmesh_tts::{AudioChunk, StreamRequest, SynthesisInputKind, TtsSession, VoiceDescriptor};
use tokio::sync::mpsc;
use tokio_tungstenite::{
    connect_async,
    tungstenite::{client::IntoClientRequest, protocol::Message},
};

use crate::bridge_support::BridgeError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BridgeTtsEvent {
    Audio { chunk: AudioChunk },
    Ended { reason: Option<String> },
    Error { message: String },
}

#[derive(Debug, Clone)]
enum BridgeTtsCommand {
    AppendInput(String),
    Commit,
    Stop,
}

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

#[derive(Debug)]
pub struct BridgeTtsSessionHandle {
    pub session: TtsSession,
    pub input_kind: SynthesisInputKind,
    command_tx: mpsc::Sender<BridgeTtsCommand>,
    event_rx: Option<mpsc::Receiver<BridgeTtsEvent>>,
}

impl BridgeTtsSessionHandle {
    fn new(
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

pub struct MockTtsBridge {
    provider_id: String,
    display_name: Option<String>,
}

impl MockTtsBridge {
    pub fn new(provider_id: impl Into<String>) -> Self {
        Self {
            provider_id: provider_id.into(),
            display_name: None,
        }
    }

    pub fn with_display_name(
        provider_id: impl Into<String>,
        display_name: impl Into<String>,
    ) -> Self {
        Self {
            provider_id: provider_id.into(),
            display_name: Some(display_name.into()),
        }
    }
}

#[async_trait]
impl TtsBridge for MockTtsBridge {
    fn descriptors(&self) -> Vec<ProviderDescriptor> {
        vec![
            ProviderDescriptor::new(
                self.provider_id.clone(),
                self.display_name
                    .clone()
                    .unwrap_or_else(|| "Mock TTS Bridge".to_string()),
                CapabilityDomain::Tts,
                RuntimeMode::LocalDaemon,
            )
            .with_capability(Capability::enabled("voice-list"))
            .with_capability(Capability::enabled("buffered-input"))
            .with_capability(Capability::enabled("buffered-text-input"))
            .with_capability(Capability::enabled("buffered-output"))
            .with_capability(Capability::enabled("rate-control")),
        ]
    }

    async fn list_voices(
        &self,
        _request: VoiceListRequest,
    ) -> Result<Vec<VoiceDescriptor>, BridgeError> {
        Ok(vec![VoiceDescriptor {
            id: "mock.default".to_string(),
            language: "und".to_string(),
            display_name: "Mock Voice".to_string(),
            gender: None,
            capabilities: vec!["rate-control".to_string()],
        }])
    }

    async fn start_stream(
        &self,
        request: StreamRequest,
    ) -> Result<BridgeTtsSessionHandle, BridgeError> {
        let input_mode = requested_tts_input_mode(&request.options);
        let output_mode = requested_tts_output_mode(&request.options);
        ensure_tts_modes_supported(input_mode, output_mode, false, false, "Mock TTS")?;
        let accepted_output_format = request.output_format.clone().or_else(|| {
            Some(AudioFormat {
                encoding: AudioEncoding::Wav,
                sample_rate_hz: 16_000,
                channels: 1,
            })
        });
        let session = TtsSession {
            id: SessionId::new(),
            provider_id: self.provider_id.clone(),
            accepted_output_format,
            input_mode,
            output_mode,
        };
        let input_kind = request.input_kind;
        let (command_tx, mut command_rx) = mpsc::channel::<BridgeTtsCommand>(32);
        let (event_tx, event_rx) = mpsc::channel::<BridgeTtsEvent>(32);
        let output_format = session.accepted_output_format.clone();

        tokio::spawn(async move {
            let mut buffer = String::new();
            while let Some(command) = command_rx.recv().await {
                match command {
                    BridgeTtsCommand::AppendInput(delta) => buffer.push_str(&delta),
                    BridgeTtsCommand::Commit => {
                        let bytes = format!("MOCK_TTS:{buffer}").into_bytes();
                        let chunk = AudioChunk {
                            bytes,
                            sequence: 1,
                            is_final: true,
                            format: output_format.clone(),
                        };
                        if event_tx
                            .send(BridgeTtsEvent::Audio { chunk })
                            .await
                            .is_err()
                        {
                            return;
                        }
                        let _ = event_tx.send(BridgeTtsEvent::Ended { reason: None }).await;
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

#[derive(Debug, Clone)]
pub struct MeloHttpTtsBridgeConfig {
    pub provider_id: String,
    pub display_name: Option<String>,
    pub base_url: String,
    pub request_timeout: Duration,
    pub chunk_size_bytes: usize,
}

pub struct MeloHttpTtsBridge {
    config: MeloHttpTtsBridgeConfig,
    client: Client,
}

#[derive(Debug, Clone)]
pub struct QwenHttpTtsBridgeConfig {
    pub provider_id: String,
    pub display_name: Option<String>,
    pub base_url: String,
    pub request_timeout: Duration,
    pub chunk_size_bytes: usize,
    pub default_model: Option<String>,
    pub default_voice: Option<String>,
    pub default_language: Option<String>,
    pub default_sample_rate_hz: u32,
}

pub struct QwenHttpTtsBridge {
    config: QwenHttpTtsBridgeConfig,
    client: Client,
}

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

#[derive(Clone)]
struct TtsProviderBinding {
    descriptor: ProviderDescriptor,
    bridge: SharedTtsBridge,
}

pub struct CompositeTtsBridge {
    bindings: Vec<TtsProviderBinding>,
}

impl CompositeTtsBridge {
    pub fn new(bridges: Vec<SharedTtsBridge>) -> Result<Self, BridgeError> {
        let mut bindings = Vec::new();
        let mut seen_provider_ids = HashSet::new();

        for bridge in bridges {
            let descriptors = bridge.descriptors();
            if descriptors.is_empty() {
                return Err(BridgeError::Unavailable(
                    "bridge registered without any provider descriptors".to_string(),
                ));
            }

            for descriptor in descriptors {
                if !seen_provider_ids.insert(descriptor.id.clone()) {
                    return Err(BridgeError::Unavailable(format!(
                        "duplicate TTS provider id registered: {}",
                        descriptor.id
                    )));
                }
                bindings.push(TtsProviderBinding {
                    descriptor,
                    bridge: bridge.clone(),
                });
            }
        }

        Ok(Self { bindings })
    }

    fn descriptor_matches_required(
        descriptor: &ProviderDescriptor,
        required_capabilities: &[String],
    ) -> bool {
        required_capabilities.iter().all(|required| {
            descriptor
                .capabilities
                .iter()
                .any(|capability| capability.enabled && capability.key == *required)
        })
    }

    fn preferred_score(
        descriptor: &ProviderDescriptor,
        preferred_capabilities: &[String],
    ) -> usize {
        preferred_capabilities
            .iter()
            .filter(|required| {
                descriptor
                    .capabilities
                    .iter()
                    .any(|capability| capability.enabled && capability.key == **required)
            })
            .count()
    }

    fn select_binding(
        &self,
        provider_mode: ProviderSelectionMode,
        provider_id: Option<&str>,
        required_capabilities: &[String],
        preferred_capabilities: &[String],
    ) -> Result<&TtsProviderBinding, BridgeError> {
        if matches!(provider_mode, ProviderSelectionMode::Provider) || provider_id.is_some() {
            let provider_id = provider_id.ok_or_else(|| {
                BridgeError::Unavailable(
                    "provider mode requires a concrete provider_id".to_string(),
                )
            })?;
            let binding = self
                .bindings
                .iter()
                .find(|binding| binding.descriptor.id == provider_id)
                .ok_or_else(|| {
                    BridgeError::Unavailable(format!(
                        "requested provider {provider_id} is not available for TTS on this gateway"
                    ))
                })?;
            if !Self::descriptor_matches_required(&binding.descriptor, required_capabilities) {
                return Err(BridgeError::Unavailable(format!(
                    "requested provider {provider_id} does not satisfy required TTS capabilities"
                )));
            }
            return Ok(binding);
        }

        self.bindings
            .iter()
            .filter(|binding| {
                Self::descriptor_matches_required(&binding.descriptor, required_capabilities)
            })
            .max_by_key(|binding| {
                Self::preferred_score(&binding.descriptor, preferred_capabilities)
            })
            .ok_or_else(|| {
                BridgeError::Unavailable(
                    "no configured TTS provider satisfies the requested capabilities".to_string(),
                )
            })
    }

    fn apply_route_preferences(selector: &mut speechmesh_core::ProviderSelector, options: &Value) {
        if !matches!(selector.mode, ProviderSelectionMode::Auto) || selector.provider_id.is_some() {
            return;
        }

        let route = options
            .get("route")
            .and_then(Value::as_str)
            .map(|value| value.trim().to_ascii_lowercase());
        let Some(route) = route else {
            return;
        };

        let preferred = match route.as_str() {
            "realtime" | "real_time" | "low_latency" | "low-latency" => {
                &["realtime-low-latency", "cloud-managed"][..]
            }
            "quality" | "quality_first" | "quality-first" | "offline" | "local" => {
                &["quality-high", "local-inference"][..]
            }
            _ => &[][..],
        };

        for capability in preferred {
            if !selector
                .preferred_capabilities
                .iter()
                .any(|existing| existing == capability)
            {
                selector
                    .preferred_capabilities
                    .push((*capability).to_string());
            }
        }
    }
}

#[async_trait]
impl TtsBridge for CompositeTtsBridge {
    fn descriptors(&self) -> Vec<ProviderDescriptor> {
        self.bindings
            .iter()
            .map(|binding| binding.descriptor.clone())
            .collect()
    }

    async fn list_voices(
        &self,
        request: VoiceListRequest,
    ) -> Result<Vec<VoiceDescriptor>, BridgeError> {
        if matches!(request.provider.mode, ProviderSelectionMode::Provider)
            || request.provider.provider_id.is_some()
        {
            let binding = self.select_binding(
                request.provider.mode,
                request.provider.provider_id.as_deref(),
                &request.provider.required_capabilities,
                &request.provider.preferred_capabilities,
            )?;
            return binding.bridge.list_voices(request).await;
        }

        let mut voices = Vec::new();
        for binding in &self.bindings {
            if !Self::descriptor_matches_required(
                &binding.descriptor,
                &request.provider.required_capabilities,
            ) {
                continue;
            }
            voices.extend(binding.bridge.list_voices(request.clone()).await?);
        }
        Ok(voices)
    }

    async fn start_stream(
        &self,
        mut request: StreamRequest,
    ) -> Result<BridgeTtsSessionHandle, BridgeError> {
        Self::apply_route_preferences(&mut request.provider, &request.options.provider_options);
        let binding = self.select_binding(
            request.provider.mode,
            request.provider.provider_id.as_deref(),
            &request.provider.required_capabilities,
            &request.provider.preferred_capabilities,
        )?;
        binding.bridge.start_stream(request).await
    }
}

impl MeloHttpTtsBridge {
    pub fn new(config: MeloHttpTtsBridgeConfig) -> Result<Self, BridgeError> {
        let client = Client::builder()
            .timeout(config.request_timeout)
            .build()
            .map_err(|error| {
                BridgeError::Unavailable(format!("failed to build MeloTTS client: {error}"))
            })?;
        Ok(Self { config, client })
    }
}

impl QwenHttpTtsBridge {
    pub fn new(config: QwenHttpTtsBridgeConfig) -> Result<Self, BridgeError> {
        let client = Client::builder()
            .timeout(config.request_timeout)
            .build()
            .map_err(|error| {
                BridgeError::Unavailable(format!("failed to build Qwen3 TTS client: {error}"))
            })?;
        Ok(Self { config, client })
    }
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

#[derive(Debug, Clone, Deserialize)]
struct MeloHealthResponse {
    ok: bool,
    language: String,
    speaker: String,
    sample_rate: u32,
}

#[derive(Debug, serde::Serialize)]
struct MeloSynthesisRequest<'a> {
    text: &'a str,
    speed: f32,
}

#[derive(Debug, serde::Serialize)]
struct QwenHttpSynthesisRequest<'a> {
    text: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    model: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    voice: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    language: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    instruction: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    rate: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pitch: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    volume: Option<f32>,
    stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    output_format: Option<&'a str>,
    #[serde(skip_serializing_if = "Value::is_null")]
    extra: Value,
}

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

#[async_trait]
impl TtsBridge for MeloHttpTtsBridge {
    fn descriptors(&self) -> Vec<ProviderDescriptor> {
        vec![
            ProviderDescriptor::new(
                self.config.provider_id.clone(),
                self.config
                    .display_name
                    .clone()
                    .unwrap_or_else(|| "MeloTTS".to_string()),
                CapabilityDomain::Tts,
                RuntimeMode::LocalDaemon,
            )
            .with_capability(Capability::enabled("voice-list"))
            .with_capability(Capability::enabled("streaming-input"))
            .with_capability(Capability::enabled("buffered-input"))
            .with_capability(Capability::enabled("buffered-text-input"))
            .with_capability(Capability::enabled("streaming-output"))
            .with_capability(Capability::enabled("buffered-output"))
            .with_capability(Capability::enabled("rate-control"))
            .with_capability(Capability::enabled("wav-output")),
        ]
    }

    async fn list_voices(
        &self,
        request: VoiceListRequest,
    ) -> Result<Vec<VoiceDescriptor>, BridgeError> {
        let health = fetch_melo_health(&self.client, &self.config.base_url).await?;
        if let Some(language) = request.language.as_deref() {
            if language != health.language {
                return Ok(Vec::new());
            }
        }
        Ok(vec![VoiceDescriptor {
            id: health.speaker.clone(),
            language: health.language,
            display_name: format!("MeloTTS {}", health.speaker),
            gender: None,
            capabilities: vec!["rate-control".to_string(), "wav-output".to_string()],
        }])
    }

    async fn start_stream(
        &self,
        request: StreamRequest,
    ) -> Result<BridgeTtsSessionHandle, BridgeError> {
        let input_mode = requested_tts_input_mode(&request.options);
        let output_mode = requested_tts_output_mode(&request.options);
        ensure_tts_modes_supported(input_mode, output_mode, false, false, "MeloTTS")?;
        if request.input_kind != SynthesisInputKind::Text {
            return Err(BridgeError::Unavailable(
                "MeloTTS bridge currently supports text input only".to_string(),
            ));
        }

        let health = fetch_melo_health(&self.client, &self.config.base_url).await?;
        if let Some(language) = request.options.language.as_deref() {
            if language != health.language {
                return Err(BridgeError::Unavailable(format!(
                    "MeloTTS bridge is serving language {} but request asked for {}",
                    health.language, language
                )));
            }
        }
        if let Some(voice) = request.options.voice.as_deref() {
            if voice != health.speaker {
                return Err(BridgeError::Unavailable(format!(
                    "MeloTTS bridge exposes voice {} but request asked for {}",
                    health.speaker, voice
                )));
            }
        }

        let accepted_output_format = request.output_format.clone().or_else(|| {
            Some(AudioFormat {
                encoding: AudioEncoding::Wav,
                sample_rate_hz: health.sample_rate,
                channels: 1,
            })
        });
        if accepted_output_format
            .as_ref()
            .is_some_and(|format| format.encoding != AudioEncoding::Wav)
        {
            return Err(BridgeError::Unavailable(
                "MeloTTS bridge currently outputs WAV only".to_string(),
            ));
        }

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
        let base_url = self.config.base_url.clone();
        let chunk_size_bytes = self.config.chunk_size_bytes.max(1);
        let speed = request.options.rate.unwrap_or(1.0).clamp(0.5, 2.0);

        tokio::spawn(async move {
            let mut buffer = String::new();
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

                        match synthesize_melo(&client, &base_url, &text, speed).await {
                            Ok(audio_bytes) => {
                                let mut sequence = 0_u64;
                                if audio_bytes.is_empty() {
                                    let _ = event_tx
                                        .send(BridgeTtsEvent::Error {
                                            message: "MeloTTS returned empty audio".to_string(),
                                        })
                                        .await;
                                    let _ = event_tx
                                        .send(BridgeTtsEvent::Ended {
                                            reason: Some("empty_audio".to_string()),
                                        })
                                        .await;
                                    return;
                                }

                                let total_chunks =
                                    std::cmp::max(1, audio_bytes.len().div_ceil(chunk_size_bytes));
                                for (index, part) in
                                    audio_bytes.chunks(chunk_size_bytes).enumerate()
                                {
                                    sequence += 1;
                                    let chunk = AudioChunk {
                                        bytes: part.to_vec(),
                                        sequence,
                                        is_final: index + 1 == total_chunks,
                                        format: if index == 0 {
                                            output_format.clone()
                                        } else {
                                            None
                                        },
                                    };
                                    if event_tx
                                        .send(BridgeTtsEvent::Audio { chunk })
                                        .await
                                        .is_err()
                                    {
                                        return;
                                    }
                                }
                                let _ = event_tx.send(BridgeTtsEvent::Ended { reason: None }).await;
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

#[async_trait]
impl TtsBridge for QwenHttpTtsBridge {
    fn descriptors(&self) -> Vec<ProviderDescriptor> {
        vec![
            ProviderDescriptor::new(
                self.config.provider_id.clone(),
                self.config
                    .display_name
                    .clone()
                    .unwrap_or_else(|| "Qwen3 TTS".to_string()),
                CapabilityDomain::Tts,
                RuntimeMode::LocalDaemon,
            )
            .with_capability(Capability::enabled("voice-list"))
            .with_capability(Capability::enabled("streaming-input"))
            .with_capability(Capability::enabled("buffered-input"))
            .with_capability(Capability::enabled("buffered-text-input"))
            .with_capability(Capability::enabled("streaming-output"))
            .with_capability(Capability::enabled("buffered-output"))
            .with_capability(Capability::enabled("rate-control"))
            .with_capability(Capability::enabled("pitch-control"))
            .with_capability(Capability::enabled("volume-control"))
            .with_capability(Capability::enabled("instruction-control"))
            .with_capability(Capability::enabled("wav-output"))
            .with_capability(Capability::enabled("quality-high"))
            .with_capability(Capability::enabled("local-inference")),
        ]
    }

    async fn list_voices(
        &self,
        request: VoiceListRequest,
    ) -> Result<Vec<VoiceDescriptor>, BridgeError> {
        let language = self
            .config
            .default_language
            .clone()
            .unwrap_or_else(|| "und".to_string());
        if let Some(filter) = request.language.as_deref() {
            if filter != language {
                return Ok(Vec::new());
            }
        }
        Ok(vec![VoiceDescriptor {
            id: self
                .config
                .default_voice
                .clone()
                .unwrap_or_else(|| "default".to_string()),
            language,
            display_name: self
                .config
                .display_name
                .clone()
                .unwrap_or_else(|| "Qwen3 TTS".to_string()),
            gender: None,
            capabilities: vec![
                "rate-control".to_string(),
                "pitch-control".to_string(),
                "volume-control".to_string(),
                "instruction-control".to_string(),
                "wav-output".to_string(),
                "quality-high".to_string(),
                "local-inference".to_string(),
            ],
        }])
    }

    async fn start_stream(
        &self,
        request: StreamRequest,
    ) -> Result<BridgeTtsSessionHandle, BridgeError> {
        let input_mode = requested_tts_input_mode(&request.options);
        let output_mode = requested_tts_output_mode(&request.options);
        ensure_tts_modes_supported(input_mode, output_mode, false, false, "Qwen3 TTS")?;
        if request.input_kind != SynthesisInputKind::Text {
            return Err(BridgeError::Unavailable(
                "Qwen3 TTS bridge currently supports text input only".to_string(),
            ));
        }

        let accepted_output_format = request.output_format.clone().or_else(|| {
            Some(AudioFormat {
                encoding: AudioEncoding::Wav,
                sample_rate_hz: self.config.default_sample_rate_hz,
                channels: 1,
            })
        });
        if accepted_output_format
            .as_ref()
            .is_some_and(|format| format.encoding != AudioEncoding::Wav)
        {
            return Err(BridgeError::Unavailable(
                "Qwen3 TTS bridge currently outputs WAV only".to_string(),
            ));
        }

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

        tokio::spawn(async move {
            let mut buffer = String::new();
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

                        match synthesize_qwen_http(&client, &config, &text, &options).await {
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
                                let _ = event_tx.send(BridgeTtsEvent::Ended { reason: None }).await;
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
                                let _ = event_tx.send(BridgeTtsEvent::Ended { reason: None }).await;
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

async fn fetch_melo_health(
    client: &Client,
    base_url: &str,
) -> Result<MeloHealthResponse, BridgeError> {
    let url = format!("{}/healthz", base_url.trim_end_matches('/'));
    let response = client.get(&url).send().await.map_err(|error| {
        BridgeError::Unavailable(format!(
            "failed to reach MeloTTS health endpoint {url}: {error}"
        ))
    })?;
    if !response.status().is_success() {
        return Err(BridgeError::Unavailable(format!(
            "MeloTTS health endpoint {url} returned status {}",
            response.status()
        )));
    }
    let payload = response
        .json::<MeloHealthResponse>()
        .await
        .map_err(|error| {
            BridgeError::Protocol(format!("failed to decode MeloTTS health response: {error}"))
        })?;
    if !payload.ok {
        return Err(BridgeError::Unavailable(
            "MeloTTS health endpoint reported not ready".to_string(),
        ));
    }
    Ok(payload)
}

async fn synthesize_melo(
    client: &Client,
    base_url: &str,
    text: &str,
    speed: f32,
) -> Result<Vec<u8>, BridgeError> {
    let url = format!("{}/v1/tts", base_url.trim_end_matches('/'));
    let response = client
        .post(&url)
        .json(&MeloSynthesisRequest { text, speed })
        .send()
        .await
        .map_err(|error| {
            BridgeError::Unavailable(format!(
                "failed to call MeloTTS synth endpoint {url}: {error}"
            ))
        })?;
    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(BridgeError::Unavailable(format!(
            "MeloTTS synth endpoint {url} returned status {status}: {body}"
        )));
    }
    let bytes = response
        .bytes()
        .await
        .map_err(|error| BridgeError::Io(format!("failed to read MeloTTS audio body: {error}")))?;
    Ok(bytes.to_vec())
}

async fn synthesize_qwen_http(
    client: &Client,
    config: &QwenHttpTtsBridgeConfig,
    text: &str,
    options: &speechmesh_tts::SynthesisOptions,
) -> Result<Vec<u8>, BridgeError> {
    let url = format!("{}/v1/tts", config.base_url.trim_end_matches('/'));
    let instruction = options
        .provider_options
        .get("instruction")
        .and_then(Value::as_str);
    let extra = filter_reserved_provider_options(
        &options.provider_options,
        &[
            "route",
            "instruction",
            "voice",
            "language",
            "model",
            "output_format",
        ],
    );
    let model = options
        .provider_options
        .get("model")
        .and_then(Value::as_str)
        .or(config.default_model.as_deref());
    let voice = options
        .provider_options
        .get("voice")
        .and_then(Value::as_str)
        .or(options.voice.as_deref())
        .or(config.default_voice.as_deref());
    let language = options
        .provider_options
        .get("language")
        .and_then(Value::as_str)
        .or(options.language.as_deref())
        .or(config.default_language.as_deref());
    let output_format = options
        .provider_options
        .get("output_format")
        .and_then(Value::as_str)
        .or(Some("wav"));

    let response = client
        .post(&url)
        .json(&QwenHttpSynthesisRequest {
            text,
            model,
            voice,
            language,
            instruction,
            rate: options.rate,
            pitch: options.pitch,
            volume: options.volume,
            stream: false,
            output_format,
            extra,
        })
        .send()
        .await
        .map_err(|error| {
            BridgeError::Unavailable(format!(
                "failed to call Qwen3 TTS synth endpoint {url}: {error}"
            ))
        })?;
    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(BridgeError::Unavailable(format!(
            "Qwen3 TTS synth endpoint {url} returned status {status}: {body}"
        )));
    }
    let bytes = response.bytes().await.map_err(|error| {
        BridgeError::Io(format!("failed to read Qwen3 TTS audio body: {error}"))
    })?;
    Ok(bytes.to_vec())
}

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

async fn emit_audio_chunks(
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

fn filter_reserved_provider_options(options: &Value, reserved_keys: &[&str]) -> Value {
    let Some(map) = options.as_object() else {
        return Value::Null;
    };
    let filtered = map
        .iter()
        .filter(|(key, _)| !reserved_keys.iter().any(|reserved| reserved == key))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect::<Map<String, Value>>();
    if filtered.is_empty() {
        Value::Null
    } else {
        Value::Object(filtered)
    }
}

fn normalize_output_format_name(
    encoding: Option<AudioEncoding>,
    fallback: &str,
) -> Result<String, BridgeError> {
    let resolved = match encoding {
        Some(AudioEncoding::Wav) => "wav",
        Some(AudioEncoding::Mp3) => "mp3",
        Some(AudioEncoding::Flac) => "flac",
        Some(other) => {
            return Err(BridgeError::Unavailable(format!(
                "unsupported TTS output encoding for upstream provider: {other:?}"
            )));
        }
        None => fallback,
    };
    Ok(resolved.trim().to_ascii_lowercase())
}

fn audio_encoding_from_name(name: &str) -> Result<AudioEncoding, BridgeError> {
    match name.trim().to_ascii_lowercase().as_str() {
        "wav" => Ok(AudioEncoding::Wav),
        "mp3" => Ok(AudioEncoding::Mp3),
        "flac" => Ok(AudioEncoding::Flac),
        other => Err(BridgeError::Unavailable(format!(
            "unsupported audio format requested from upstream provider: {other}"
        ))),
    }
}

fn decode_minimax_audio(encoded: &str) -> Result<Vec<u8>, BridgeError> {
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

fn requested_tts_input_mode(options: &speechmesh_tts::SynthesisOptions) -> StreamMode {
    parse_stream_mode(
        options.provider_options.get("input_mode"),
        StreamMode::Buffered,
    )
}

fn requested_tts_output_mode(options: &speechmesh_tts::SynthesisOptions) -> StreamMode {
    if let Some(explicit) = options.provider_options.get("output_mode") {
        return parse_stream_mode(Some(explicit), StreamMode::Buffered);
    }
    if options.stream {
        StreamMode::Streaming
    } else {
        StreamMode::Buffered
    }
}

fn ensure_tts_modes_supported(
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

#[cfg(test)]
mod tests {
    use super::*;
    use speechmesh_core::ProviderSelectionMode;
    use speechmesh_tts::SynthesisOptions;

    fn mock_request() -> StreamRequest {
        StreamRequest {
            provider: speechmesh_core::ProviderSelector {
                mode: ProviderSelectionMode::Auto,
                provider_id: None,
                required_capabilities: Vec::new(),
                preferred_capabilities: Vec::new(),
            },
            input_kind: SynthesisInputKind::Text,
            output_format: Some(AudioFormat {
                encoding: AudioEncoding::Wav,
                sample_rate_hz: 16_000,
                channels: 1,
            }),
            options: SynthesisOptions::default(),
        }
    }

    #[tokio::test]
    async fn mock_tts_bridge_buffers_text_until_commit() {
        let bridge = MockTtsBridge::new("mock.tts");
        let mut session = bridge
            .start_stream(mock_request())
            .await
            .expect("start stream");
        session
            .append_input("hello".to_string())
            .await
            .expect("append input");
        session.commit().await.expect("commit");
        let mut events = session.take_event_rx().expect("event stream");
        let audio = events.recv().await.expect("audio event");
        let ended = events.recv().await.expect("ended event");

        match audio {
            BridgeTtsEvent::Audio { chunk } => {
                assert!(String::from_utf8_lossy(&chunk.bytes).contains("hello"));
                assert!(chunk.is_final);
            }
            other => panic!("unexpected event: {other:?}"),
        }
        assert_eq!(ended, BridgeTtsEvent::Ended { reason: None });
    }

    #[tokio::test]
    async fn composite_tts_bridge_routes_explicit_provider_id() {
        let bridge = CompositeTtsBridge::new(vec![
            Arc::new(MockTtsBridge::with_display_name("mock.a", "Mock A")),
            Arc::new(MockTtsBridge::with_display_name("mock.b", "Mock B")),
        ])
        .expect("composite should build");

        let mut request = mock_request();
        request.provider = speechmesh_core::ProviderSelector {
            mode: ProviderSelectionMode::Provider,
            provider_id: Some("mock.b".to_string()),
            required_capabilities: Vec::new(),
            preferred_capabilities: Vec::new(),
        };

        let session = bridge.start_stream(request).await.expect("route tts");
        assert_eq!(session.session.provider_id, "mock.b");
    }

    #[tokio::test]
    async fn composite_tts_bridge_prefers_realtime_route_capability() {
        let bridge = CompositeTtsBridge::new(vec![
            Arc::new(MockTtsBridge::with_display_name("mock.quality", "Quality")),
            Arc::new(MockTtsBridge::with_display_name(
                "mock.realtime",
                "Realtime",
            )),
        ])
        .expect("composite should build");

        let mut bridge = bridge;
        bridge.bindings[0]
            .descriptor
            .capabilities
            .push(Capability::enabled("quality-high"));
        bridge.bindings[1]
            .descriptor
            .capabilities
            .push(Capability::enabled("realtime-low-latency"));

        let mut request = mock_request();
        request.options = SynthesisOptions {
            provider_options: json!({ "route": "realtime" }),
            ..SynthesisOptions::default()
        };

        let session = bridge.start_stream(request).await.expect("route tts");
        assert_eq!(session.session.provider_id, "mock.realtime");
    }

    #[tokio::test]
    async fn composite_tts_bridge_prefers_quality_route_capability() {
        let bridge = CompositeTtsBridge::new(vec![
            Arc::new(MockTtsBridge::with_display_name("mock.quality", "Quality")),
            Arc::new(MockTtsBridge::with_display_name(
                "mock.realtime",
                "Realtime",
            )),
        ])
        .expect("composite should build");

        let mut bridge = bridge;
        bridge.bindings[0]
            .descriptor
            .capabilities
            .push(Capability::enabled("quality-high"));
        bridge.bindings[1]
            .descriptor
            .capabilities
            .push(Capability::enabled("realtime-low-latency"));

        let mut request = mock_request();
        request.options = SynthesisOptions {
            provider_options: json!({ "route": "quality" }),
            ..SynthesisOptions::default()
        };

        let session = bridge.start_stream(request).await.expect("route tts");
        assert_eq!(session.session.provider_id, "mock.quality");
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

    #[tokio::test]
    async fn mock_tts_bridge_rejects_streaming_output_mode() {
        let bridge = MockTtsBridge::new("mock.tts");
        let mut request = mock_request();
        request.options.stream = true;

        let error = bridge
            .start_stream(request)
            .await
            .expect_err("should reject");
        assert!(error.to_string().contains("streaming TTS output"));
    }
}
