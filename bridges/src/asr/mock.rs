use async_trait::async_trait;
use speechmesh_asr::{AsrSession, StreamRequest, Transcript};
use speechmesh_core::{
    Capability, CapabilityDomain, ProviderDescriptor, RuntimeMode, SessionId, StreamMode,
};
use tokio::sync::mpsc;

use super::{
    AsrBridge, BridgeAsrEvent, BridgeAsrSessionHandle, BridgeCommand,
    asr_descriptor_with_io_modes, requested_asr_input_mode, requested_asr_output_mode,
};
use crate::BridgeError;

pub struct MockAsrBridge {
    provider_id: String,
    display_name: Option<String>,
}

impl MockAsrBridge {
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
impl AsrBridge for MockAsrBridge {
    fn descriptors(&self) -> Vec<ProviderDescriptor> {
        vec![
            asr_descriptor_with_io_modes(
                ProviderDescriptor::new(
                    self.provider_id.clone(),
                    self.display_name
                        .clone()
                        .unwrap_or_else(|| "Mock ASR Bridge".to_string()),
                    CapabilityDomain::Asr,
                    RuntimeMode::LocalDaemon,
                ),
                true,
            )
            .with_capability(Capability::enabled("interim-results")),
        ]
    }

    async fn start_stream(
        &self,
        request: StreamRequest,
    ) -> Result<BridgeAsrSessionHandle, BridgeError> {
        let input_mode = requested_asr_input_mode(&request);
        let output_mode = requested_asr_output_mode(&request);
        let (command_tx, mut command_rx) = mpsc::channel::<BridgeCommand>(64);
        let (event_tx, event_rx) = mpsc::channel::<BridgeAsrEvent>(64);
        let provider_id = self.provider_id.clone();
        let session_id = SessionId::new();
        let session = AsrSession {
            id: session_id,
            provider_id: provider_id.clone(),
            accepted_input_format: request.input_format,
            input_mode,
            output_mode,
        };

        tokio::spawn(async move {
            let mut buffered_bytes: usize = 0;
            while let Some(command) = command_rx.recv().await {
                match command {
                    BridgeCommand::PushAudio(chunk) => {
                        buffered_bytes += chunk.len();
                        if matches!(output_mode, StreamMode::Streaming) {
                            let text = format!("mock partial bytes={buffered_bytes}");
                            if event_tx
                                .send(BridgeAsrEvent::Partial { text })
                                .await
                                .is_err()
                            {
                                return;
                            }
                        }
                    }
                    BridgeCommand::Commit => {
                        let transcript = Transcript {
                            text: format!("mock transcript bytes={buffered_bytes}"),
                            language: Some("en-US".to_string()),
                            segments: Vec::new(),
                        };
                        if event_tx
                            .send(BridgeAsrEvent::Final { transcript })
                            .await
                            .is_err()
                        {
                            return;
                        }
                        let _ = event_tx.send(BridgeAsrEvent::Ended { reason: None }).await;
                        return;
                    }
                    BridgeCommand::Stop => {
                        let _ = event_tx
                            .send(BridgeAsrEvent::Ended {
                                reason: Some("stopped".to_string()),
                            })
                            .await;
                        return;
                    }
                }
            }
        });

        Ok(BridgeAsrSessionHandle::new(session, command_tx, event_rx))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use speechmesh_core::{AudioFormat, ProviderSelectionMode};

    fn request_with_selector(
        mode: ProviderSelectionMode,
        provider_id: Option<&str>,
    ) -> StreamRequest {
        StreamRequest {
            provider: speechmesh_core::ProviderSelector {
                mode,
                provider_id: provider_id.map(ToString::to_string),
                required_capabilities: Vec::new(),
                preferred_capabilities: Vec::new(),
            },
            input_format: AudioFormat::pcm_s16le(16_000, 1),
            options: speechmesh_asr::RecognitionOptions {
                provider_options: serde_json::Value::Null,
                ..speechmesh_asr::RecognitionOptions::default()
            },
        }
    }

    #[tokio::test]
    async fn mock_asr_bridge_buffers_output_when_requested() {
        let bridge = MockAsrBridge::new("mock.asr");
        let mut request = request_with_selector(ProviderSelectionMode::Auto, None);
        request.options.provider_options = serde_json::json!({ "output_mode": "buffered" });

        let mut session = bridge.start_stream(request).await.expect("start");
        session
            .push_audio(vec![1, 2, 3, 4])
            .await
            .expect("push audio");
        session.commit().await.expect("commit");
        let mut events = session.take_event_rx().expect("event rx");

        match events.recv().await.expect("first event") {
            BridgeAsrEvent::Final { transcript } => {
                assert!(transcript.text.contains("bytes=4"));
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }
}
