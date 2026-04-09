use std::time::Duration;

use async_trait::async_trait;
use reqwest::Client;
use serde_json::Value;
use speechmesh_core::{
    AudioEncoding, AudioFormat, Capability, CapabilityDomain, ProviderDescriptor, RuntimeMode,
    SessionId,
};
use speechmesh_tts::{StreamRequest, SynthesisInputKind, TtsSession, VoiceDescriptor};
use speechmesh_transport::VoiceListRequest;
use tokio::sync::mpsc;

use super::{
    BridgeTtsCommand, BridgeTtsEvent, BridgeTtsSessionHandle, TtsBridge, emit_audio_chunks,
    ensure_tts_modes_supported, filter_reserved_provider_options, requested_tts_input_mode,
    requested_tts_output_mode,
};
use crate::BridgeError;

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
