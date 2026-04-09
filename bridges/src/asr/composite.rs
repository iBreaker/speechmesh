use std::collections::HashSet;

use async_trait::async_trait;
use speechmesh_asr::StreamRequest;
use speechmesh_core::{ProviderDescriptor, ProviderSelectionMode};

use super::{
    AsrBridge, BridgeAsrSessionHandle, SharedAsrBridge, has_enabled_capability,
};
use crate::BridgeError;

#[derive(Clone)]
struct BridgeProviderBinding {
    descriptor: ProviderDescriptor,
    bridge: SharedAsrBridge,
}

pub struct CompositeAsrBridge {
    bindings: Vec<BridgeProviderBinding>,
}

impl CompositeAsrBridge {
    pub fn new(bridges: Vec<SharedAsrBridge>) -> Result<Self, BridgeError> {
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
                        "duplicate ASR provider id registered: {}",
                        descriptor.id
                    )));
                }
                bindings.push(BridgeProviderBinding {
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
        required_capabilities
            .iter()
            .all(|required| has_enabled_capability(descriptor, required))
    }

    fn preferred_score(descriptor: &ProviderDescriptor, request: &StreamRequest) -> usize {
        let preferred = request
            .provider
            .preferred_capabilities
            .iter()
            .filter(|capability| has_enabled_capability(descriptor, capability))
            .count();
        let on_device_bonus = usize::from(
            request.options.prefer_on_device && has_enabled_capability(descriptor, "on-device"),
        );
        preferred + on_device_bonus
    }

    fn select_binding(
        &self,
        request: &StreamRequest,
    ) -> Result<&BridgeProviderBinding, BridgeError> {
        let requested_provider_id = request.provider.provider_id.as_deref();

        if matches!(request.provider.mode, ProviderSelectionMode::Provider)
            || requested_provider_id.is_some()
        {
            let provider_id = requested_provider_id.ok_or_else(|| {
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
                        "requested provider {provider_id} is not installed on this gateway"
                    ))
                })?;

            if !Self::descriptor_matches_required(
                &binding.descriptor,
                &request.provider.required_capabilities,
            ) {
                return Err(BridgeError::Unavailable(format!(
                    "requested provider {provider_id} does not satisfy required capabilities"
                )));
            }

            return Ok(binding);
        }

        self.bindings
            .iter()
            .filter(|binding| {
                Self::descriptor_matches_required(
                    &binding.descriptor,
                    &request.provider.required_capabilities,
                )
            })
            .max_by_key(|binding| Self::preferred_score(&binding.descriptor, request))
            .ok_or_else(|| {
                BridgeError::Unavailable(
                    "no installed ASR provider satisfies the requested capabilities".to_string(),
                )
            })
    }
}

#[async_trait]
impl AsrBridge for CompositeAsrBridge {
    fn descriptors(&self) -> Vec<ProviderDescriptor> {
        self.bindings
            .iter()
            .map(|binding| binding.descriptor.clone())
            .collect()
    }

    async fn start_stream(
        &self,
        request: StreamRequest,
    ) -> Result<BridgeAsrSessionHandle, BridgeError> {
        let bridge = self.select_binding(&request)?.bridge.clone();
        bridge.start_stream(request).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use speechmesh_core::{
        AudioFormat, Capability, CapabilityDomain, ProviderSelectionMode, RuntimeMode, SessionId,
        StreamMode,
    };
    use speechmesh_asr::{AsrSession, RecognitionOptions};
    use serde_json::Value;
    use std::sync::Arc;
    use tokio::sync::mpsc;

    use crate::asr::BridgeCommand;

    struct TestBridge {
        descriptor: ProviderDescriptor,
    }

    #[async_trait]
    impl AsrBridge for TestBridge {
        fn descriptors(&self) -> Vec<ProviderDescriptor> {
            vec![self.descriptor.clone()]
        }

        async fn start_stream(
            &self,
            request: StreamRequest,
        ) -> Result<BridgeAsrSessionHandle, BridgeError> {
            let (command_tx, _command_rx) = mpsc::channel::<BridgeCommand>(4);
            let (_event_tx, event_rx) = mpsc::channel::<super::super::BridgeAsrEvent>(4);
            Ok(BridgeAsrSessionHandle::new(
                AsrSession {
                    id: SessionId::new(),
                    provider_id: self.descriptor.id.clone(),
                    accepted_input_format: request.input_format,
                    input_mode: StreamMode::Streaming,
                    output_mode: StreamMode::Streaming,
                },
                command_tx,
                event_rx,
            ))
        }
    }

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
            options: RecognitionOptions {
                provider_options: Value::Null,
                ..RecognitionOptions::default()
            },
        }
    }

    #[tokio::test]
    async fn composite_bridge_routes_explicit_provider_id() {
        let bridge = CompositeAsrBridge::new(vec![
            Arc::new(TestBridge {
                descriptor: ProviderDescriptor::new(
                    "apple.asr",
                    "Apple",
                    CapabilityDomain::Asr,
                    RuntimeMode::RemoteGateway,
                )
                .with_capability(Capability::enabled("streaming-input"))
                .with_capability(Capability::enabled("on-device")),
            }),
            Arc::new(TestBridge {
                descriptor: ProviderDescriptor::new(
                    "sensevoice.asr",
                    "SenseVoice",
                    CapabilityDomain::Asr,
                    RuntimeMode::RemoteGateway,
                )
                .with_capability(Capability::enabled("streaming-input")),
            }),
        ])
        .expect("composite should build");

        let session = bridge
            .start_stream(request_with_selector(
                ProviderSelectionMode::Provider,
                Some("sensevoice.asr"),
            ))
            .await
            .expect("provider should route");

        assert_eq!(session.session.provider_id, "sensevoice.asr");
    }

    #[tokio::test]
    async fn composite_bridge_prefers_on_device_when_requested() {
        let bridge = CompositeAsrBridge::new(vec![
            Arc::new(TestBridge {
                descriptor: ProviderDescriptor::new(
                    "sensevoice.asr",
                    "SenseVoice",
                    CapabilityDomain::Asr,
                    RuntimeMode::RemoteGateway,
                )
                .with_capability(Capability::enabled("streaming-input")),
            }),
            Arc::new(TestBridge {
                descriptor: ProviderDescriptor::new(
                    "apple.asr",
                    "Apple",
                    CapabilityDomain::Asr,
                    RuntimeMode::RemoteGateway,
                )
                .with_capability(Capability::enabled("streaming-input"))
                .with_capability(Capability::enabled("on-device")),
            }),
        ])
        .expect("composite should build");

        let mut request = request_with_selector(ProviderSelectionMode::Auto, None);
        request.options.prefer_on_device = true;

        let session = bridge
            .start_stream(request)
            .await
            .expect("auto selection should succeed");

        assert_eq!(session.session.provider_id, "apple.asr");
    }

    #[test]
    fn composite_bridge_rejects_duplicate_provider_ids() {
        let duplicate = CompositeAsrBridge::new(vec![
            Arc::new(TestBridge {
                descriptor: ProviderDescriptor::new(
                    "dup.asr",
                    "one",
                    CapabilityDomain::Asr,
                    RuntimeMode::LocalDaemon,
                ),
            }),
            Arc::new(TestBridge {
                descriptor: ProviderDescriptor::new(
                    "dup.asr",
                    "two",
                    CapabilityDomain::Asr,
                    RuntimeMode::RemoteGateway,
                ),
            }),
        ]);

        assert!(duplicate.is_err());
    }
}
