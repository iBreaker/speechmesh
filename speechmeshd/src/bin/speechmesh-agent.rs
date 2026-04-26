use std::ffi::OsString;
use std::time::Duration;
use std::{collections::HashMap, env, fs, path::PathBuf};

use anyhow::{Context, Result, anyhow};
use clap::{Parser, Subcommand, ValueEnum};
use serde::Deserialize;
use tracing_subscriber::EnvFilter;

#[path = "speechmesh_agent/runtime.rs"]
mod runtime;

const CLI_AFTER_HELP: &str = "\
Examples:
  speechmesh agent run --agent-id mac03-agent --device-id mac03
  speechmesh agent --config ~/.speechmesh/config.yml run --agent-id mac03-agent --device-id mac03
  speechmesh agent run --gateway-url ws://127.0.0.1:8765/agent --shared-secret secret

Notes:
  - This process is a lightweight, long-running device agent.
  - It only connects to the SpeechMesh server /agent endpoint.
  - play_audio tasks are executed by a local player process (ffplay by default).
  - Override player command with SPEECHMESH_PLAYBACK_CMD when needed (for example on Android/Termux).
  - Set SPEECHMESH_PAD_PLAYER_CMD to force the iPad-only native player binary.
  - Playback uses the operating system current default output device.";

const RUN_AFTER_HELP: &str = "\
Examples:
  speechmesh agent run --gateway-url ws://127.0.0.1:8765/agent --agent-id mac01-agent --device-id mac01
  speechmesh agent run --shared-secret secret --capability speaker

Behavior:
  - This command is long-running and automatically reconnects on disconnect.
  - play_audio streams chunks to a local player process (ffplay by default, mpv fallback).
  - On iPad/iOS set SPEECHMESH_PAD_PLAYER_CMD to the dedicated player binary, e.g. 'speechmesh-pad-speaker'.
  - Set SPEECHMESH_PLAYBACK_CMD to force a specific command, e.g. 'mpv --no-video --really-quiet -'.
  - Local playback follows system default output (no fixed headset/speaker binding).";

#[derive(Debug, Parser)]
#[command(name = "speechmesh agent")]
#[command(about = "SpeechMesh generic device-side agent")]
#[command(after_help = CLI_AFTER_HELP)]
struct Cli {
    #[arg(
        long,
        global = true,
        value_name = "PATH",
        help = "Config file path; defaults to ~/.speechmesh/config.yml when present"
    )]
    config: Option<PathBuf>,
    #[arg(
        long,
        global = true,
        help = "Profile name inside the config file; falls back to active_profile or default"
    )]
    profile: Option<String>,
    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    #[command(about = "Run the long-lived device agent loop")]
    Run(RunArgs),
}

#[derive(Debug, Clone, Parser)]
#[command(after_help = RUN_AFTER_HELP)]
struct RunArgs {
    #[arg(
        long,
        help = "SpeechMesh /agent websocket URL; falls back to config file or ws://127.0.0.1:8765/agent"
    )]
    gateway_url: Option<String>,
    #[arg(
        long,
        default_value = "device-agent-1",
        help = "Stable unique id for this agent process"
    )]
    agent_id: String,
    #[arg(
        long,
        default_value = "SpeechMesh Device Agent",
        help = "Human-readable agent name"
    )]
    agent_name: String,
    #[arg(
        long,
        default_value = "local-device",
        help = "Stable machine identity (not a specific audio output device)"
    )]
    device_id: String,
    #[arg(
        long,
        default_value = "device.speaker",
        help = "Provider id for server-side routing compatibility"
    )]
    provider_id: String,
    #[arg(long, help = "Optional shared secret expected by the server")]
    shared_secret: Option<String>,
    #[arg(long, value_enum, default_value_t = AgentCapability::Speaker, help = "Capability profile exposed by this agent")]
    capability: AgentCapability,
    #[arg(
        long,
        default_value_t = 2,
        help = "Reconnect delay in seconds after disconnect/failure"
    )]
    reconnect_delay_secs: u64,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum AgentCapability {
    Speaker,
}

#[allow(dead_code)]
fn main() -> Result<()> {
    run_with_args(std::env::args_os())
}

pub fn run_with_args<I, T>(args: I) -> Result<()>
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    init_rustls();
    init_tracing();

    let cli = Cli::parse_from(args);
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("failed to initialize tokio runtime")?;
    runtime.block_on(async move {
        let config = load_agent_config(cli.config.as_deref())?;
        let profile = resolve_profile(config.as_ref(), cli.profile.as_deref());
        match cli.command {
            Commands::Run(args) => {
                let config = runtime::AgentRuntimeConfig {
                    gateway_url: args
                        .gateway_url
                        .or_else(|| profile.and_then(|p| p.gateway.agent_url.clone()))
                        .or_else(|| profile.and_then(|p| p.gateway.ws_url.clone()))
                        .unwrap_or_else(|| "ws://127.0.0.1:8765/agent".to_string()),
                    agent_id: args.agent_id,
                    agent_name: args.agent_name,
                    device_id: args.device_id,
                    provider_id: args.provider_id,
                    shared_secret: args
                        .shared_secret
                        .or_else(|| profile.and_then(|p| p.shared_secret.clone())),
                    capabilities: capabilities_for(args.capability),
                    reconnect_delay: Duration::from_secs(args.reconnect_delay_secs),
                };
                runtime::run_forever(config).await
            }
        }
    })
}

fn capabilities_for(capability: AgentCapability) -> Vec<String> {
    match capability {
        AgentCapability::Speaker => vec!["speaker".to_string()],
    }
}

fn init_tracing() {
    let env_filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("speechmeshd::agent=info,speechmesh_agent=info"));
    tracing_subscriber::fmt()
        .with_env_filter(env_filter)
        .compact()
        .init();
}

fn init_rustls() {
    let _ = rustls::crypto::ring::default_provider().install_default();
}

#[derive(Debug, Clone, Default, Deserialize)]
struct FileConfig {
    active_profile: Option<String>,
    #[serde(default)]
    profiles: HashMap<String, ProfileConfig>,
    #[serde(flatten)]
    default_profile: ProfileConfig,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct ProfileConfig {
    #[serde(default)]
    gateway: GatewayConfig,
    shared_secret: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct GatewayConfig {
    ws_url: Option<String>,
    agent_url: Option<String>,
}

fn load_agent_config(explicit_path: Option<&std::path::Path>) -> Result<Option<FileConfig>> {
    let path = config_path(explicit_path)?;
    let Some(path) = path else {
        return Ok(None);
    };
    if !path.exists() {
        return Ok(None);
    }
    let raw = fs::read_to_string(&path)
        .with_context(|| format!("failed to read config file {}", path.display()))?;
    let config = parse_file_config(&raw)
        .with_context(|| format!("failed to parse config {}", path.display()))?;
    Ok(Some(config))
}

fn resolve_profile<'a>(
    config: Option<&'a FileConfig>,
    selected: Option<&str>,
) -> Option<&'a ProfileConfig> {
    let config = config?;
    if let Some(name) = selected {
        if let Some(profile) = config.profiles.get(name) {
            return Some(profile);
        }
    }
    if let Some(name) = config.active_profile.as_deref() {
        if let Some(profile) = config.profiles.get(name) {
            return Some(profile);
        }
    }
    if let Some(profile) = config.profiles.get("default") {
        return Some(profile);
    }
    if config.default_profile.gateway.ws_url.is_some()
        || config.default_profile.gateway.agent_url.is_some()
        || config.default_profile.shared_secret.is_some()
    {
        return Some(&config.default_profile);
    }
    None
}

fn config_path(explicit_path: Option<&std::path::Path>) -> Result<Option<PathBuf>> {
    if let Some(path) = explicit_path {
        return Ok(Some(path.to_path_buf()));
    }
    let home = env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| anyhow!("HOME is not set; use --config to specify a config file"))?;
    Ok(Some(home.join(".speechmesh").join("config.yml")))
}

fn parse_file_config(raw: &str) -> Result<FileConfig> {
    let mut config = FileConfig::default();
    let mut current_profile: Option<String> = None;
    let mut in_profiles = false;
    let mut in_gateway = false;

    for original in raw.lines() {
        let line = original.trim_end();
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }

        let indent = line.chars().take_while(|c| *c == ' ').count();
        match indent {
            0 => {
                current_profile = None;
                in_gateway = false;
                if trimmed == "profiles:" {
                    in_profiles = true;
                    continue;
                }
                in_profiles = false;
                if trimmed == "gateway:" {
                    in_gateway = true;
                    continue;
                }
                assign_profile_value(&mut config.default_profile, trimmed)?;
                if let Some((key, value)) = split_key_value(trimmed) {
                    if key == "active_profile" {
                        config.active_profile = Some(value.to_string());
                    }
                }
            }
            2 if in_profiles && trimmed.ends_with(':') => {
                let name = trimmed.trim_end_matches(':').trim().to_string();
                current_profile = Some(name.clone());
                config.profiles.entry(name).or_default();
                in_gateway = false;
            }
            2 if !in_profiles && in_gateway => {
                assign_gateway_value(&mut config.default_profile.gateway, trimmed)?;
            }
            4 if current_profile.is_some() && trimmed == "gateway:" => {
                in_gateway = true;
            }
            4 if current_profile.is_some() => {
                in_gateway = false;
                let profile = config
                    .profiles
                    .get_mut(current_profile.as_deref().unwrap_or_default())
                    .ok_or_else(|| anyhow!("unknown profile while parsing"))?;
                assign_profile_value(profile, trimmed)?;
            }
            6 if current_profile.is_some() && in_gateway => {
                let profile = config
                    .profiles
                    .get_mut(current_profile.as_deref().unwrap_or_default())
                    .ok_or_else(|| anyhow!("unknown profile while parsing"))?;
                assign_gateway_value(&mut profile.gateway, trimmed)?;
            }
            _ => {}
        }
    }

    Ok(config)
}

fn assign_profile_value(profile: &mut ProfileConfig, trimmed: &str) -> Result<()> {
    if trimmed == "gateway:" {
        return Ok(());
    }
    let Some((key, value)) = split_key_value(trimmed) else {
        return Ok(());
    };
    match key {
        "shared_secret" => profile.shared_secret = Some(value.to_string()),
        "ws_url" => profile.gateway.ws_url = Some(value.to_string()),
        "agent_url" => profile.gateway.agent_url = Some(value.to_string()),
        _ => {}
    }
    Ok(())
}

fn assign_gateway_value(gateway: &mut GatewayConfig, trimmed: &str) -> Result<()> {
    let Some((key, value)) = split_key_value(trimmed) else {
        return Ok(());
    };
    match key {
        "ws_url" => gateway.ws_url = Some(value.to_string()),
        "agent_url" => gateway.agent_url = Some(value.to_string()),
        _ => {}
    }
    Ok(())
}

fn split_key_value(line: &str) -> Option<(&str, &str)> {
    let (key, value) = line.split_once(':')?;
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    Some((key.trim(), strip_quotes(value)))
}

fn strip_quotes(value: &str) -> &str {
    value
        .strip_prefix('"')
        .and_then(|v| v.strip_suffix('"'))
        .or_else(|| value.strip_prefix('\'').and_then(|v| v.strip_suffix('\'')))
        .unwrap_or(value)
}
