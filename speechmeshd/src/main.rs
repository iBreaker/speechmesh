use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Result, anyhow};
use clap::{Args as ClapArgs, Parser, Subcommand, ValueEnum};
use speechmeshd::agent::{AgentRegistry, RemoteAgentAsrBridge, RemoteAgentAsrBridgeConfig};
use speechmeshd::asr_bridge::{
    CompositeAsrBridge, MockAsrBridge, SharedAsrBridge, StdioAsrBridge, StdioAsrBridgeConfig,
    TcpAsrBridge, TcpAsrBridgeConfig,
};
use speechmeshd::providers::{
    InstallStateChange, InstalledAsrBridgeKind, InstalledAsrProvider, InstalledAsrProvidersConfig,
    bridge_mode_name, install_asr_provider, list_provider_statuses, load_asr_provider_catalog,
    load_asr_provider_config, load_asr_provider_state_or_default, set_asr_provider_enabled,
    uninstall_asr_provider,
};
use speechmeshd::server::{ServerConfig, run_server};
use speechmeshd::tts_bridge::{
    CompositeTtsBridge, MeloHttpTtsBridge, MeloHttpTtsBridgeConfig, MiniMaxHttpTtsBridge,
    MiniMaxHttpTtsBridgeConfig, MockTtsBridge, QwenHttpTtsBridge, QwenHttpTtsBridgeConfig,
    SharedTtsBridge,
};
use tracing_subscriber::EnvFilter;

#[derive(Debug, Clone, Copy, ValueEnum)]
enum AsrBridgeMode {
    Mock,
    Stdio,
    Tcp,
    Agent,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum TtsBridgeMode {
    Disabled,
    Mock,
    MeloHttp,
}

#[derive(Debug, Clone, Copy, ValueEnum, PartialEq, Eq)]
enum TtsProviderKind {
    Mock,
    MeloHttp,
    Qwen3Http,
    MiniMaxHttp,
}

#[derive(Debug, Parser)]
#[command(name = "speechmeshd")]
#[command(about = "SpeechMesh WebSocket daemon and provider installer")]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
    #[command(flatten)]
    run: RunArgs,
}

#[derive(Debug, Subcommand)]
enum Commands {
    Providers(ProvidersArgs),
}

#[derive(Debug, ClapArgs)]
struct ProvidersArgs {
    #[command(subcommand)]
    command: ProvidersCommand,
}

#[derive(Debug, Subcommand)]
enum ProvidersCommand {
    List(ListProvidersArgs),
    Install(InstallProviderArgs),
    Uninstall(ChangeProviderArgs),
    Enable(ChangeProviderArgs),
    Disable(ChangeProviderArgs),
}

#[derive(Debug, ClapArgs)]
struct ListProvidersArgs {
    #[arg(long)]
    catalog: Option<PathBuf>,
    #[arg(long)]
    state: Option<PathBuf>,
}

#[derive(Debug, ClapArgs)]
struct InstallProviderArgs {
    provider_id: String,
    #[arg(long)]
    catalog: PathBuf,
    #[arg(long)]
    state: PathBuf,
    #[arg(long, conflicts_with = "disable")]
    enable: bool,
    #[arg(long, conflicts_with = "enable")]
    disable: bool,
}

#[derive(Debug, ClapArgs)]
struct ChangeProviderArgs {
    provider_id: String,
    #[arg(long)]
    state: PathBuf,
}

#[derive(Debug, ClapArgs)]
struct RunArgs {
    #[arg(long, default_value = "127.0.0.1:8765")]
    listen: SocketAddr,
    #[arg(long, default_value = "v1")]
    protocol_version: String,
    #[arg(long, default_value = "speechmeshd")]
    server_name: String,
    #[arg(long)]
    asr_providers_config: Option<PathBuf>,
    #[arg(long)]
    asr_providers_state: Option<PathBuf>,
    #[arg(long, value_enum, default_value_t = AsrBridgeMode::Mock)]
    asr_bridge_mode: AsrBridgeMode,
    #[arg(long, default_value = "bridge.asr")]
    asr_provider_id: String,
    #[arg(long)]
    asr_bridge_command: Option<String>,
    #[arg(long)]
    asr_bridge_args: Vec<String>,
    #[arg(long)]
    asr_bridge_address: Option<String>,
    #[arg(long)]
    agent_shared_secret: Option<String>,
    #[arg(long, default_value_t = 10)]
    agent_start_timeout_secs: u64,
    #[arg(long, value_enum, default_value_t = TtsBridgeMode::Disabled)]
    tts_bridge_mode: TtsBridgeMode,
    #[arg(long = "tts-provider", value_enum)]
    tts_providers: Vec<TtsProviderKind>,
    #[arg(long, default_value = "tts.bridge")]
    tts_provider_id: String,
    #[arg(long, default_value = "SpeechMesh TTS")]
    tts_provider_name: String,
    #[arg(long)]
    tts_melo_base_url: Option<String>,
    #[arg(long)]
    tts_qwen3_base_url: Option<String>,
    #[arg(long, default_value = "qwen3.tts")]
    tts_qwen3_provider_id: String,
    #[arg(long, default_value = "Qwen3 TTS")]
    tts_qwen3_provider_name: String,
    #[arg(long)]
    tts_qwen3_model: Option<String>,
    #[arg(long)]
    tts_qwen3_voice: Option<String>,
    #[arg(long)]
    tts_qwen3_language: Option<String>,
    #[arg(long, default_value_t = 24000)]
    tts_qwen3_sample_rate_hz: u32,
    #[arg(long, default_value = "minimax.tts")]
    tts_minimax_provider_id: String,
    #[arg(long, default_value = "MiniMax Speech")]
    tts_minimax_provider_name: String,
    #[arg(long, default_value = "https://api.minimaxi.com")]
    tts_minimax_base_url: String,
    #[arg(long)]
    tts_minimax_api_key: Option<String>,
    #[arg(long)]
    tts_minimax_group_id: Option<String>,
    #[arg(long)]
    tts_minimax_api_key_file: Option<PathBuf>,
    #[arg(long, default_value = "speech-2.8-turbo")]
    tts_minimax_model: String,
    #[arg(long, default_value = "female-shaonv")]
    tts_minimax_voice_id: String,
    #[arg(long, default_value_t = 32000)]
    tts_minimax_sample_rate_hz: u32,
    #[arg(long, default_value = "wav")]
    tts_minimax_format: String,
    #[arg(long, default_value_t = 16384)]
    tts_chunk_size_bytes: usize,
}

#[tokio::main]
async fn main() -> Result<()> {
    init_tracing();
    let cli = Cli::parse();
    match cli.command {
        Some(Commands::Providers(command)) => execute_provider_command(command),
        None => run_gateway(cli.run).await,
    }
}

async fn run_gateway(args: RunArgs) -> Result<()> {
    let selection = select_asr_bridge(&args)?;
    let tts_bridge = select_tts_bridge(&args)?;
    let config = ServerConfig {
        listen: args.listen,
        protocol_version: args.protocol_version,
        server_name: args.server_name,
    };
    run_server(
        config,
        selection.bridge,
        tts_bridge,
        selection.agent_registry,
    )
    .await
}

fn init_tracing() {
    let env_filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("speechmeshd=info"));
    tracing_subscriber::fmt()
        .with_env_filter(env_filter)
        .compact()
        .init();
}

fn execute_provider_command(args: ProvidersArgs) -> Result<()> {
    match args.command {
        ProvidersCommand::List(command) => {
            let catalog = command
                .catalog
                .as_deref()
                .map(load_asr_provider_catalog)
                .transpose()?;
            let state = if let Some(path) = command.state.as_deref() {
                load_asr_provider_state_or_default(path)?
            } else {
                InstalledAsrProvidersConfig {
                    format: None,
                    providers: Vec::new(),
                }
            };

            if catalog.is_none() && command.state.is_none() {
                return Err(anyhow!(
                    "providers list requires --catalog, --state, or both"
                ));
            }

            for row in list_provider_statuses(catalog.as_ref(), &state) {
                let status = if row.installed {
                    if row.enabled {
                        "installed/enabled"
                    } else {
                        "installed/disabled"
                    }
                } else {
                    "available"
                };
                println!(
                    "{} | {} | bridge={} | download={}{}{}",
                    row.provider_id,
                    status,
                    row.bridge_mode,
                    if row.download_required {
                        "required"
                    } else {
                        "no"
                    },
                    row.display_name
                        .as_deref()
                        .map(|name| format!(" | name={name}"))
                        .unwrap_or_default(),
                    row.artifact_hint
                        .as_deref()
                        .map(|hint| format!(" | artifact={hint}"))
                        .unwrap_or_default()
                );
                if let Some(description) = row.description.as_deref() {
                    println!("  {description}");
                }
            }
            Ok(())
        }
        ProvidersCommand::Install(command) => {
            let enabled_override = if command.enable {
                Some(true)
            } else if command.disable {
                Some(false)
            } else {
                None
            };
            let (change, provider) = install_asr_provider(
                &command.catalog,
                &command.state,
                &command.provider_id,
                enabled_override,
            )?;
            let action = match change {
                InstallStateChange::Installed => "installed",
                InstallStateChange::Updated => "updated",
            };
            println!(
                "{} {} in {} | bridge={} | enabled={}",
                action,
                provider.provider_id,
                command.state.display(),
                bridge_mode_name(&provider.bridge),
                provider.enabled
            );
            if provider.install.download_required {
                println!("  download required before traffic should be routed");
            }
            for note in &provider.install.notes {
                println!("  note: {note}");
            }
            Ok(())
        }
        ProvidersCommand::Uninstall(command) => {
            let provider = uninstall_asr_provider(&command.state, &command.provider_id)?;
            println!(
                "removed {} from {}",
                provider.provider_id,
                command.state.display()
            );
            Ok(())
        }
        ProvidersCommand::Enable(command) => {
            let (changed, provider) =
                set_asr_provider_enabled(&command.state, &command.provider_id, true)?;
            let action = if changed {
                "enabled"
            } else {
                "already enabled"
            };
            println!(
                "{} {} in {}",
                action,
                provider.provider_id,
                command.state.display()
            );
            Ok(())
        }
        ProvidersCommand::Disable(command) => {
            let (changed, provider) =
                set_asr_provider_enabled(&command.state, &command.provider_id, false)?;
            let action = if changed {
                "disabled"
            } else {
                "already disabled"
            };
            println!(
                "{} {} in {}",
                action,
                provider.provider_id,
                command.state.display()
            );
            Ok(())
        }
    }
}

struct BridgeSelection {
    bridge: SharedAsrBridge,
    agent_registry: Option<AgentRegistry>,
}

fn select_tts_bridge(args: &RunArgs) -> Result<Option<SharedTtsBridge>> {
    let mut bridges: Vec<SharedTtsBridge> = Vec::new();
    let provider_kinds: Vec<TtsProviderKind> = if args.tts_providers.is_empty() {
        match args.tts_bridge_mode {
            TtsBridgeMode::Disabled => Vec::new(),
            TtsBridgeMode::Mock => vec![TtsProviderKind::Mock],
            TtsBridgeMode::MeloHttp => vec![TtsProviderKind::MeloHttp],
        }
    } else {
        args.tts_providers.clone()
    };

    for kind in provider_kinds {
        match kind {
            TtsProviderKind::Mock => bridges.push(Arc::new(MockTtsBridge::with_display_name(
                args.tts_provider_id.clone(),
                args.tts_provider_name.clone(),
            ))),
            TtsProviderKind::MeloHttp => {
                let base_url = args.tts_melo_base_url.clone().ok_or_else(|| {
                    anyhow!(
                        "--tts-melo-base-url is required when enabling the melo-http TTS provider"
                    )
                })?;
                bridges.push(Arc::new(MeloHttpTtsBridge::new(MeloHttpTtsBridgeConfig {
                    provider_id: args.tts_provider_id.clone(),
                    display_name: Some(args.tts_provider_name.clone()),
                    base_url,
                    request_timeout: Duration::from_secs(120),
                    chunk_size_bytes: args.tts_chunk_size_bytes,
                })?));
            }
            TtsProviderKind::Qwen3Http => {
                let base_url = args.tts_qwen3_base_url.clone().ok_or_else(|| {
                    anyhow!(
                        "--tts-qwen3-base-url is required when enabling the qwen3-http TTS provider"
                    )
                })?;
                bridges.push(Arc::new(QwenHttpTtsBridge::new(QwenHttpTtsBridgeConfig {
                    provider_id: args.tts_qwen3_provider_id.clone(),
                    display_name: Some(args.tts_qwen3_provider_name.clone()),
                    base_url,
                    request_timeout: Duration::from_secs(300),
                    chunk_size_bytes: args.tts_chunk_size_bytes,
                    default_model: args.tts_qwen3_model.clone(),
                    default_voice: args.tts_qwen3_voice.clone(),
                    default_language: args.tts_qwen3_language.clone(),
                    default_sample_rate_hz: args.tts_qwen3_sample_rate_hz,
                })?));
            }
            TtsProviderKind::MiniMaxHttp => {
                let api_key = resolve_minimax_api_key(args)?;
                let group_id = resolve_minimax_group_id(args).ok_or_else(|| {
                    anyhow!("--tts-minimax-group-id or MINIMAX_GROUP_ID is required when enabling the minimax-http TTS provider")
                })?;
                bridges.push(Arc::new(MiniMaxHttpTtsBridge::new(
                    MiniMaxHttpTtsBridgeConfig {
                        provider_id: args.tts_minimax_provider_id.clone(),
                        display_name: Some(args.tts_minimax_provider_name.clone()),
                        base_url: args.tts_minimax_base_url.clone(),
                        api_key,
                        group_id,
                        default_model: args.tts_minimax_model.clone(),
                        default_voice_id: args.tts_minimax_voice_id.clone(),
                        default_sample_rate_hz: args.tts_minimax_sample_rate_hz,
                        default_format: args.tts_minimax_format.clone(),
                        request_timeout: Duration::from_secs(120),
                        chunk_size_bytes: args.tts_chunk_size_bytes,
                    },
                )?));
            }
        }
    }

    if bridges.is_empty() {
        return Ok(None);
    }

    Ok(Some(Arc::new(CompositeTtsBridge::new(bridges)?)))
}

fn resolve_minimax_api_key(args: &RunArgs) -> Result<String> {
    if let Some(value) = args.tts_minimax_api_key.as_deref() {
        let trimmed = value.trim();
        if !trimmed.is_empty() {
            return Ok(trimmed.to_string());
        }
    }

    if let Some(value) = std::env::var_os("MINIMAX_API_KEY") {
        let trimmed = value.to_string_lossy().trim().to_string();
        if !trimmed.is_empty() {
            return Ok(trimmed);
        }
    }

    if let Some(path) = args.tts_minimax_api_key_file.as_deref() {
        return read_secret_from_file(path);
    }

    let default_path = PathBuf::from("~/.minimax/token");
    let expanded = expand_tilde_path(&default_path);
    if expanded.exists() {
        return read_secret_from_file(&expanded);
    }

    Err(anyhow!(
        "--tts-minimax-api-key, --tts-minimax-api-key-file, or ~/.minimax/token is required when enabling the minimax-http TTS provider"
    ))
}

fn resolve_minimax_group_id(args: &RunArgs) -> Option<String> {
    args.tts_minimax_group_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .or_else(|| {
            std::env::var_os("MINIMAX_GROUP_ID").and_then(|value| {
                let trimmed = value.to_string_lossy().trim().to_string();
                (!trimmed.is_empty()).then_some(trimmed)
            })
        })
}

fn read_secret_from_file(path: &Path) -> Result<String> {
    let contents = std::fs::read_to_string(path)
        .map_err(|error| anyhow!("failed to read {}: {error}", path.display()))?;
    let trimmed = contents.trim();
    if trimmed.is_empty() {
        return Err(anyhow!("{} is empty", path.display()));
    }
    Ok(trimmed.to_string())
}

fn expand_tilde_path(path: &Path) -> PathBuf {
    let value = path.to_string_lossy();
    if value == "~" {
        return dirs_home_dir().unwrap_or_else(|| PathBuf::from("/"));
    }
    if let Some(suffix) = value.strip_prefix("~/") {
        if let Some(home) = dirs_home_dir() {
            return home.join(suffix);
        }
    }
    path.to_path_buf()
}

fn dirs_home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

fn select_asr_bridge(args: &RunArgs) -> Result<BridgeSelection> {
    if args.asr_providers_config.is_some() && args.asr_providers_state.is_some() {
        return Err(anyhow!(
            "--asr-providers-config and --asr-providers-state are mutually exclusive"
        ));
    }

    if let Some(path) = args.asr_providers_state.as_deref() {
        let parsed = load_asr_provider_config(path)?;
        return select_asr_bridges_from_config(args, path, parsed);
    }

    if let Some(path) = args.asr_providers_config.as_deref() {
        let parsed = load_asr_provider_config(path)?;
        return select_asr_bridges_from_config(args, path, parsed);
    }

    let mut agent_registry = None;
    let bridge: SharedAsrBridge = match args.asr_bridge_mode {
        AsrBridgeMode::Mock => Arc::new(MockAsrBridge::new(args.asr_provider_id.clone())),
        AsrBridgeMode::Stdio => {
            let command = args
                .asr_bridge_command
                .clone()
                .ok_or_else(|| anyhow!("--asr-bridge-command is required for stdio mode"))?;
            Arc::new(StdioAsrBridge::new(StdioAsrBridgeConfig {
                provider_id: args.asr_provider_id.clone(),
                display_name: None,
                command,
                args: args.asr_bridge_args.clone(),
            }))
        }
        AsrBridgeMode::Tcp => {
            let address = args
                .asr_bridge_address
                .clone()
                .ok_or_else(|| anyhow!("--asr-bridge-address is required for tcp mode"))?;
            Arc::new(TcpAsrBridge::new(TcpAsrBridgeConfig {
                provider_id: args.asr_provider_id.clone(),
                display_name: None,
                address,
            }))
        }
        AsrBridgeMode::Agent => {
            let registry = AgentRegistry::new(args.agent_shared_secret.clone());
            agent_registry = Some(registry.clone());
            Arc::new(RemoteAgentAsrBridge::new(
                RemoteAgentAsrBridgeConfig {
                    provider_id: args.asr_provider_id.clone(),
                    display_name: None,
                    start_timeout: Duration::from_secs(args.agent_start_timeout_secs),
                },
                registry,
            ))
        }
    };
    Ok(BridgeSelection {
        bridge,
        agent_registry,
    })
}

fn select_asr_bridges_from_config(
    args: &RunArgs,
    path: &Path,
    parsed: InstalledAsrProvidersConfig,
) -> Result<BridgeSelection> {
    let installed: Vec<_> = parsed
        .providers
        .into_iter()
        .filter(|provider| provider.enabled)
        .collect();
    if installed.is_empty() {
        return Err(anyhow!(
            "ASR provider file {} does not contain any enabled providers",
            path.display()
        ));
    }

    let needs_agent_registry = installed
        .iter()
        .any(|provider| matches!(provider.bridge, InstalledAsrBridgeKind::Agent { .. }));
    let agent_registry =
        needs_agent_registry.then(|| AgentRegistry::new(args.agent_shared_secret.clone()));

    let bridges = installed
        .into_iter()
        .map(|provider| build_installed_asr_bridge(provider, args, agent_registry.clone()))
        .collect::<Result<Vec<_>>>()?;

    let bridge = Arc::new(
        CompositeAsrBridge::new(bridges)
            .map_err(|error| anyhow!("failed to register ASR providers: {error}"))?,
    );

    Ok(BridgeSelection {
        bridge,
        agent_registry,
    })
}

fn build_installed_asr_bridge(
    provider: InstalledAsrProvider,
    args: &RunArgs,
    agent_registry: Option<AgentRegistry>,
) -> Result<SharedAsrBridge> {
    let bridge: SharedAsrBridge = match provider.bridge {
        InstalledAsrBridgeKind::Mock => {
            Arc::new(if let Some(display_name) = provider.display_name {
                MockAsrBridge::with_display_name(provider.provider_id, display_name)
            } else {
                MockAsrBridge::new(provider.provider_id)
            })
        }
        InstalledAsrBridgeKind::Stdio {
            command,
            args: bridge_args,
        } => Arc::new(StdioAsrBridge::new(StdioAsrBridgeConfig {
            provider_id: provider.provider_id,
            display_name: provider.display_name,
            command,
            args: bridge_args,
        })),
        InstalledAsrBridgeKind::Tcp { address } => {
            Arc::new(TcpAsrBridge::new(TcpAsrBridgeConfig {
                provider_id: provider.provider_id,
                display_name: provider.display_name,
                address,
            }))
        }
        InstalledAsrBridgeKind::Agent { start_timeout_secs } => {
            let registry = agent_registry.clone().ok_or_else(|| {
                anyhow!(
                    "provider {} uses bridge_mode=agent but no agent registry is available",
                    provider.provider_id
                )
            })?;
            Arc::new(RemoteAgentAsrBridge::new(
                RemoteAgentAsrBridgeConfig {
                    provider_id: provider.provider_id,
                    display_name: provider.display_name,
                    start_timeout: Duration::from_secs(
                        start_timeout_secs.unwrap_or(args.agent_start_timeout_secs),
                    ),
                },
                registry,
            ))
        }
    };
    Ok(bridge)
}
