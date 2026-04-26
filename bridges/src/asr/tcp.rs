use async_trait::async_trait;
use speechmesh_asr::{AsrSession, StreamRequest};
use speechmesh_core::{Capability, CapabilityDomain, ProviderDescriptor, RuntimeMode, SessionId};
use tokio::net::TcpStream;
use tokio::sync::mpsc;

use super::stdio::{run_bridge_reader, run_bridge_writer};
use super::{
    AsrBridge, BridgeAsrEvent, BridgeAsrSessionHandle, BridgeCommand, asr_descriptor_with_io_modes,
    requested_asr_input_mode, requested_asr_output_mode,
};
use crate::BridgeError;

#[derive(Debug, Clone)]
pub struct TcpAsrBridgeConfig {
    pub provider_id: String,
    pub display_name: Option<String>,
    pub address: String,
}

pub struct TcpAsrBridge {
    config: TcpAsrBridgeConfig,
}

impl TcpAsrBridge {
    pub fn new(config: TcpAsrBridgeConfig) -> Self {
        Self { config }
    }
}

#[async_trait]
impl AsrBridge for TcpAsrBridge {
    fn descriptors(&self) -> Vec<ProviderDescriptor> {
        vec![
            asr_descriptor_with_io_modes(
                ProviderDescriptor::new(
                    self.config.provider_id.clone(),
                    self.config
                        .display_name
                        .clone()
                        .unwrap_or_else(|| "TCP ASR Bridge".to_string()),
                    CapabilityDomain::Asr,
                    RuntimeMode::RemoteGateway,
                ),
                true,
            )
            .with_capability(Capability::enabled("bridge-tcp")),
        ]
    }

    async fn start_stream(
        &self,
        request: StreamRequest,
    ) -> Result<BridgeAsrSessionHandle, BridgeError> {
        let stream = TcpStream::connect(&self.config.address)
            .await
            .map_err(|error| {
                BridgeError::Unavailable(format!(
                    "failed to connect to remote bridge {}: {error}",
                    self.config.address
                ))
            })?;

        let session_id = SessionId::new();
        let session = AsrSession {
            id: session_id,
            provider_id: self.config.provider_id.clone(),
            accepted_input_format: request.input_format.clone(),
            input_mode: requested_asr_input_mode(&request),
            output_mode: requested_asr_output_mode(&request),
        };

        let (command_tx, command_rx) = mpsc::channel::<BridgeCommand>(64);
        let (event_tx, event_rx) = mpsc::channel::<BridgeAsrEvent>(64);
        let (read_half, write_half) = tokio::io::split(stream);
        tokio::spawn(run_bridge_writer(
            session.id.clone(),
            request,
            write_half,
            command_rx,
        ));
        tokio::spawn(run_bridge_reader(read_half, event_tx));

        Ok(BridgeAsrSessionHandle::new(session, command_tx, event_rx))
    }
}
