use std::time::Duration;

use async_trait::async_trait;
use reqwest::Client;
use serde::Deserialize;
use speechmesh_core::{
    AudioEncoding, AudioFormat, Capability, CapabilityDomain, ProviderDescriptor, RuntimeMode,
    SessionId,
};
use speechmesh_tts::{AudioChunk, StreamRequest, SynthesisInputKind, TtsSession, VoiceDescriptor};
use speechmesh_transport::VoiceListRequest;
use tokio::sync::mpsc;

use super::{
    BridgeTtsCommand, BridgeTtsEvent, BridgeTtsSessionHandle, TtsBridge,
    ensure_tts_modes_supported, requested_tts_input_mode, requested_tts_output_mode,
};
use crate::BridgeError;

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
