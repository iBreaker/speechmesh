//! Agent 注册表
//!
//! 管理已连接 agent 的注册、注销和查询，同时维护会话与任务的路由表。
//! 注册表对 agent 命令类型和会话事件类型是泛型的，以避免依赖 speechmeshd 的协议类型。

use std::collections::HashMap;
use std::fmt::Debug;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use speechmesh_core::CapabilityDomain;
use speechmesh_transport::agent::AgentUpdateStatus;
use tokio::sync::Mutex;
use tracing::warn;

use crate::model::Device;

/// Agent 类型
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AgentKind {
    #[default]
    AsrProvider,
    Device,
}

/// 旧版 agent 设备身份信息（向后兼容）
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentDeviceIdentity {
    pub device_id: String,
    #[serde(default)]
    pub hostname: Option<String>,
    #[serde(default)]
    pub platform: Option<String>,
}

/// 已注册 agent 的内部记录
///
/// `Cmd` 类型参数表示向 agent 发送的命令类型（如 `GatewayToAgentMessage`）。
#[derive(Debug, Clone)]
pub struct RegisteredAgent<Cmd: Send + Clone + 'static> {
    pub agent_id: String,
    pub agent_name: String,
    pub provider_id: Option<String>,
    pub capabilities: Vec<String>,
    pub capability_domains: Vec<CapabilityDomain>,
    pub agent_kind: AgentKind,
    pub client_version: Option<String>,
    pub update_status: Option<AgentUpdateStatus>,
    /// 旧版设备身份（向后兼容字段）
    pub device: Option<AgentDeviceIdentity>,
    /// 多端点设备信息（新版）
    pub device_info: Option<Device>,
    /// 向 agent 发送命令的通道
    pub command_tx: tokio::sync::mpsc::Sender<Cmd>,
}

/// Agent 快照，用于查询接口返回（不含 command_tx）
#[derive(Debug, Clone, Serialize)]
pub struct AgentSnapshot {
    pub agent_id: String,
    pub agent_name: String,
    pub provider_id: Option<String>,
    pub capabilities: Vec<String>,
    pub capability_domains: Vec<CapabilityDomain>,
    pub agent_kind: AgentKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub update_status: Option<AgentUpdateStatus>,
    pub device: Option<AgentDeviceIdentity>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device_info: Option<Device>,
}

impl AgentSnapshot {
    pub fn from_agent<Cmd: Send + Clone>(agent: &RegisteredAgent<Cmd>) -> Self {
        Self {
            agent_id: agent.agent_id.clone(),
            agent_name: agent.agent_name.clone(),
            provider_id: agent.provider_id.clone(),
            capabilities: agent.capabilities.clone(),
            capability_domains: agent.capability_domains.clone(),
            agent_kind: agent.agent_kind,
            client_version: agent.client_version.clone(),
            update_status: agent.update_status.clone(),
            device: agent.device.clone(),
            device_info: agent.device_info.clone(),
        }
    }
}

/// Agent 查询过滤条件
#[derive(Debug, Clone, Default)]
pub struct AgentSnapshotFilter {
    pub agent_id: Option<String>,
    pub device_id: Option<String>,
}

impl AgentSnapshotFilter {
    pub fn matches<Cmd: Send + Clone>(&self, agent: &RegisteredAgent<Cmd>) -> bool {
        if let Some(agent_id) = &self.agent_id {
            if agent.agent_id != *agent_id {
                return false;
            }
        }
        if let Some(device_id) = &self.device_id {
            // 优先使用新版 device_info，回退到旧版 device
            let agent_device_id = agent
                .device_info
                .as_ref()
                .map(|info| info.id.as_str())
                .or_else(|| agent.device.as_ref().map(|d| d.device_id.as_str()));
            if agent_device_id != Some(device_id.as_str()) {
                return false;
            }
        }
        true
    }
}

/// 任务路由记录
pub struct TaskRoute {
    pub agent_id: String,
}

/// 注册表内部状态
pub struct AgentRegistryInner<Cmd: Send + Clone + 'static> {
    pub agents: HashMap<String, RegisteredAgent<Cmd>>,
    pub tasks: HashMap<String, TaskRoute>,
}

impl<Cmd: Send + Clone + 'static> Default for AgentRegistryInner<Cmd> {
    fn default() -> Self {
        Self {
            agents: HashMap::new(),
            tasks: HashMap::new(),
        }
    }
}

/// Agent 注册表
///
/// 管理已连接 agent 的生命周期和任务路由。
/// 会话路由由 speechmeshd 自行管理（因为会话类型依赖协议层具体类型）。
#[derive(Clone)]
pub struct AgentRegistry<Cmd: Send + Clone + 'static> {
    expected_shared_secret: Option<String>,
    pub inner: Arc<Mutex<AgentRegistryInner<Cmd>>>,
}

/// 从注册表中移除 agent 的返回值
pub struct AgentRemovalResult {
    pub orphaned_task_ids: Vec<String>,
}

/// 在持有锁的情况下移除 agent 并收集受影响的任务
fn remove_agent_tasks_locked<Cmd: Send + Clone>(
    inner: &mut AgentRegistryInner<Cmd>,
    agent_id: &str,
) -> AgentRemovalResult {
    inner.agents.remove(agent_id);

    let orphaned_task_ids: Vec<String> = inner
        .tasks
        .iter()
        .filter_map(|(task_id, route)| {
            if route.agent_id == agent_id {
                Some(task_id.clone())
            } else {
                None
            }
        })
        .collect();
    for task_id in &orphaned_task_ids {
        inner.tasks.remove(task_id);
    }

    AgentRemovalResult { orphaned_task_ids }
}

impl<Cmd: Send + Clone + 'static> AgentRegistry<Cmd> {
    pub fn new(expected_shared_secret: Option<String>) -> Self {
        Self {
            expected_shared_secret,
            inner: Arc::new(Mutex::new(AgentRegistryInner::default())),
        }
    }

    /// 获取预期的共享密钥
    pub fn expected_shared_secret(&self) -> Option<&str> {
        self.expected_shared_secret.as_deref()
    }

    /// 注册 agent，返回被替换的旧 agent 产生的孤儿任务
    pub async fn register_agent(
        &self,
        agent: RegisteredAgent<Cmd>,
    ) -> AgentRemovalResult {
        let mut inner = self.inner.lock().await;
        let result = if inner.agents.contains_key(&agent.agent_id) {
            warn!(
                "replacing existing agent registration for {}",
                agent.agent_id
            );
            remove_agent_tasks_locked(&mut inner, &agent.agent_id)
        } else {
            AgentRemovalResult {
                orphaned_task_ids: Vec::new(),
            }
        };
        inner.agents.insert(agent.agent_id.clone(), agent);
        result
    }

    /// 注销 agent，返回孤儿任务
    pub async fn unregister_agent(&self, agent_id: &str) -> AgentRemovalResult {
        let mut inner = self.inner.lock().await;
        remove_agent_tasks_locked(&mut inner, agent_id)
    }

    /// 按 provider_id 选择 agent
    pub async fn select_agent(&self, provider_id: &str) -> Option<RegisteredAgent<Cmd>> {
        let inner = self.inner.lock().await;
        inner
            .agents
            .values()
            .find(|agent| agent.provider_id.as_deref() == Some(provider_id))
            .cloned()
    }

    /// 选择具有扬声器能力的设备 agent
    pub async fn select_speaker_agent(
        &self,
        device_id: Option<&str>,
        agent_id: Option<&str>,
    ) -> Option<RegisteredAgent<Cmd>> {
        let inner = self.inner.lock().await;
        inner
            .agents
            .values()
            .find(|agent| {
                if agent.agent_kind != AgentKind::Device {
                    return false;
                }
                // 检查传统能力标签或 capability_domains
                let has_speaker_capability = agent
                    .capabilities
                    .iter()
                    .any(|capability| capability == "speaker")
                    || agent
                        .capability_domains
                        .iter()
                        .any(|domain| *domain == CapabilityDomain::Tts);
                if !has_speaker_capability {
                    return false;
                }
                if let Some(expected_agent_id) = agent_id {
                    if agent.agent_id != expected_agent_id {
                        return false;
                    }
                }
                if let Some(expected_device_id) = device_id {
                    // 优先检查新版 device_info，回退到旧版 device
                    let actual_device_id = agent
                        .device_info
                        .as_ref()
                        .map(|info| info.id.as_str())
                        .or_else(|| agent.device.as_ref().map(|d| d.device_id.as_str()));
                    if actual_device_id != Some(expected_device_id) {
                        return false;
                    }
                }
                true
            })
            .cloned()
    }

    /// 注册任务路由
    pub async fn register_task(&self, task_id: String, agent_id: String) -> Result<(), String> {
        let mut inner = self.inner.lock().await;
        if inner.tasks.contains_key(&task_id) {
            return Err(format!("task_id {task_id} is already active"));
        }
        inner.tasks.insert(task_id, TaskRoute { agent_id });
        Ok(())
    }

    /// 移除任务路由
    pub async fn remove_task(&self, task_id: &str) -> Option<TaskRoute> {
        let mut inner = self.inner.lock().await;
        inner.tasks.remove(task_id)
    }

    /// 查询任务对应的 agent_id
    pub async fn task_agent_id(&self, task_id: &str) -> Option<String> {
        let inner = self.inner.lock().await;
        inner.tasks.get(task_id).map(|route| route.agent_id.clone())
    }

    /// 获取 agent 的 command_tx
    pub async fn agent_command_tx(
        &self,
        agent_id: &str,
    ) -> Option<tokio::sync::mpsc::Sender<Cmd>> {
        let inner = self.inner.lock().await;
        inner
            .agents
            .get(agent_id)
            .map(|agent| agent.command_tx.clone())
    }

    /// 按 agent_id 获取完整的 RegisteredAgent（包含 command_tx）
    pub async fn get_registered_agent(&self, agent_id: &str) -> Option<RegisteredAgent<Cmd>> {
        let inner = self.inner.lock().await;
        inner.agents.get(agent_id).cloned()
    }

    /// 获取 agent 快照列表
    pub async fn snapshot(&self, filter: AgentSnapshotFilter) -> Vec<AgentSnapshot> {
        let inner = self.inner.lock().await;
        inner
            .agents
            .values()
            .filter(|agent| filter.matches(*agent))
            .map(AgentSnapshot::from_agent)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_test_agent(
        agent_id: &str,
        kind: AgentKind,
        device_id: Option<&str>,
    ) -> RegisteredAgent<String> {
        let (tx, _rx) = tokio::sync::mpsc::channel(1);
        RegisteredAgent {
            agent_id: agent_id.to_string(),
            agent_name: format!("test-{agent_id}"),
            provider_id: Some(format!("provider-{agent_id}")),
            capabilities: vec!["speaker".to_string()],
            capability_domains: vec![CapabilityDomain::Tts],
            agent_kind: kind,
            client_version: None,
            update_status: None,
            device: device_id.map(|id| AgentDeviceIdentity {
                device_id: id.to_string(),
                hostname: None,
                platform: None,
            }),
            device_info: None,
            command_tx: tx,
        }
    }

    #[tokio::test]
    async fn register_and_select_agent() {
        let registry = AgentRegistry::<String>::new(None);
        let agent = make_test_agent("a1", AgentKind::AsrProvider, None);
        registry.register_agent(agent).await;
        let found = registry.select_agent("provider-a1").await;
        assert!(found.is_some());
        assert_eq!(found.unwrap().agent_id, "a1");
    }

    #[tokio::test]
    async fn select_speaker_agent_filters_by_kind() {
        let registry = AgentRegistry::<String>::new(None);
        let agent = make_test_agent("d1", AgentKind::Device, Some("device-1"));
        registry.register_agent(agent).await;

        let found = registry.select_speaker_agent(None, None).await;
        assert!(found.is_some());

        let found = registry
            .select_speaker_agent(Some("device-1"), None)
            .await;
        assert!(found.is_some());

        let found = registry
            .select_speaker_agent(Some("nonexistent"), None)
            .await;
        assert!(found.is_none());
    }

    #[tokio::test]
    async fn snapshot_filter_by_device_id() {
        let registry = AgentRegistry::<String>::new(None);
        let agent = make_test_agent("d1", AgentKind::Device, Some("device-1"));
        registry.register_agent(agent).await;

        let filter = AgentSnapshotFilter {
            device_id: Some("device-1".to_string()),
            ..Default::default()
        };
        let results = registry.snapshot(filter).await;
        assert_eq!(results.len(), 1);

        let filter = AgentSnapshotFilter {
            device_id: Some("other".to_string()),
            ..Default::default()
        };
        let results = registry.snapshot(filter).await;
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn unregister_returns_orphaned_tasks() {
        let registry = AgentRegistry::<String>::new(None);
        let agent = make_test_agent("a1", AgentKind::AsrProvider, None);
        registry.register_agent(agent).await;
        registry
            .register_task("t1".to_string(), "a1".to_string())
            .await
            .unwrap();

        let result = registry.unregister_agent("a1").await;
        assert_eq!(result.orphaned_task_ids, vec!["t1".to_string()]);
    }
}
