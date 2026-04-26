use std::collections::HashSet;

use async_trait::async_trait;
use serde_json::Value;
use speechmesh_core::{ProviderDescriptor, ProviderSelectionMode};
use speechmesh_transport::VoiceListRequest;
use speechmesh_tts::{StreamRequest, VoiceDescriptor};

use super::{BridgeTtsSessionHandle, SharedTtsBridge, TtsBridge};
use crate::BridgeError;

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tts::MockTtsBridge;
    use crate::tts::tests::mock_request;
    use serde_json::json;
    use speechmesh_core::{Capability, ProviderSelectionMode};
    use speechmesh_tts::SynthesisOptions;
    use std::sync::Arc;

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
}
