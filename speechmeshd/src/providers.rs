use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, anyhow};
use serde::{Deserialize, Serialize};

const ASR_PROVIDER_CATALOG_FORMAT: &str = "speechmesh/asr-provider-catalog.v1";
const ASR_PROVIDER_STATE_FORMAT: &str = "speechmesh/asr-provider-state.v1";

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AsrProviderInstallMetadata {
    #[serde(default)]
    pub download_required: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifact_hint: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
}

impl AsrProviderInstallMetadata {
    pub fn is_empty(&self) -> bool {
        !self.download_required && self.artifact_hint.is_none() && self.notes.is_empty()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstalledAsrProvidersConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
    pub providers: Vec<InstalledAsrProvider>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstalledAsrProvider {
    pub provider_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub installed_at_unix_secs: Option<u64>,
    #[serde(default, skip_serializing_if = "AsrProviderInstallMetadata::is_empty")]
    pub install: AsrProviderInstallMetadata,
    #[serde(flatten)]
    pub bridge: InstalledAsrBridgeKind,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "bridge_mode", rename_all = "snake_case")]
pub enum InstalledAsrBridgeKind {
    Mock,
    Stdio {
        command: String,
        #[serde(default)]
        args: Vec<String>,
    },
    Tcp {
        address: String,
    },
    Agent {
        start_timeout_secs: Option<u64>,
    },
    MiniMaxHttp {
        #[serde(default = "default_minimax_base_url")]
        base_url: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        api_key_file: Option<String>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AsrProviderCatalog {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
    pub providers: Vec<CatalogAsrProvider>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CatalogAsrProvider {
    pub provider_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default = "default_true")]
    pub enabled_by_default: bool,
    #[serde(default, skip_serializing_if = "AsrProviderInstallMetadata::is_empty")]
    pub install: AsrProviderInstallMetadata,
    #[serde(flatten)]
    pub bridge: InstalledAsrBridgeKind,
}

impl CatalogAsrProvider {
    fn to_installed(&self, enabled_override: Option<bool>) -> InstalledAsrProvider {
        InstalledAsrProvider {
            provider_id: self.provider_id.clone(),
            display_name: self.display_name.clone(),
            description: self.description.clone(),
            enabled: enabled_override.unwrap_or(self.enabled_by_default),
            installed_at_unix_secs: Some(now_unix_secs()),
            install: self.install.clone(),
            bridge: self.bridge.clone(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallStateChange {
    Installed,
    Updated,
}

#[derive(Debug, Clone)]
pub struct ProviderInstallStatus {
    pub provider_id: String,
    pub display_name: Option<String>,
    pub description: Option<String>,
    pub installed: bool,
    pub enabled: bool,
    pub bridge_mode: &'static str,
    pub download_required: bool,
    pub artifact_hint: Option<String>,
}

fn default_true() -> bool {
    true
}

fn default_minimax_base_url() -> String {
    "https://api.minimaxi.com/v1".to_string()
}

fn now_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
fn now_unix_nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
}

fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create directory {}", parent.display()))?;
    }
    let encoded =
        serde_json::to_string_pretty(value).context("failed to serialize provider JSON")?;
    fs::write(path, format!("{encoded}\n"))
        .with_context(|| format!("failed to write {}", path.display()))?;
    Ok(())
}

pub fn load_asr_provider_catalog(path: &Path) -> Result<AsrProviderCatalog> {
    let raw = fs::read_to_string(path)
        .with_context(|| format!("failed to read ASR provider catalog {}", path.display()))?;
    let catalog: AsrProviderCatalog = serde_json::from_str(&raw)
        .with_context(|| format!("failed to parse ASR provider catalog {}", path.display()))?;
    validate_format(path, catalog.format.as_deref(), ASR_PROVIDER_CATALOG_FORMAT)?;
    Ok(catalog)
}

pub fn load_asr_provider_config(path: &Path) -> Result<InstalledAsrProvidersConfig> {
    let raw = fs::read_to_string(path)
        .with_context(|| format!("failed to read ASR provider config {}", path.display()))?;
    let config: InstalledAsrProvidersConfig = serde_json::from_str(&raw)
        .with_context(|| format!("failed to parse ASR provider config {}", path.display()))?;
    validate_format(path, config.format.as_deref(), ASR_PROVIDER_STATE_FORMAT)?;
    Ok(config)
}

pub fn load_asr_provider_state_or_default(path: &Path) -> Result<InstalledAsrProvidersConfig> {
    match fs::read_to_string(path) {
        Ok(raw) => {
            let state: InstalledAsrProvidersConfig =
                serde_json::from_str(&raw).with_context(|| {
                    format!("failed to parse ASR provider state {}", path.display())
                })?;
            validate_format(path, state.format.as_deref(), ASR_PROVIDER_STATE_FORMAT)?;
            Ok(state)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Ok(InstalledAsrProvidersConfig {
                format: Some(ASR_PROVIDER_STATE_FORMAT.to_string()),
                providers: Vec::new(),
            })
        }
        Err(error) => Err(error)
            .with_context(|| format!("failed to read ASR provider state {}", path.display())),
    }
}

pub fn save_asr_provider_state(path: &Path, mut state: InstalledAsrProvidersConfig) -> Result<()> {
    state.format = Some(ASR_PROVIDER_STATE_FORMAT.to_string());
    state.providers.sort_by(|left, right| {
        left.provider_id
            .cmp(&right.provider_id)
            .then_with(|| left.display_name.cmp(&right.display_name))
    });
    write_json(path, &state)
}

pub fn install_asr_provider(
    catalog_path: &Path,
    state_path: &Path,
    provider_id: &str,
    enabled_override: Option<bool>,
) -> Result<(InstallStateChange, InstalledAsrProvider)> {
    let catalog = load_asr_provider_catalog(catalog_path)?;
    let template = catalog
        .providers
        .into_iter()
        .find(|provider| provider.provider_id == provider_id)
        .ok_or_else(|| {
            anyhow!(
                "provider {provider_id} is not present in {}",
                catalog_path.display()
            )
        })?;

    let mut state = load_asr_provider_state_or_default(state_path)?;
    if let Some(existing) = state
        .providers
        .iter_mut()
        .find(|provider| provider.provider_id == provider_id)
    {
        let installed_at = existing
            .installed_at_unix_secs
            .or_else(|| Some(now_unix_secs()));
        *existing = template.to_installed(enabled_override.or(Some(existing.enabled)));
        existing.installed_at_unix_secs = installed_at;
        let provider = existing.clone();
        save_asr_provider_state(state_path, state)?;
        return Ok((InstallStateChange::Updated, provider));
    }

    let provider = template.to_installed(enabled_override);
    state.providers.push(provider.clone());
    save_asr_provider_state(state_path, state)?;
    Ok((InstallStateChange::Installed, provider))
}

pub fn uninstall_asr_provider(
    state_path: &Path,
    provider_id: &str,
) -> Result<InstalledAsrProvider> {
    let mut state = load_asr_provider_state_or_default(state_path)?;
    let index = state
        .providers
        .iter()
        .position(|provider| provider.provider_id == provider_id)
        .ok_or_else(|| {
            anyhow!(
                "provider {provider_id} is not installed in {}",
                state_path.display()
            )
        })?;
    let removed = state.providers.remove(index);
    save_asr_provider_state(state_path, state)?;
    Ok(removed)
}

pub fn set_asr_provider_enabled(
    state_path: &Path,
    provider_id: &str,
    enabled: bool,
) -> Result<(bool, InstalledAsrProvider)> {
    let mut state = load_asr_provider_state_or_default(state_path)?;
    let provider = state
        .providers
        .iter_mut()
        .find(|provider| provider.provider_id == provider_id)
        .ok_or_else(|| {
            anyhow!(
                "provider {provider_id} is not installed in {}",
                state_path.display()
            )
        })?;
    let changed = provider.enabled != enabled;
    provider.enabled = enabled;
    let updated = provider.clone();
    save_asr_provider_state(state_path, state)?;
    Ok((changed, updated))
}

pub fn list_provider_statuses(
    catalog: Option<&AsrProviderCatalog>,
    state: &InstalledAsrProvidersConfig,
) -> Vec<ProviderInstallStatus> {
    let installed = state
        .providers
        .iter()
        .map(|provider| (provider.provider_id.clone(), provider))
        .collect::<HashMap<_, _>>();

    let mut rows = Vec::new();
    if let Some(catalog) = catalog {
        for provider in &catalog.providers {
            let installed_provider = installed.get(&provider.provider_id);
            rows.push(ProviderInstallStatus {
                provider_id: provider.provider_id.clone(),
                display_name: installed_provider
                    .and_then(|provider| provider.display_name.clone())
                    .or_else(|| provider.display_name.clone()),
                description: installed_provider
                    .and_then(|provider| provider.description.clone())
                    .or_else(|| provider.description.clone()),
                installed: installed_provider.is_some(),
                enabled: installed_provider.is_some_and(|provider| provider.enabled),
                bridge_mode: bridge_mode_name(&provider.bridge),
                download_required: installed_provider
                    .map(|provider| provider.install.download_required)
                    .unwrap_or(provider.install.download_required),
                artifact_hint: installed_provider
                    .and_then(|provider| provider.install.artifact_hint.clone())
                    .or_else(|| provider.install.artifact_hint.clone()),
            });
        }
    } else {
        for provider in &state.providers {
            rows.push(ProviderInstallStatus {
                provider_id: provider.provider_id.clone(),
                display_name: provider.display_name.clone(),
                description: provider.description.clone(),
                installed: true,
                enabled: provider.enabled,
                bridge_mode: bridge_mode_name(&provider.bridge),
                download_required: provider.install.download_required,
                artifact_hint: provider.install.artifact_hint.clone(),
            });
        }
    }

    for provider in &state.providers {
        if rows
            .iter()
            .any(|row| row.provider_id == provider.provider_id)
        {
            continue;
        }
        rows.push(ProviderInstallStatus {
            provider_id: provider.provider_id.clone(),
            display_name: provider.display_name.clone(),
            description: provider.description.clone(),
            installed: true,
            enabled: provider.enabled,
            bridge_mode: bridge_mode_name(&provider.bridge),
            download_required: provider.install.download_required,
            artifact_hint: provider.install.artifact_hint.clone(),
        });
    }

    rows.sort_by(|left, right| left.provider_id.cmp(&right.provider_id));
    rows
}

pub fn bridge_mode_name(bridge: &InstalledAsrBridgeKind) -> &'static str {
    match bridge {
        InstalledAsrBridgeKind::Mock => "mock",
        InstalledAsrBridgeKind::Stdio { .. } => "stdio",
        InstalledAsrBridgeKind::Tcp { .. } => "tcp",
        InstalledAsrBridgeKind::Agent { .. } => "agent",
        InstalledAsrBridgeKind::MiniMaxHttp { .. } => "minimax_http",
    }
}

fn validate_format(path: &Path, actual: Option<&str>, expected: &str) -> Result<()> {
    match actual {
        Some(value) if value != expected => Err(anyhow!(
            "{} uses format {value}, expected {expected}",
            path.display()
        )),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(name: &str) -> std::path::PathBuf {
        let unique = format!(
            "{}-{}-{:?}",
            std::process::id(),
            now_unix_nanos(),
            std::thread::current().id()
        );
        std::env::temp_dir().join(format!("speechmesh-{name}-{unique}.json"))
    }

    fn write_catalog(path: &Path) {
        let catalog = AsrProviderCatalog {
            format: Some(ASR_PROVIDER_CATALOG_FORMAT.to_string()),
            providers: vec![
                CatalogAsrProvider {
                    provider_id: "apple.asr".to_string(),
                    display_name: Some("Apple Speech".to_string()),
                    description: Some("Apple Speech via macOS agent".to_string()),
                    enabled_by_default: true,
                    install: AsrProviderInstallMetadata {
                        download_required: false,
                        artifact_hint: None,
                        notes: vec!["Requires a connected apple_agent".to_string()],
                    },
                    bridge: InstalledAsrBridgeKind::Agent {
                        start_timeout_secs: Some(10),
                    },
                },
                CatalogAsrProvider {
                    provider_id: "sensevoice.asr".to_string(),
                    display_name: Some("SenseVoice".to_string()),
                    description: None,
                    enabled_by_default: false,
                    install: AsrProviderInstallMetadata {
                        download_required: true,
                        artifact_hint: Some("/srv/models/sensevoice".to_string()),
                        notes: Vec::new(),
                    },
                    bridge: InstalledAsrBridgeKind::Tcp {
                        address: "127.0.0.1:9901".to_string(),
                    },
                },
            ],
        };
        write_json(path, &catalog).expect("write catalog");
    }

    #[test]
    fn install_creates_state_with_install_metadata() {
        let catalog = temp_path("catalog");
        let state = temp_path("state");
        write_catalog(&catalog);

        let (change, provider) =
            install_asr_provider(&catalog, &state, "sensevoice.asr", Some(true))
                .expect("install provider");

        assert_eq!(change, InstallStateChange::Installed);
        assert_eq!(provider.provider_id, "sensevoice.asr");
        assert!(provider.enabled);
        assert!(provider.installed_at_unix_secs.is_some());
        assert!(provider.install.download_required);
        assert_eq!(
            provider.install.artifact_hint.as_deref(),
            Some("/srv/models/sensevoice")
        );

        let loaded = load_asr_provider_config(&state).expect("load state");
        assert_eq!(loaded.providers.len(), 1);

        let _ = fs::remove_file(catalog);
        let _ = fs::remove_file(state);
    }

    #[test]
    fn install_updates_existing_provider_without_losing_install_time() {
        let catalog = temp_path("catalog");
        let state = temp_path("state");
        write_catalog(&catalog);

        let (_, first) =
            install_asr_provider(&catalog, &state, "apple.asr", None).expect("initial install");
        let (change, second) = install_asr_provider(&catalog, &state, "apple.asr", Some(false))
            .expect("update install");

        assert_eq!(change, InstallStateChange::Updated);
        assert_eq!(first.installed_at_unix_secs, second.installed_at_unix_secs);
        assert!(!second.enabled);

        let _ = fs::remove_file(catalog);
        let _ = fs::remove_file(state);
    }

    #[test]
    fn enable_disable_and_uninstall_work_against_state() {
        let catalog = temp_path("catalog");
        let state = temp_path("state");
        write_catalog(&catalog);
        install_asr_provider(&catalog, &state, "apple.asr", None).expect("install provider");

        let (changed, provider) =
            set_asr_provider_enabled(&state, "apple.asr", false).expect("disable provider");
        assert!(changed);
        assert!(!provider.enabled);

        let removed = uninstall_asr_provider(&state, "apple.asr").expect("remove provider");
        assert_eq!(removed.provider_id, "apple.asr");

        let loaded = load_asr_provider_state_or_default(&state).expect("load final state");
        assert!(loaded.providers.is_empty());

        let _ = fs::remove_file(catalog);
        let _ = fs::remove_file(state);
    }
}
