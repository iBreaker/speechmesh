use async_trait::async_trait;
use speechmesh_core::{
    AudioEncoding, AudioFormat, Capability, CapabilityDomain, ProviderDescriptor, RuntimeMode,
    SessionId,
};
use speechmesh_transport::VoiceListRequest;
use speechmesh_tts::{AudioChunk, StreamRequest, TtsSession, VoiceDescriptor};
use tokio::sync::mpsc;

use super::{
    BridgeTtsCommand, BridgeTtsEvent, BridgeTtsSessionHandle, TtsBridge,
    ensure_tts_modes_supported, requested_tts_input_mode, requested_tts_output_mode,
};
use crate::BridgeError;

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tts::tests::mock_request;

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
