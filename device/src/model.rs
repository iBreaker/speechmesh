//! 设备领域模型
//!
//! Device 是聚合根，AudioEndpoint 是实体，
//! DeviceId / EndpointId / EndpointDirection 是值对象。

use serde::{Deserialize, Serialize};
use speechmesh_core::AudioFormat;

/// 设备唯一标识
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct DeviceId(pub String);

impl DeviceId {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<T: Into<String>> From<T> for DeviceId {
    fn from(value: T) -> Self {
        Self(value.into())
    }
}

/// 端点唯一标识
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct EndpointId(pub String);

impl EndpointId {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<T: Into<String>> From<T> for EndpointId {
    fn from(value: T) -> Self {
        Self(value.into())
    }
}

/// 端点方向：输入（麦克风）、输出（扬声器）、全双工
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EndpointDirection {
    /// 麦克风等音频输入
    Input,
    /// 扬声器等音频输出
    Output,
    /// 全双工，同时支持输入和输出
    Duplex,
}

impl EndpointDirection {
    /// 判断该方向是否能作为输入使用
    pub fn supports_input(&self) -> bool {
        matches!(self, EndpointDirection::Input | EndpointDirection::Duplex)
    }

    /// 判断该方向是否能作为输出使用
    pub fn supports_output(&self) -> bool {
        matches!(self, EndpointDirection::Output | EndpointDirection::Duplex)
    }
}

/// 音频端点，描述设备上的一个音频输入/输出通道
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioEndpoint {
    pub id: EndpointId,
    #[serde(default)]
    pub display_name: Option<String>,
    pub direction: EndpointDirection,
    /// 该端点支持的能力标签，如 "speaker"、"microphone" 等
    #[serde(default)]
    pub capabilities: Vec<String>,
    /// 该端点支持的音频格式
    #[serde(default)]
    pub supported_formats: Vec<AudioFormat>,
}

/// 设备聚合根，包含一个或多个音频端点
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Device {
    pub id: DeviceId,
    #[serde(default)]
    pub hostname: Option<String>,
    #[serde(default)]
    pub platform: Option<String>,
    pub endpoints: Vec<AudioEndpoint>,
}

impl Device {
    /// 从旧版 AgentDeviceIdentity 构造单端点 Device（向后兼容）
    ///
    /// 当 agent 没有携带完整的 device_info 时，用旧字段构造一个默认的全双工端点。
    pub fn from_legacy(device_id: &str, hostname: Option<&str>, platform: Option<&str>) -> Self {
        let default_endpoint = AudioEndpoint {
            id: EndpointId("default".to_string()),
            display_name: Some("Default Endpoint".to_string()),
            direction: EndpointDirection::Duplex,
            capabilities: vec!["speaker".to_string(), "microphone".to_string()],
            supported_formats: Vec::new(),
        };
        Self {
            id: DeviceId(device_id.to_string()),
            hostname: hostname.map(ToString::to_string),
            platform: platform.map(ToString::to_string),
            endpoints: vec![default_endpoint],
        }
    }

    /// 按端点 ID 查找端点
    pub fn endpoint(&self, endpoint_id: &EndpointId) -> Option<&AudioEndpoint> {
        self.endpoints.iter().find(|ep| ep.id == *endpoint_id)
    }

    /// 查找所有匹配指定方向的端点
    pub fn endpoints_by_direction(&self, direction: EndpointDirection) -> Vec<&AudioEndpoint> {
        self.endpoints
            .iter()
            .filter(|ep| ep.direction == direction)
            .collect()
    }

    /// 查找所有支持输出的端点（包括 Output 和 Duplex）
    pub fn output_endpoints(&self) -> Vec<&AudioEndpoint> {
        self.endpoints
            .iter()
            .filter(|ep| ep.direction.supports_output())
            .collect()
    }

    /// 查找所有支持输入的端点（包括 Input 和 Duplex）
    pub fn input_endpoints(&self) -> Vec<&AudioEndpoint> {
        self.endpoints
            .iter()
            .filter(|ep| ep.direction.supports_input())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_device_creates_duplex_endpoint() {
        let device = Device::from_legacy("test-device", Some("myhost"), Some("linux-x86_64"));
        assert_eq!(device.id.as_str(), "test-device");
        assert_eq!(device.hostname.as_deref(), Some("myhost"));
        assert_eq!(device.endpoints.len(), 1);
        assert_eq!(device.endpoints[0].direction, EndpointDirection::Duplex);
    }

    #[test]
    fn endpoint_direction_support_checks() {
        assert!(EndpointDirection::Input.supports_input());
        assert!(!EndpointDirection::Input.supports_output());
        assert!(!EndpointDirection::Output.supports_input());
        assert!(EndpointDirection::Output.supports_output());
        assert!(EndpointDirection::Duplex.supports_input());
        assert!(EndpointDirection::Duplex.supports_output());
    }

    #[test]
    fn output_endpoints_includes_duplex() {
        let device = Device {
            id: DeviceId("d1".to_string()),
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
                    id: EndpointId("speaker".to_string()),
                    display_name: None,
                    direction: EndpointDirection::Output,
                    capabilities: Vec::new(),
                    supported_formats: Vec::new(),
                },
                AudioEndpoint {
                    id: EndpointId("headset".to_string()),
                    display_name: None,
                    direction: EndpointDirection::Duplex,
                    capabilities: Vec::new(),
                    supported_formats: Vec::new(),
                },
            ],
        };
        let outputs = device.output_endpoints();
        assert_eq!(outputs.len(), 2);
        assert_eq!(outputs[0].id.as_str(), "speaker");
        assert_eq!(outputs[1].id.as_str(), "headset");
    }
}
