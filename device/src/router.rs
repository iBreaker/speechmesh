//! 设备路由选择逻辑
//!
//! 提供按 endpoint 粒度的路由选择，扩展原有的 device_id 级别路由。

use crate::model::{DeviceId, EndpointDirection, EndpointId};
use crate::registry::{AgentRegistry, AgentSnapshotFilter, RegisteredAgent};

/// 路由选择结果，包含匹配的 agent 和可选的端点信息
#[derive(Debug, Clone)]
pub struct RouteMatch<Cmd: Send + Clone + 'static> {
    pub agent: RegisteredAgent<Cmd>,
    pub endpoint_id: Option<EndpointId>,
}

impl<Cmd: Send + Clone + 'static> AgentRegistry<Cmd> {
    /// 按 device_id + endpoint_id 精确路由
    ///
    /// 在 device_info 中查找指定端点，如果找到则返回对应 agent。
    /// 如果 agent 没有 device_info（旧版），回退到 device_id 匹配。
    pub async fn select_by_endpoint(
        &self,
        device_id: &DeviceId,
        endpoint_id: &EndpointId,
    ) -> Option<RouteMatch<Cmd>> {
        let agent = self
            .find_agent_with_endpoint(device_id, Some(endpoint_id))
            .await?;
        Some(RouteMatch {
            endpoint_id: Some(endpoint_id.clone()),
            agent,
        })
    }

    /// 按 device_id + 端点方向路由
    ///
    /// 在 device_info 中查找第一个匹配指定方向的端点。
    /// 如果 agent 没有 device_info（旧版），当方向为 Output 或 Duplex 时回退到 device_id 匹配。
    pub async fn select_by_direction(
        &self,
        device_id: &DeviceId,
        direction: EndpointDirection,
    ) -> Option<RouteMatch<Cmd>> {
        self.find_agent_by_direction(device_id, direction).await
    }

    /// 内部方法：查找指定设备上具有特定端点的 agent
    async fn find_agent_with_endpoint(
        &self,
        device_id: &DeviceId,
        endpoint_id: Option<&EndpointId>,
    ) -> Option<RegisteredAgent<Cmd>> {
        let agents = self
            .snapshot(AgentSnapshotFilter {
                device_id: Some(device_id.as_str().to_string()),
                ..Default::default()
            })
            .await;

        for snapshot in &agents {
            if let Some(device_info) = &snapshot.device_info {
                if let Some(ep_id) = endpoint_id {
                    if device_info.endpoint(ep_id).is_some() {
                        return self.get_registered_agent(&snapshot.agent_id).await;
                    }
                } else {
                    return self.get_registered_agent(&snapshot.agent_id).await;
                }
            } else {
                // 旧版 agent 没有 device_info，如果 device_id 匹配就返回
                let agent_device_id = snapshot
                    .device
                    .as_ref()
                    .map(|d| d.device_id.as_str());
                if agent_device_id == Some(device_id.as_str()) {
                    return self.get_registered_agent(&snapshot.agent_id).await;
                }
            }
        }
        None
    }

    /// 内部方法：查找指定设备上具有特定方向端点的 agent
    async fn find_agent_by_direction(
        &self,
        device_id: &DeviceId,
        direction: EndpointDirection,
    ) -> Option<RouteMatch<Cmd>> {
        let agents = self
            .snapshot(AgentSnapshotFilter {
                device_id: Some(device_id.as_str().to_string()),
                ..Default::default()
            })
            .await;

        for snapshot in &agents {
            if let Some(device_info) = &snapshot.device_info {
                // 新版：在端点中查找匹配方向的
                let matched_endpoint = device_info.endpoints.iter().find(|ep| {
                    ep.direction == direction
                        || (direction == EndpointDirection::Output
                            && ep.direction == EndpointDirection::Duplex)
                        || (direction == EndpointDirection::Input
                            && ep.direction == EndpointDirection::Duplex)
                });
                if let Some(ep) = matched_endpoint {
                    if let Some(agent) = self.get_registered_agent(&snapshot.agent_id).await {
                        return Some(RouteMatch {
                            agent,
                            endpoint_id: Some(ep.id.clone()),
                        });
                    }
                }
            } else {
                // 旧版：假设默认全双工，Output 和 Input 都匹配
                let agent_device_id = snapshot
                    .device
                    .as_ref()
                    .map(|d| d.device_id.as_str());
                if agent_device_id == Some(device_id.as_str()) {
                    if let Some(agent) = self.get_registered_agent(&snapshot.agent_id).await {
                        return Some(RouteMatch {
                            agent,
                            endpoint_id: None,
                        });
                    }
                }
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{AudioEndpoint, Device};
    use crate::registry::{AgentDeviceIdentity, AgentKind, RegisteredAgent};
    use speechmesh_core::CapabilityDomain;

    fn make_agent_with_device_info(
        agent_id: &str,
        device: Device,
    ) -> RegisteredAgent<String> {
        let (tx, _rx) = tokio::sync::mpsc::channel(1);
        RegisteredAgent {
            agent_id: agent_id.to_string(),
            agent_name: format!("test-{agent_id}"),
            provider_id: None,
            capabilities: vec!["speaker".to_string()],
            capability_domains: vec![CapabilityDomain::Tts],
            agent_kind: AgentKind::Device,
            device: Some(AgentDeviceIdentity {
                device_id: device.id.as_str().to_string(),
                hostname: device.hostname.clone(),
                platform: device.platform.clone(),
            }),
            device_info: Some(device),
            command_tx: tx,
        }
    }

    fn make_legacy_agent(agent_id: &str, device_id: &str) -> RegisteredAgent<String> {
        let (tx, _rx) = tokio::sync::mpsc::channel(1);
        RegisteredAgent {
            agent_id: agent_id.to_string(),
            agent_name: format!("test-{agent_id}"),
            provider_id: None,
            capabilities: vec!["speaker".to_string()],
            capability_domains: vec![CapabilityDomain::Tts],
            agent_kind: AgentKind::Device,
            device: Some(AgentDeviceIdentity {
                device_id: device_id.to_string(),
                hostname: None,
                platform: None,
            }),
            device_info: None,
            command_tx: tx,
        }
    }

    #[tokio::test]
    async fn select_by_endpoint_with_device_info() {
        let registry = AgentRegistry::<String>::new(None);
        let device = Device {
            id: DeviceId("d1".to_string()),
            hostname: None,
            platform: None,
            endpoints: vec![AudioEndpoint {
                id: EndpointId("spk".to_string()),
                display_name: None,
                direction: EndpointDirection::Output,
                capabilities: Vec::new(),
                supported_formats: Vec::new(),
            }],
        };
        registry
            .register_agent(make_agent_with_device_info("a1", device))
            .await;

        let result = registry
            .select_by_endpoint(&DeviceId("d1".to_string()), &EndpointId("spk".to_string()))
            .await;
        assert!(result.is_some());
        assert_eq!(result.unwrap().agent.agent_id, "a1");
    }

    #[tokio::test]
    async fn select_by_direction_fallback_for_legacy() {
        let registry = AgentRegistry::<String>::new(None);
        registry
            .register_agent(make_legacy_agent("a2", "legacy-device"))
            .await;

        let result = registry
            .select_by_direction(
                &DeviceId("legacy-device".to_string()),
                EndpointDirection::Output,
            )
            .await;
        assert!(result.is_some());
        let m = result.unwrap();
        assert_eq!(m.agent.agent_id, "a2");
        // 旧版没有端点信息
        assert!(m.endpoint_id.is_none());
    }

    #[tokio::test]
    async fn select_by_direction_with_device_info() {
        let registry = AgentRegistry::<String>::new(None);
        let device = Device {
            id: DeviceId("d2".to_string()),
            hostname: None,
            platform: None,
            endpoints: vec![
                AudioEndpoint {
                    id: EndpointId("mic".to_string()),
                    display_name: None,
                    direction: EndpointDirection::Input,
                    capabilities: Vec::new(),
                    supported_formats: Vec::new(),
                },
                AudioEndpoint {
                    id: EndpointId("spk".to_string()),
                    display_name: None,
                    direction: EndpointDirection::Output,
                    capabilities: Vec::new(),
                    supported_formats: Vec::new(),
                },
            ],
        };
        registry
            .register_agent(make_agent_with_device_info("a3", device))
            .await;

        // 查找输出端点
        let result = registry
            .select_by_direction(
                &DeviceId("d2".to_string()),
                EndpointDirection::Output,
            )
            .await;
        assert!(result.is_some());
        let m = result.unwrap();
        assert_eq!(m.endpoint_id.unwrap().as_str(), "spk");

        // 查找输入端点
        let result = registry
            .select_by_direction(
                &DeviceId("d2".to_string()),
                EndpointDirection::Input,
            )
            .await;
        assert!(result.is_some());
        let m = result.unwrap();
        assert_eq!(m.endpoint_id.unwrap().as_str(), "mic");
    }
}
