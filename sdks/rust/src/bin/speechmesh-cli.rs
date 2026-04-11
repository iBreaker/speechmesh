use std::ffi::OsString;
use std::io::{Read, Write};
use std::path::Path;
use std::path::PathBuf;
use std::process::Stdio;
use std::{collections::HashMap, env};
use std::{fs, io};

use anyhow::{anyhow, bail, Context, Result};
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use clap::{Args, Parser, Subcommand, ValueEnum};
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use speechmesh_sdk::{
    AudioEncoding, AudioFormat, CapabilityDomain, Client, ClientConfig, GatewayMessage,
    ProviderSelector, RecognitionOptions, StreamRequest, SynthesisInputKind, SynthesisOptions,
    TtsStreamRequest,
};
use speechmesh_transport::{
    AgentSnapshot, ClientMessage, ControlAgentStatusPayload, ControlAgentStatusResultPayload,
    ControlDevicesListPayload, ControlPlayAudioAcceptedPayload, ControlPlayAudioPayload,
    ControlRequest, ControlResponse, HelloRequest, ServerMessage,
};
use tokio::io::AsyncWriteExt;
use tokio::process::{ChildStdin, Command};
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::protocol::Message;

const CLI_AFTER_HELP: &str = "\
Examples:
  speechmesh say --device mac01 --text \"你好\"
  speechmesh --config ~/.speechmesh/config.yml say --device mac01 --text \"你好\"
  speechmesh discover providers --json
  speechmesh tts voices --provider minimax.tts --json
  speechmesh tts play --provider minimax.tts --text \"Hello from SpeechMesh\"
  speechmesh tts stream --provider minimax.tts --text \"Hello\" > out.mp3
  speechmesh asr transcribe --provider mock.asr --stdin < audio.pcm

Notes:
  - `say` sends text to the gateway for TTS, then routes playback to a target device agent.
  - `tts play` requires `ffplay` on the local machine.
  - `tts stream` writes raw audio bytes to stdout and logs status to stderr.
  - All commands connect to a SpeechMesh `/ws` endpoint via `--url`.
  - `doctor`, `devices`, and `agent status` use the gateway control plane to inspect health and registered agents.";

const DISCOVER_AFTER_HELP: &str = "\
Examples:
  speechmesh discover providers
  speechmesh discover providers --domain tts --json";

const DOCTOR_AFTER_HELP: &str = "\
Runs a lightweight end-to-end health check against the configured gateway.

Checks:
  gateway   Connect to `/ws` and complete the hello handshake.
  tts       Discover TTS providers through the gateway.
  asr       Discover ASR providers through the gateway.
  playback  Optional `/control` route test using a tiny silent WAV payload.

Examples:
  speechmesh doctor
  speechmesh doctor --skip-playback
  speechmesh doctor --device mac03 --json";

const DEVICES_AFTER_HELP: &str = "\
Lists currently registered agents known to the gateway control plane.

Examples:
  speechmesh devices
  speechmesh devices --json";

const AGENT_AFTER_HELP: &str = "\
Subcommands:
  status  Show one registered agent, or the agent currently attached to a device id.

Examples:
  speechmesh agent status --agent-id mac03-speaker-agent
  speechmesh agent status --device mac03 --json";

const TTS_AFTER_HELP: &str = "\
Subcommands:
  voices  List voices exposed by a TTS provider.
  stream  Emit audio bytes to stdout for piping into another tool.
  play    Stream audio directly to the local speaker through ffplay.

Examples:
  speechmesh tts voices --provider minimax.tts --json
  speechmesh tts play --provider minimax.tts --text \"你好\"
  speechmesh tts stream --provider minimax.tts --stdin > out.mp3";

const ASR_AFTER_HELP: &str = "\
Subcommands:
  transcribe  Send PCM or encoded audio and print the recognized text.

Examples:
  speechmesh asr transcribe --provider mock.asr --stdin < audio.pcm
  speechmesh asr transcribe --file sample.wav --encoding wav";

const TTS_VOICES_AFTER_HELP: &str = "\
Examples:
  speechmesh tts voices --provider minimax.tts
  speechmesh tts voices --provider minimax.tts --language zh-CN --json";

const TTS_STREAM_AFTER_HELP: &str = "\
Writes audio bytes to stdout.

Examples:
  speechmesh tts stream --provider minimax.tts --text \"Hello\" > out.mp3
  echo \"Hello\" | speechmesh tts stream --provider minimax.tts --stdin | ffplay -nodisp -autoexit -i pipe:0";

const TTS_PLAY_AFTER_HELP: &str = "\
Streams audio directly to the local speaker using ffplay.

Examples:
  speechmesh tts play --provider minimax.tts --text \"Hello from SpeechMesh\"
  cat line.txt | speechmesh tts play --provider minimax.tts --stdin

Requirement:
  - `ffplay` must be installed and available on PATH.

Voice selection:
  - `--voice-profile` selects a named voice profile from config.
  - When omitted, `speechmesh` can auto-select a voice profile by longest matching `project_voice_profiles.*.root` prefix of the current working directory.";

const SAY_AFTER_HELP: &str = "\
Synthesizes text through the gateway and routes playback to a target device agent.

Examples:
  speechmesh say --device mac01 --text \"你好，这里是 SpeechMesh\"
  speechmesh say --device mac01:airpod --text \"切到耳机播放\"
  speechmesh say --text \"如果配置了默认设备，这里可以省略 --device\"
  speechmesh say --device mac02 --provider minimax.tts --voice female-shaonv --text-file note.txt

Notes:
  - `say` connects to `--url` for `/ws` TTS, then derives a matching `/control` websocket.
  - Most options can be omitted when matching values are set under `profiles.<name>.defaults` in `~/.speechmesh/config.yml`.
  - `--voice-profile` overrides project-path voice auto mapping; explicit `--provider`, `--voice`, `--language`, `--rate`, `--pitch`, and `--volume` still take priority.
  - The target machine must have a connected `speechmesh agent run` process with `speaker` capability.";

const ASR_TRANSCRIBE_AFTER_HELP: &str = "\
Examples:
  speechmesh asr transcribe --provider mock.asr --stdin < audio.pcm
  speechmesh asr transcribe --file sample.wav --encoding wav

Input:
  - Use `--file` for a local audio file.
  - Use `--stdin` to read bytes from a pipe or redirected file.";

#[derive(Parser, Debug)]
#[command(name = "speechmesh")]
#[command(about = "Unified SpeechMesh client for TTS, ASR, and discovery")]
#[command(
    long_about = "Unified SpeechMesh client for discovery, text-to-speech, and speech-to-text over the stable `/ws` protocol."
)]
#[command(after_help = CLI_AFTER_HELP)]
struct Cli {
    #[arg(
        long,
        global = true,
        help = "SpeechMesh WebSocket endpoint; falls back to config file or ws://127.0.0.1:8765/ws"
    )]
    url: Option<String>,
    #[arg(
        long,
        global = true,
        help = "Client name sent during the hello handshake; falls back to config file or speechmesh"
    )]
    client_name: Option<String>,
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
    #[arg(
        long,
        global = true,
        help = "Print structured JSON output for one-shot commands"
    )]
    json: bool,
    #[arg(
        long,
        global = true,
        help = "Print streaming events as one JSON object per line"
    )]
    jsonl: bool,
    #[command(subcommand)]
    command: TopCommand,
}

#[derive(Subcommand, Debug)]
enum TopCommand {
    #[command(about = "Inspect providers exposed by a SpeechMesh gateway")]
    Discover(DiscoverCommand),
    #[command(about = "Run connectivity and routing diagnostics", after_help = DOCTOR_AFTER_HELP)]
    Doctor(DoctorArgs),
    #[command(about = "List registered agents and devices", after_help = DEVICES_AFTER_HELP)]
    Devices(DevicesArgs),
    #[command(about = "Inspect one agent registered with the gateway", after_help = AGENT_AFTER_HELP)]
    Agent(AgentCommand),
    #[command(about = "Synthesize text and play it on a target device", after_help = SAY_AFTER_HELP)]
    Say(SayArgs),
    #[command(about = "Text-to-speech commands")]
    Tts(TtsCommand),
    #[command(about = "Speech-to-text commands")]
    Asr(AsrCommand),
}

#[derive(Subcommand, Debug)]
enum DiscoverSubcommand {
    Providers(DiscoverProvidersArgs),
}

#[derive(Args, Debug)]
#[command(after_help = DISCOVER_AFTER_HELP)]
struct DiscoverCommand {
    #[command(subcommand)]
    command: DiscoverSubcommand,
}

#[derive(Args, Debug)]
struct DiscoverProvidersArgs {
    #[arg(long, value_enum, help = "Filter providers by capability domain")]
    domain: Option<DomainArg>,
}

#[derive(Args, Debug)]
struct DoctorArgs {
    #[arg(
        long,
        conflicts_with = "agent_id",
        help = "Run the playback route check against this device id"
    )]
    device: Option<String>,
    #[arg(
        long,
        conflicts_with = "device",
        help = "Run the playback route check against this agent id"
    )]
    agent_id: Option<String>,
    #[arg(long, help = "Skip the `/control` playback route probe")]
    skip_playback: bool,
}

#[derive(Args, Debug)]
struct DevicesArgs {}

#[derive(Subcommand, Debug)]
enum AgentSubcommand {
    #[command(about = "Show one agent by agent id or device id")]
    Status(AgentStatusArgs),
}

#[derive(Args, Debug)]
#[command(after_help = AGENT_AFTER_HELP)]
struct AgentCommand {
    #[command(subcommand)]
    command: AgentSubcommand,
}

#[derive(Args, Debug)]
struct AgentStatusArgs {
    #[arg(long, conflicts_with = "device", help = "Lookup by exact agent id")]
    agent_id: Option<String>,
    #[arg(
        long,
        conflicts_with = "agent_id",
        help = "Lookup the agent currently attached to this device id"
    )]
    device: Option<String>,
}

#[derive(Subcommand, Debug)]
enum TtsSubcommand {
    #[command(about = "List voices for a TTS provider")]
    Voices(TtsVoicesArgs),
    #[command(about = "Write TTS audio bytes to stdout", after_help = TTS_STREAM_AFTER_HELP)]
    Stream(TtsRunArgs),
    #[command(about = "Play TTS audio through the local speaker", after_help = TTS_PLAY_AFTER_HELP)]
    Play(TtsRunArgs),
}

#[derive(Args, Debug)]
#[command(after_help = TTS_AFTER_HELP)]
struct TtsCommand {
    #[command(subcommand)]
    command: TtsSubcommand,
}

#[derive(Args, Debug, Clone)]
struct ProviderArgs {
    #[arg(
        long,
        help = "Explicit provider id. If omitted, SpeechMesh auto-selects a provider"
    )]
    provider: Option<String>,
    #[arg(
        long = "require",
        help = "Capability that must be present on the chosen provider"
    )]
    required_capabilities: Vec<String>,
    #[arg(
        long = "prefer",
        help = "Capability preferred during provider auto-selection"
    )]
    preferred_capabilities: Vec<String>,
}

#[derive(Args, Debug)]
#[command(after_help = TTS_VOICES_AFTER_HELP)]
struct TtsVoicesArgs {
    #[command(flatten)]
    provider: ProviderArgs,
    #[arg(long, help = "Optional language filter passed to the provider")]
    language: Option<String>,
}

#[derive(Args, Debug)]
struct TtsRunArgs {
    #[command(flatten)]
    provider: ProviderArgs,
    #[command(flatten)]
    input: TextInputArgs,
    #[arg(
        long,
        help = "Named voice profile from config; overrides project-path auto mapping"
    )]
    voice_profile: Option<String>,
    #[arg(long, help = "Voice id to request from the provider")]
    voice: Option<String>,
    #[arg(long, help = "Language hint for the provider")]
    language: Option<String>,
    #[arg(long, help = "Speech rate multiplier, if supported by the provider")]
    rate: Option<f32>,
    #[arg(long, help = "Pitch adjustment, if supported by the provider")]
    pitch: Option<f32>,
    #[arg(long, help = "Volume adjustment, if supported by the provider")]
    volume: Option<f32>,
    #[arg(long, value_enum, help = "Requested output encoding")]
    format: Option<OutputEncodingArg>,
    #[arg(long, help = "Requested output sample rate")]
    sample_rate: Option<u32>,
    #[arg(long, help = "Requested output channel count")]
    channels: Option<u16>,
}

#[derive(Args, Debug)]
struct SayArgs {
    #[command(flatten)]
    tts: TtsRunArgs,
    #[arg(
        long,
        conflicts_with = "agent_id",
        help = "Target device id or host:output target (for example: mac01 or mac01:airpod); falls back to profiles.<name>.defaults.device"
    )]
    device: Option<String>,
    #[arg(long, conflicts_with = "device", help = "Target agent id")]
    agent_id: Option<String>,
    #[arg(
        long,
        help = "Optional explicit control websocket endpoint; falls back to config or `--url` with `/control`"
    )]
    control_url: Option<String>,
    #[arg(long, help = "Chunk size hint sent to the /control endpoint")]
    chunk_size_bytes: Option<usize>,
}

#[derive(Args, Debug)]
struct TextInputArgs {
    #[arg(long, conflicts_with_all = ["text_file", "stdin"], help = "Inline text to synthesize")]
    text: Option<String>,
    #[arg(long, value_name = "PATH", conflicts_with_all = ["text", "stdin"], help = "Read text input from a file")]
    text_file: Option<PathBuf>,
    #[arg(long, conflicts_with_all = ["text", "text_file"], help = "Read text input from stdin")]
    stdin: bool,
}

#[derive(Subcommand, Debug)]
enum AsrSubcommand {
    #[command(about = "Send audio to ASR and print the recognized text")]
    Transcribe(AsrTranscribeArgs),
}

#[derive(Args, Debug)]
#[command(after_help = ASR_AFTER_HELP)]
struct AsrCommand {
    #[command(subcommand)]
    command: AsrSubcommand,
}

#[derive(Args, Debug)]
#[command(after_help = ASR_TRANSCRIBE_AFTER_HELP)]
struct AsrTranscribeArgs {
    #[command(flatten)]
    provider: ProviderArgs,
    #[command(flatten)]
    input: AudioInputArgs,
    #[arg(long, help = "Language hint for recognition")]
    language: Option<String>,
    #[arg(long, help = "Print partial ASR results to stderr while decoding")]
    interim: bool,
    #[arg(
        long,
        conflicts_with = "no_punctuation",
        help = "Enable provider-side punctuation when supported"
    )]
    punctuation: bool,
    #[arg(
        long,
        conflicts_with = "punctuation",
        help = "Disable provider-side punctuation"
    )]
    no_punctuation: bool,
    #[arg(long, help = "Request token timestamps when supported")]
    timestamps: bool,
    #[arg(long = "hint", help = "Recognition hint; repeat for multiple hints")]
    hints: Vec<String>,
}

#[derive(Args, Debug)]
struct AudioInputArgs {
    #[arg(
        long,
        value_name = "PATH",
        conflicts_with = "stdin",
        help = "Read audio bytes from a local file"
    )]
    file: Option<PathBuf>,
    #[arg(long, conflicts_with = "file", help = "Read audio bytes from stdin")]
    stdin: bool,
    #[arg(long, value_enum, help = "Encoding of the supplied audio bytes")]
    encoding: Option<AudioEncodingArg>,
    #[arg(long, help = "Input sample rate in Hz")]
    sample_rate: Option<u32>,
    #[arg(long, help = "Input channel count")]
    channels: Option<u16>,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum DomainArg {
    Asr,
    Tts,
}

#[derive(Clone, Copy, Debug, Deserialize, ValueEnum)]
enum OutputEncodingArg {
    Mp3,
    Wav,
    Flac,
}

#[derive(Clone, Copy, Debug, Deserialize, ValueEnum)]
enum AudioEncodingArg {
    PcmS16Le,
    PcmF32Le,
    Opus,
    Mp3,
    Aac,
    Flac,
    Wav,
}

#[allow(dead_code)]
fn main() {
    if let Err(error) = run_with_args(std::env::args_os()) {
        eprintln!("error: {error:#}");
        std::process::exit(1);
    }
}

pub fn run_with_args<I, T>(args: I) -> Result<()>
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    let cli = Cli::parse_from(args);
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("failed to initialize tokio runtime")?;
    runtime.block_on(async move {
        let config = load_cli_config(cli.config.as_deref())?;
        let profile = resolve_profile(config.as_ref(), cli.profile.as_deref());
        let runtime = RuntimeConfig {
            url: cli
                .url
                .or_else(|| profile.and_then(|p| p.gateway.ws_url.clone()))
                .unwrap_or_else(|| "ws://127.0.0.1:8765/ws".to_string()),
            control_url: profile.and_then(|p| p.gateway.control_url.clone()),
            defaults: profile.map(|p| p.defaults.clone()).unwrap_or_default(),
            voice_profiles: config
                .as_ref()
                .map(|cfg| cfg.voice_profiles.clone())
                .unwrap_or_default(),
            project_voice_profiles: config
                .as_ref()
                .map(|cfg| cfg.project_voice_profiles.clone())
                .unwrap_or_default(),
            client_name: cli
                .client_name
                .or_else(|| profile.and_then(|p| p.client_name.clone()))
                .unwrap_or_else(|| "speechmesh".to_string()),
            json: cli.json,
            jsonl: cli.jsonl,
        };
        match cli.command {
            TopCommand::Discover(command) => run_discover(&runtime, command).await,
            TopCommand::Doctor(args) => run_doctor(&runtime, args).await,
            TopCommand::Devices(args) => run_devices(&runtime, args).await,
            TopCommand::Agent(command) => run_agent(&runtime, command).await,
            TopCommand::Say(args) => run_say(&runtime, args).await,
            TopCommand::Tts(command) => run_tts(&runtime, command).await,
            TopCommand::Asr(command) => run_asr(&runtime, command).await,
        }
    })
}

#[derive(Clone, Debug)]
struct RuntimeConfig {
    url: String,
    control_url: Option<String>,
    defaults: DefaultsConfig,
    voice_profiles: HashMap<String, VoiceProfileConfig>,
    project_voice_profiles: HashMap<String, ProjectVoiceBinding>,
    client_name: String,
    json: bool,
    jsonl: bool,
}

#[derive(Debug, Serialize)]
struct DoctorReport {
    gateway_url: String,
    control_url: String,
    playback_target: DoctorPlaybackTarget,
    gateway: DoctorCheckResult<DoctorGatewayPayload>,
    tts: DoctorCheckResult<DoctorDiscoverPayload>,
    asr: DoctorCheckResult<DoctorDiscoverPayload>,
    playback: DoctorCheckResult<DoctorPlaybackPayload>,
}

#[derive(Debug, Serialize)]
struct DoctorPlaybackTarget {
    device: Option<String>,
    agent_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PlaybackTarget {
    device_id: String,
    output_target: Option<String>,
}

#[derive(Debug, Serialize)]
struct DoctorCheckResult<T> {
    ok: bool,
    skipped: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    detail: Option<T>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

impl<T> DoctorCheckResult<T> {
    fn ok(detail: T) -> Self {
        Self {
            ok: true,
            skipped: false,
            detail: Some(detail),
            error: None,
        }
    }

    fn failed(error: String) -> Self {
        Self {
            ok: false,
            skipped: false,
            detail: None,
            error: Some(error),
        }
    }

    fn skipped(reason: impl Into<String>) -> Self {
        Self {
            ok: true,
            skipped: true,
            detail: None,
            error: Some(reason.into()),
        }
    }
}

#[derive(Debug, Serialize)]
struct DoctorGatewayPayload {
    server_name: String,
    protocol_version: String,
    one_session_per_connection: bool,
}

#[derive(Debug, Serialize)]
struct DoctorDiscoverPayload {
    provider_count: usize,
    provider_ids: Vec<String>,
}

#[derive(Debug, Serialize)]
struct DoctorPlaybackPayload {
    routed_agent_id: String,
    task_id: String,
    chunk_count: u64,
    total_bytes: u64,
}

async fn run_discover(cli: &RuntimeConfig, command: DiscoverCommand) -> Result<()> {
    match command.command {
        DiscoverSubcommand::Providers(args) => {
            let mut client = connect_client(cli).await?;
            let domains = match args.domain {
                Some(DomainArg::Asr) => vec![CapabilityDomain::Asr],
                Some(DomainArg::Tts) => vec![CapabilityDomain::Tts],
                None => vec![CapabilityDomain::Asr, CapabilityDomain::Tts],
            };
            let result = client.discover(domains).await?;
            print_serialized(&result.providers)?;
            Ok(())
        }
    }
}

async fn run_doctor(cli: &RuntimeConfig, args: DoctorArgs) -> Result<()> {
    let gateway = match check_gateway(cli).await {
        Ok(payload) => DoctorCheckResult::ok(payload),
        Err(error) => DoctorCheckResult::failed(error.to_string()),
    };

    let tts = match check_discover(cli, CapabilityDomain::Tts).await {
        Ok(payload) => DoctorCheckResult::ok(payload),
        Err(error) => DoctorCheckResult::failed(error.to_string()),
    };

    let asr = match check_discover(cli, CapabilityDomain::Asr).await {
        Ok(payload) => DoctorCheckResult::ok(payload),
        Err(error) => DoctorCheckResult::failed(error.to_string()),
    };

    let playback_target = DoctorPlaybackTarget {
        device: args.device.or_else(|| cli.defaults.device.clone()),
        agent_id: args.agent_id,
    };
    let playback = if args.skip_playback {
        DoctorCheckResult::skipped("skipped by --skip-playback")
    } else if playback_target.device.is_none() && playback_target.agent_id.is_none() {
        DoctorCheckResult::skipped("no --device/--agent-id and no defaults.device configured")
    } else {
        match check_playback_route(cli, &playback_target).await {
            Ok(payload) => DoctorCheckResult::ok(payload),
            Err(error) => DoctorCheckResult::failed(error.to_string()),
        }
    };

    let report = DoctorReport {
        gateway_url: cli.url.clone(),
        control_url: cli
            .defaults
            .control_url
            .clone()
            .or_else(|| cli.control_url.clone())
            .unwrap_or_else(|| derive_control_url(&cli.url)),
        playback_target,
        gateway,
        tts,
        asr,
        playback,
    };

    if cli.json {
        print_serialized(&report)?;
    } else if cli.jsonl {
        print_jsonl(&report)?;
    } else {
        print_doctor_report(&report);
    }

    if !report.gateway.ok || !report.tts.ok || !report.asr.ok || !report.playback.ok {
        bail!("doctor found one or more failing checks");
    }

    Ok(())
}

async fn run_devices(cli: &RuntimeConfig, _args: DevicesArgs) -> Result<()> {
    let payload: ControlDevicesListPayload =
        request_control(cli, ControlRequest::DevicesList, |message| match message {
            ControlResponse::DevicesList { payload } => Some(payload),
            _ => None,
        })
        .await
        .map_err(|error| annotate_control_capability_error(error, "devices.list"))?;

    if cli.json {
        print_serialized(&payload.agents)?;
    } else if cli.jsonl {
        for agent in &payload.agents {
            print_jsonl(agent)?;
        }
    } else {
        print_devices(&payload.agents);
    }
    Ok(())
}

async fn run_agent(cli: &RuntimeConfig, command: AgentCommand) -> Result<()> {
    match command.command {
        AgentSubcommand::Status(args) => run_agent_status(cli, args).await,
    }
}

async fn run_agent_status(cli: &RuntimeConfig, args: AgentStatusArgs) -> Result<()> {
    let device_id = args.device.or_else(|| cli.defaults.device.clone());
    if args.agent_id.is_none() && device_id.is_none() {
        bail!("one of --agent-id or --device is required");
    }

    let payload: ControlAgentStatusResultPayload = request_control(
        cli,
        ControlRequest::AgentStatus {
            payload: ControlAgentStatusPayload {
                agent_id: args.agent_id,
                device_id,
            },
        },
        |message| match message {
            ControlResponse::AgentStatus { payload } => Some(payload),
            _ => None,
        },
    )
    .await
    .map_err(|error| annotate_control_capability_error(error, "agent.status"))?;

    let Some(agent) = payload.agent else {
        bail!("no agent matched the requested selector");
    };

    if cli.json {
        print_serialized(&agent)?;
    } else if cli.jsonl {
        print_jsonl(&agent)?;
    } else {
        print_agent(&agent);
    }
    Ok(())
}

async fn run_tts(cli: &RuntimeConfig, command: TtsCommand) -> Result<()> {
    match command.command {
        TtsSubcommand::Voices(args) => {
            let mut client = connect_client(cli).await?;
            let result = client
                .list_tts_voices(
                    provider_selector(merge_provider_args(cli, args.provider)),
                    args.language.or_else(|| cli.defaults.language.clone()),
                )
                .await?;
            print_serialized(&result.voices)?;
            Ok(())
        }
        TtsSubcommand::Stream(args) => run_tts_stream(cli, args, OutputSink::Stdout).await,
        TtsSubcommand::Play(args) => run_tts_play(cli, args).await,
    }
}

async fn check_gateway(cli: &RuntimeConfig) -> Result<DoctorGatewayPayload> {
    let (mut websocket, response) = connect_async(&cli.url)
        .await
        .with_context(|| format!("failed to connect to {}", cli.url))?;
    if response.status().as_u16() != 101 {
        bail!(
            "gateway websocket handshake failed with {}",
            response.status()
        );
    }

    let hello = ClientMessage::Hello {
        request_id: None,
        payload: HelloRequest {
            protocol_version: "v1".to_string(),
            client_name: Some(cli.client_name.clone()),
        },
    };
    websocket
        .send(Message::Text(serde_json::to_string(&hello)?.into()))
        .await
        .context("failed to send hello")?;

    while let Some(frame) = websocket.next().await {
        match frame.context("failed to read gateway frame")? {
            Message::Text(text) => {
                let message: ServerMessage =
                    serde_json::from_str(&text).context("failed to decode hello response")?;
                match message {
                    ServerMessage::HelloOk { payload, .. } => {
                        let _ = websocket.close(None).await;
                        return Ok(DoctorGatewayPayload {
                            server_name: payload.server_name,
                            protocol_version: payload.protocol_version,
                            one_session_per_connection: payload.one_session_per_connection,
                        });
                    }
                    ServerMessage::Error { payload, .. } => {
                        bail!(
                            "gateway hello failed: {} ({})",
                            payload.error.message,
                            payload.error.code
                        );
                    }
                    _ => {}
                }
            }
            Message::Ping(payload) => {
                websocket
                    .send(Message::Pong(payload))
                    .await
                    .context("failed to send pong")?;
            }
            Message::Close(_) => bail!("gateway closed before hello.ok"),
            Message::Binary(_) | Message::Pong(_) | Message::Frame(_) => {}
        }
    }

    bail!("gateway closed before hello.ok")
}

async fn check_discover(
    cli: &RuntimeConfig,
    domain: CapabilityDomain,
) -> Result<DoctorDiscoverPayload> {
    let mut client = connect_client(cli).await?;
    let result = client.discover(vec![domain]).await?;
    let provider_ids = result
        .providers
        .iter()
        .map(|provider| provider.id.clone())
        .collect::<Vec<_>>();
    Ok(DoctorDiscoverPayload {
        provider_count: provider_ids.len(),
        provider_ids,
    })
}

async fn check_playback_route(
    cli: &RuntimeConfig,
    target: &DoctorPlaybackTarget,
) -> Result<DoctorPlaybackPayload> {
    let control_url = cli
        .defaults
        .control_url
        .clone()
        .or_else(|| cli.control_url.clone())
        .unwrap_or_else(|| derive_control_url(&cli.url));
    let parsed_target = target
        .device
        .as_deref()
        .map(parse_playback_target)
        .transpose()?;
    let accepted = send_play_audio(
        &control_url,
        ControlPlayAudioPayload {
            task_id: Some("doctor-playback".to_string()),
            device_id: parsed_target
                .as_ref()
                .map(|target| target.device_id.clone()),
            output_target: parsed_target.and_then(|target| target.output_target),
            agent_id: target.agent_id.clone(),
            format: Some(AudioFormat {
                encoding: AudioEncoding::Wav,
                sample_rate_hz: 16000,
                channels: 1,
            }),
            audio_base64: BASE64.encode(silent_wav_16k_mono()),
            chunk_size_bytes: Some(512),
        },
    )
    .await?;

    Ok(DoctorPlaybackPayload {
        routed_agent_id: accepted.routed_agent_id,
        task_id: accepted.task_id,
        chunk_count: accepted.chunk_count,
        total_bytes: accepted.total_bytes,
    })
}

fn silent_wav_16k_mono() -> Vec<u8> {
    let sample_rate = 16_000u32;
    let channels = 1u16;
    let bits_per_sample = 16u16;
    let bytes_per_sample = (bits_per_sample / 8) as usize;
    let sample_count = (sample_rate / 20) as usize;
    let data_size = sample_count * channels as usize * bytes_per_sample;
    let byte_rate = sample_rate * channels as u32 * bytes_per_sample as u32;
    let block_align = channels * bytes_per_sample as u16;

    let mut wav = Vec::with_capacity(44 + data_size);
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + data_size as u32).to_le_bytes());
    wav.extend_from_slice(b"WAVE");
    wav.extend_from_slice(b"fmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&channels.to_le_bytes());
    wav.extend_from_slice(&sample_rate.to_le_bytes());
    wav.extend_from_slice(&byte_rate.to_le_bytes());
    wav.extend_from_slice(&block_align.to_le_bytes());
    wav.extend_from_slice(&bits_per_sample.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&(data_size as u32).to_le_bytes());
    wav.resize(44 + data_size, 0);
    wav
}

fn print_doctor_report(report: &DoctorReport) {
    println!("gateway: {}", format_check(&report.gateway));
    print_detail(
        "gateway detail",
        report
            .gateway
            .detail
            .as_ref()
            .map(|detail| {
                format!(
                    "{} protocol={} one_session_per_connection={}",
                    detail.server_name, detail.protocol_version, detail.one_session_per_connection
                )
            })
            .as_deref(),
    );

    println!("tts discover: {}", format_check(&report.tts));
    print_detail(
        "tts providers",
        report
            .tts
            .detail
            .as_ref()
            .map(|detail| {
                format!(
                    "{} -> {}",
                    detail.provider_count,
                    detail.provider_ids.join(", ")
                )
            })
            .as_deref(),
    );

    println!("asr discover: {}", format_check(&report.asr));
    print_detail(
        "asr providers",
        report
            .asr
            .detail
            .as_ref()
            .map(|detail| {
                format!(
                    "{} -> {}",
                    detail.provider_count,
                    detail.provider_ids.join(", ")
                )
            })
            .as_deref(),
    );

    println!("playback route: {}", format_check(&report.playback));
    print_detail(
        "playback target",
        Some(
            format!(
                "device={:?} agent_id={:?}",
                report.playback_target.device, report.playback_target.agent_id
            )
            .as_str(),
        ),
    );
    print_detail(
        "playback detail",
        report
            .playback
            .detail
            .as_ref()
            .map(|detail| {
                format!(
                    "task_id={} routed_agent_id={} chunks={} bytes={}",
                    detail.task_id, detail.routed_agent_id, detail.chunk_count, detail.total_bytes
                )
            })
            .as_deref(),
    );
}

fn print_devices(agents: &[AgentSnapshot]) {
    if agents.is_empty() {
        println!("no registered agents");
        return;
    }
    for agent in agents {
        println!(
            "{} [{}] device={} provider={}",
            agent.agent_id,
            agent.agent_kind,
            agent
                .device
                .as_ref()
                .map(|device| device.device_id.as_str())
                .unwrap_or("-"),
            agent.provider_id.as_deref().unwrap_or("-"),
        );
        print_agent_common(agent);
    }
}

fn print_agent(agent: &AgentSnapshot) {
    println!("agent_id: {}", agent.agent_id);
    println!("agent_name: {}", agent.agent_name);
    println!("agent_kind: {}", agent.agent_kind);
    println!(
        "provider_id: {}",
        agent.provider_id.as_deref().unwrap_or("-")
    );
    println!(
        "device_id: {}",
        agent
            .device
            .as_ref()
            .map(|device| device.device_id.as_str())
            .unwrap_or("-")
    );
    if let Some(device) = &agent.device {
        if let Some(hostname) = &device.hostname {
            println!("hostname: {hostname}");
        }
        if let Some(platform) = &device.platform {
            println!("platform: {platform}");
        }
    }
    println!(
        "capabilities: {}",
        if agent.capabilities.is_empty() {
            "-".to_string()
        } else {
            agent.capabilities.join(", ")
        }
    );
    println!(
        "capability_domains: {}",
        if agent.capability_domains.is_empty() {
            "-".to_string()
        } else {
            agent
                .capability_domains
                .iter()
                .map(format_capability_domain)
                .collect::<Vec<_>>()
                .join(", ")
        }
    );
}

fn print_agent_common(agent: &AgentSnapshot) {
    if let Some(device) = &agent.device {
        if let Some(hostname) = &device.hostname {
            println!("  hostname: {hostname}");
        }
        if let Some(platform) = &device.platform {
            println!("  platform: {platform}");
        }
    }
    if !agent.capabilities.is_empty() {
        println!("  capabilities: {}", agent.capabilities.join(", "));
    }
    if !agent.capability_domains.is_empty() {
        println!(
            "  domains: {}",
            agent
                .capability_domains
                .iter()
                .map(format_capability_domain)
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
}

fn format_capability_domain(domain: &CapabilityDomain) -> String {
    match domain {
        CapabilityDomain::Asr => "asr".to_string(),
        CapabilityDomain::Tts => "tts".to_string(),
        CapabilityDomain::Transport => "transport".to_string(),
    }
}

fn format_check<T>(check: &DoctorCheckResult<T>) -> String {
    if check.skipped {
        return format!("skipped ({})", check.error.as_deref().unwrap_or("n/a"));
    }
    if check.ok {
        "ok".to_string()
    } else {
        format!(
            "failed ({})",
            check.error.as_deref().unwrap_or("unknown error")
        )
    }
}

fn print_detail(label: &str, detail: Option<&str>) {
    if let Some(detail) = detail
        && !detail.is_empty()
    {
        println!("  {label}: {detail}");
    }
}

fn parse_playback_target(raw: &str) -> Result<PlaybackTarget> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        bail!("playback target cannot be empty");
    }
    let Some((device_id, output_target)) = trimmed.split_once(':') else {
        return Ok(PlaybackTarget {
            device_id: trimmed.to_string(),
            output_target: None,
        });
    };
    let device_id = device_id.trim();
    let output_target = output_target.trim();
    if device_id.is_empty() {
        bail!("playback target host is empty in {trimmed:?}");
    }
    if output_target.is_empty() {
        bail!("playback target output is empty in {trimmed:?}");
    }
    Ok(PlaybackTarget {
        device_id: device_id.to_string(),
        output_target: Some(output_target.to_string()),
    })
}

async fn run_say(cli: &RuntimeConfig, args: SayArgs) -> Result<()> {
    let target_device = args.device.clone().or_else(|| cli.defaults.device.clone());
    if target_device.is_none() && args.agent_id.is_none() {
        bail!("one of --device or --agent-id is required");
    }

    let text = load_text_input(&args.tts.input).await?;
    if text.trim().is_empty() {
        bail!("TTS input is empty");
    }

    // Remote playback does not require live audio chunk delivery. Buffered TTS
    // avoids the MiniMax streaming websocket path and is more robust here.
    let request = build_tts_request_with_stream(cli, &args.tts, false)?;
    let synthesized = synthesize_tts(cli, request, text).await?;
    let control_url = args
        .control_url
        .or_else(|| cli.defaults.control_url.clone())
        .or_else(|| cli.control_url.clone())
        .unwrap_or_else(|| derive_control_url(&cli.url));
    let chunk_size_bytes = args
        .chunk_size_bytes
        .or(cli.defaults.chunk_size_bytes)
        .unwrap_or(16 * 1024)
        .max(1);
    let parsed_target = target_device
        .as_deref()
        .map(parse_playback_target)
        .transpose()?;
    let accepted = send_play_audio(
        &control_url,
        ControlPlayAudioPayload {
            task_id: None,
            device_id: parsed_target
                .as_ref()
                .map(|target| target.device_id.clone()),
            output_target: parsed_target.and_then(|target| target.output_target),
            agent_id: args.agent_id,
            format: synthesized.format,
            audio_base64: BASE64.encode(&synthesized.audio),
            chunk_size_bytes: Some(chunk_size_bytes),
        },
    )
    .await?;

    if cli.json {
        print_serialized(&accepted)?;
    } else if !cli.jsonl {
        println!("{}", serde_json::to_string_pretty(&accepted)?);
    }
    Ok(())
}

async fn run_asr(cli: &RuntimeConfig, command: AsrCommand) -> Result<()> {
    match command.command {
        AsrSubcommand::Transcribe(args) => run_asr_transcribe(cli, args).await,
    }
}

async fn connect_client(cli: &RuntimeConfig) -> Result<Client> {
    let mut config = ClientConfig::new(cli.url.clone());
    config.client_name = cli.client_name.clone();
    Client::connect(config)
        .await
        .context("failed to connect to SpeechMesh")
}

fn provider_selector(args: ProviderArgs) -> ProviderSelector {
    let mut selector = if let Some(provider_id) = args.provider {
        ProviderSelector::provider(provider_id)
    } else {
        ProviderSelector::default()
    };
    selector.required_capabilities = args.required_capabilities;
    selector.preferred_capabilities = args.preferred_capabilities;
    selector
}

fn merge_provider_args(cli: &RuntimeConfig, args: ProviderArgs) -> ProviderArgs {
    merge_provider_args_with_voice_profile(cli, args, None)
}

fn merge_provider_args_with_voice_profile(
    cli: &RuntimeConfig,
    args: ProviderArgs,
    voice_profile: Option<&VoiceProfileConfig>,
) -> ProviderArgs {
    let mut merged = args;
    if merged.provider.is_none() {
        merged.provider = voice_profile
            .and_then(|profile| profile.provider.clone())
            .or_else(|| cli.defaults.provider.clone());
    }
    if merged.required_capabilities.is_empty() {
        merged.required_capabilities = cli.defaults.require.clone();
    }
    if merged.preferred_capabilities.is_empty() {
        merged.preferred_capabilities = cli.defaults.prefer.clone();
    }
    merged
}

fn output_audio_format(cli: &RuntimeConfig, args: &TtsRunArgs) -> AudioFormat {
    let encoding = args
        .format
        .or(cli.defaults.format)
        .unwrap_or(OutputEncodingArg::Mp3);
    AudioFormat {
        encoding: match encoding {
            OutputEncodingArg::Mp3 => AudioEncoding::Mp3,
            OutputEncodingArg::Wav => AudioEncoding::Wav,
            OutputEncodingArg::Flac => AudioEncoding::Flac,
        },
        sample_rate_hz: args
            .sample_rate
            .or(cli.defaults.sample_rate)
            .unwrap_or(32000),
        channels: args.channels.or(cli.defaults.channels).unwrap_or(1),
    }
}

fn build_tts_request_with_stream(
    cli: &RuntimeConfig,
    args: &TtsRunArgs,
    stream: bool,
) -> Result<TtsStreamRequest> {
    let voice_profile = select_voice_profile(cli, args.voice_profile.as_deref())?;
    Ok(TtsStreamRequest {
        provider: provider_selector(merge_provider_args_with_voice_profile(
            cli,
            args.provider.clone(),
            voice_profile,
        )),
        input_kind: SynthesisInputKind::Text,
        output_format: Some(output_audio_format(cli, args)),
        options: SynthesisOptions {
            language: args
                .language
                .clone()
                .or_else(|| voice_profile.and_then(|profile| profile.language.clone()))
                .or_else(|| cli.defaults.language.clone()),
            voice: args
                .voice
                .clone()
                .or_else(|| voice_profile.and_then(|profile| profile.voice.clone()))
                .or_else(|| cli.defaults.voice.clone()),
            stream,
            rate: args
                .rate
                .or_else(|| voice_profile.and_then(|profile| profile.rate))
                .or(cli.defaults.rate),
            pitch: args
                .pitch
                .or_else(|| voice_profile.and_then(|profile| profile.pitch))
                .or(cli.defaults.pitch),
            volume: args
                .volume
                .or_else(|| voice_profile.and_then(|profile| profile.volume))
                .or(cli.defaults.volume),
            ..SynthesisOptions::default()
        },
    })
}

fn build_tts_request(cli: &RuntimeConfig, args: &TtsRunArgs) -> Result<TtsStreamRequest> {
    build_tts_request_with_stream(cli, args, true)
}

async fn load_text_input(args: &TextInputArgs) -> Result<String> {
    if let Some(text) = &args.text {
        return Ok(text.clone());
    }
    if let Some(path) = &args.text_file {
        return fs::read_to_string(path)
            .with_context(|| format!("failed to read text file {}", path.display()));
    }
    if args.stdin {
        let mut input = String::new();
        io::stdin()
            .read_to_string(&mut input)
            .context("failed to read text from stdin")?;
        return Ok(input);
    }
    bail!("one of --text, --text-file, or --stdin is required")
}

async fn load_audio_input(args: &AudioInputArgs) -> Result<Vec<u8>> {
    if let Some(path) = &args.file {
        return fs::read(path)
            .with_context(|| format!("failed to read audio file {}", path.display()));
    }
    if args.stdin {
        let mut bytes = Vec::new();
        io::stdin()
            .read_to_end(&mut bytes)
            .context("failed to read audio bytes from stdin")?;
        return Ok(bytes);
    }
    bail!("one of --file or --stdin is required")
}

async fn run_tts_play(cli: &RuntimeConfig, args: TtsRunArgs) -> Result<()> {
    let output = output_audio_format(cli, &args);
    let player = spawn_player(output.encoding).await?;
    run_tts_stream(cli, args, OutputSink::Player(player)).await
}

async fn run_tts_stream(cli: &RuntimeConfig, args: TtsRunArgs, mut sink: OutputSink) -> Result<()> {
    let text = load_text_input(&args.input).await?;
    if text.trim().is_empty() {
        bail!("TTS input is empty")
    }

    let request = build_tts_request(cli, &args)?;
    stream_tts_to_sink(cli, request, text, &mut sink).await?;

    sink.finish().await?;
    Ok(())
}

async fn stream_tts_to_sink(
    cli: &RuntimeConfig,
    request: TtsStreamRequest,
    text: String,
    sink: &mut OutputSink,
) -> Result<()> {
    let mut client = connect_client(cli).await?;
    let started = client.start_tts(request).await?;
    if !cli.json && !cli.jsonl {
        eprintln!(
            "started tts session {:?} with provider {}",
            started.session_id, started.payload.provider_id
        );
    }
    client.append_tts_input(text).await?;
    client.commit().await?;

    let mut saw_audio_done = false;
    let mut saw_session_end = false;

    while !(saw_audio_done && saw_session_end) {
        match client.recv().await? {
            GatewayMessage::TtsAudioDelta { payload, .. } => {
                let bytes = BASE64
                    .decode(payload.audio_base64.as_bytes())
                    .context("failed to decode tts audio chunk")?;
                sink.write_all(&bytes).await?;
                if cli.jsonl {
                    print_jsonl(&payload)?;
                }
            }
            GatewayMessage::TtsAudioDone { payload, .. } => {
                saw_audio_done = true;
                sink.flush().await?;
                if cli.jsonl {
                    print_jsonl(&payload)?;
                } else if !cli.json {
                    eprintln!(
                        "tts completed: {} chunks, {} bytes",
                        payload.total_chunks, payload.total_bytes
                    );
                }
            }
            GatewayMessage::SessionEnded { payload, .. } => {
                saw_session_end = true;
                if cli.jsonl {
                    print_jsonl(&payload)?;
                } else if !cli.json {
                    let reason = payload.reason.unwrap_or_else(|| "completed".to_string());
                    eprintln!("session ended: {reason}");
                }
            }
            GatewayMessage::Error { payload, .. } => {
                bail!(
                    "server error: {} ({})",
                    payload.error.message,
                    payload.error.code
                )
            }
            other => {
                if cli.jsonl {
                    print_jsonl(&other)?;
                }
            }
        }
    }

    Ok(())
}

async fn synthesize_tts(
    cli: &RuntimeConfig,
    request: TtsStreamRequest,
    text: String,
) -> Result<SynthesizedAudio> {
    let mut sink = OutputSink::Buffer(Vec::new());
    stream_tts_to_sink(cli, request, text, &mut sink).await?;
    match sink {
        OutputSink::Buffer(bytes) if bytes.is_empty() => bail!("TTS produced empty audio"),
        OutputSink::Buffer(bytes) => Ok(SynthesizedAudio {
            audio: bytes,
            format: None,
        }),
        _ => bail!("internal error: unexpected non-buffer sink"),
    }
}

async fn run_asr_transcribe(cli: &RuntimeConfig, args: AsrTranscribeArgs) -> Result<()> {
    let audio_bytes = load_audio_input(&args.input).await?;
    if audio_bytes.is_empty() {
        bail!("ASR input is empty")
    }

    let mut client = connect_client(cli).await?;
    let interim_requested = args.interim || cli.defaults.asr_interim.unwrap_or(false);
    let input_format = AudioFormat {
        encoding: args
            .input
            .encoding
            .or(cli.defaults.asr_encoding)
            .unwrap_or(AudioEncodingArg::PcmS16Le)
            .into(),
        sample_rate_hz: args
            .input
            .sample_rate
            .or(cli.defaults.asr_sample_rate)
            .unwrap_or(16000),
        channels: args
            .input
            .channels
            .or(cli.defaults.asr_channels)
            .unwrap_or(1),
    };
    let punctuation = if args.punctuation {
        true
    } else if args.no_punctuation {
        false
    } else {
        cli.defaults.asr_punctuation.unwrap_or(true)
    };
    let started = client
        .start_asr(StreamRequest {
            provider: provider_selector(merge_provider_args(cli, args.provider)),
            input_format,
            options: RecognitionOptions {
                language: args.language.or_else(|| cli.defaults.asr_language.clone()),
                hints: if args.hints.is_empty() {
                    cli.defaults.asr_hints.clone()
                } else {
                    args.hints
                },
                interim_results: interim_requested,
                timestamps: if args.timestamps {
                    true
                } else {
                    cli.defaults.asr_timestamps.unwrap_or(false)
                },
                punctuation,
                ..RecognitionOptions::default()
            },
        })
        .await?;
    if !cli.json && !cli.jsonl {
        eprintln!(
            "started asr session {:?} with provider {}",
            started.session_id, started.payload.provider_id
        );
    }

    client.send_audio(&audio_bytes).await?;
    client.commit().await?;

    let mut final_text = None;
    loop {
        match client.recv().await? {
            GatewayMessage::AsrResult { payload, .. } => {
                if cli.jsonl {
                    print_jsonl(&payload)?;
                } else if payload.is_final && payload.speech_final {
                    final_text = Some(payload.text.clone());
                } else if interim_requested && !cli.json {
                    eprintln!("partial: {}", payload.text);
                }
            }
            GatewayMessage::SessionEnded { payload, .. } => {
                if cli.jsonl {
                    print_jsonl(&payload)?;
                }
                break;
            }
            GatewayMessage::Error { payload, .. } => {
                bail!(
                    "server error: {} ({})",
                    payload.error.message,
                    payload.error.code
                )
            }
            other => {
                if cli.jsonl {
                    print_jsonl(&other)?;
                }
            }
        }
    }

    if cli.json {
        print_serialized(&serde_json::json!({ "text": final_text }))?;
    } else if !cli.jsonl {
        println!("{}", final_text.unwrap_or_default());
    }
    Ok(())
}

async fn spawn_player(encoding: AudioEncoding) -> Result<ChildStdin> {
    let format = match encoding {
        AudioEncoding::Mp3 => "mp3",
        AudioEncoding::Wav => "wav",
        AudioEncoding::Flac => "flac",
        other => bail!("streaming playback is unsupported for output encoding {other:?}"),
    };

    let mut child = Command::new("ffplay")
        .args([
            "-nodisp",
            "-autoexit",
            "-loglevel",
            "error",
            "-f",
            format,
            "-i",
            "pipe:0",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()
        .context("failed to launch ffplay; install ffmpeg/ffplay or use `tts stream`")?;
    child
        .stdin
        .take()
        .ok_or_else(|| anyhow!("ffplay stdin is unavailable"))
}

fn print_serialized<T: serde::Serialize>(value: &T) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}

fn print_jsonl<T: serde::Serialize>(value: &T) -> Result<()> {
    println!("{}", serde_json::to_string(value)?);
    Ok(())
}

enum OutputSink {
    Stdout,
    Player(ChildStdin),
    Buffer(Vec<u8>),
}

impl OutputSink {
    async fn write_all(&mut self, bytes: &[u8]) -> Result<()> {
        match self {
            OutputSink::Stdout => {
                let mut stdout = io::stdout();
                stdout.write_all(bytes)?;
                stdout.flush()?;
            }
            OutputSink::Player(stdin) => {
                stdin.write_all(bytes).await?;
            }
            OutputSink::Buffer(buffer) => buffer.extend_from_slice(bytes),
        }
        Ok(())
    }

    async fn flush(&mut self) -> Result<()> {
        match self {
            OutputSink::Stdout => {
                let mut stdout = io::stdout();
                stdout.flush()?;
            }
            OutputSink::Player(stdin) => {
                stdin.flush().await?;
            }
            OutputSink::Buffer(_) => {}
        }
        Ok(())
    }

    async fn finish(self) -> Result<()> {
        match self {
            OutputSink::Stdout => Ok(()),
            OutputSink::Player(mut stdin) => {
                stdin.shutdown().await?;
                Ok(())
            }
            OutputSink::Buffer(_) => Ok(()),
        }
    }
}

#[derive(Debug)]
struct SynthesizedAudio {
    audio: Vec<u8>,
    format: Option<AudioFormat>,
}

#[derive(Debug, Clone, Default)]
struct FileConfig {
    active_profile: Option<String>,
    profiles: HashMap<String, ProfileConfig>,
    voice_profiles: HashMap<String, VoiceProfileConfig>,
    project_voice_profiles: HashMap<String, ProjectVoiceBinding>,
    default_profile: ProfileConfig,
}

#[derive(Debug, Clone, Default)]
struct ProfileConfig {
    gateway: GatewayConfig,
    client_name: Option<String>,
    defaults: DefaultsConfig,
}

#[derive(Debug, Clone, Default)]
struct GatewayConfig {
    ws_url: Option<String>,
    control_url: Option<String>,
}

#[derive(Debug, Clone, Default)]
struct DefaultsConfig {
    device: Option<String>,
    provider: Option<String>,
    require: Vec<String>,
    prefer: Vec<String>,
    voice: Option<String>,
    language: Option<String>,
    rate: Option<f32>,
    pitch: Option<f32>,
    volume: Option<f32>,
    format: Option<OutputEncodingArg>,
    sample_rate: Option<u32>,
    channels: Option<u16>,
    control_url: Option<String>,
    chunk_size_bytes: Option<usize>,
    asr_encoding: Option<AudioEncodingArg>,
    asr_sample_rate: Option<u32>,
    asr_channels: Option<u16>,
    asr_language: Option<String>,
    asr_interim: Option<bool>,
    asr_punctuation: Option<bool>,
    asr_timestamps: Option<bool>,
    asr_hints: Vec<String>,
}

#[derive(Debug, Clone, Default)]
struct VoiceProfileConfig {
    provider: Option<String>,
    voice: Option<String>,
    language: Option<String>,
    rate: Option<f32>,
    pitch: Option<f32>,
    volume: Option<f32>,
}

#[derive(Debug, Clone, Default)]
struct ProjectVoiceBinding {
    root: Option<PathBuf>,
    voice_profile: Option<String>,
}

fn load_cli_config(explicit_path: Option<&std::path::Path>) -> Result<Option<FileConfig>> {
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
    if let Some(name) = selected
        && let Some(profile) = config.profiles.get(name)
    {
        return Some(profile);
    }
    if let Some(name) = config.active_profile.as_deref()
        && let Some(profile) = config.profiles.get(name)
    {
        return Some(profile);
    }
    if let Some(profile) = config.profiles.get("default") {
        return Some(profile);
    }
    if config.default_profile.gateway.ws_url.is_some()
        || config.default_profile.gateway.control_url.is_some()
        || config.default_profile.client_name.is_some()
        || defaults_present(&config.default_profile.defaults)
    {
        return Some(&config.default_profile);
    }
    None
}

fn defaults_present(defaults: &DefaultsConfig) -> bool {
    defaults.device.is_some()
        || defaults.provider.is_some()
        || !defaults.require.is_empty()
        || !defaults.prefer.is_empty()
        || defaults.voice.is_some()
        || defaults.language.is_some()
        || defaults.rate.is_some()
        || defaults.pitch.is_some()
        || defaults.volume.is_some()
        || defaults.format.is_some()
        || defaults.sample_rate.is_some()
        || defaults.channels.is_some()
        || defaults.control_url.is_some()
        || defaults.chunk_size_bytes.is_some()
        || defaults.asr_encoding.is_some()
        || defaults.asr_sample_rate.is_some()
        || defaults.asr_channels.is_some()
        || defaults.asr_language.is_some()
        || defaults.asr_interim.is_some()
        || defaults.asr_punctuation.is_some()
        || defaults.asr_timestamps.is_some()
        || !defaults.asr_hints.is_empty()
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
    let mut current_voice_profile: Option<String> = None;
    let mut current_project_binding: Option<String> = None;
    let mut in_profiles = false;
    let mut in_voice_profiles = false;
    let mut in_project_voice_profiles = false;
    let mut in_gateway = false;
    let mut in_defaults = false;

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
                current_voice_profile = None;
                current_project_binding = None;
                in_gateway = false;
                in_defaults = false;
                if trimmed == "profiles:" {
                    in_profiles = true;
                    in_voice_profiles = false;
                    in_project_voice_profiles = false;
                    continue;
                }
                if trimmed == "voice_profiles:" {
                    in_profiles = false;
                    in_voice_profiles = true;
                    in_project_voice_profiles = false;
                    continue;
                }
                if trimmed == "project_voice_profiles:" {
                    in_profiles = false;
                    in_voice_profiles = false;
                    in_project_voice_profiles = true;
                    continue;
                }
                in_profiles = false;
                in_voice_profiles = false;
                in_project_voice_profiles = false;
                if trimmed == "gateway:" {
                    in_gateway = true;
                    continue;
                }
                if trimmed == "defaults:" {
                    in_defaults = true;
                    continue;
                }
                assign_profile_value(&mut config.default_profile, trimmed)?;
                if let Some((key, value)) = split_key_value(trimmed)
                    && key == "active_profile"
                {
                    config.active_profile = Some(value.to_string());
                }
            }
            2 if in_profiles && trimmed.ends_with(':') => {
                let name = trimmed.trim_end_matches(':').trim().to_string();
                current_profile = Some(name.clone());
                config.profiles.entry(name).or_default();
                in_gateway = false;
                in_defaults = false;
            }
            2 if in_voice_profiles && trimmed.ends_with(':') => {
                let name = trimmed.trim_end_matches(':').trim().to_string();
                current_voice_profile = Some(name.clone());
                config.voice_profiles.entry(name).or_default();
            }
            2 if in_project_voice_profiles && trimmed.ends_with(':') => {
                let name = trimmed.trim_end_matches(':').trim().to_string();
                current_project_binding = Some(name.clone());
                config.project_voice_profiles.entry(name).or_default();
            }
            2 if !in_profiles && in_gateway => {
                assign_gateway_value(&mut config.default_profile.gateway, trimmed)?;
            }
            2 if !in_profiles && in_defaults => {
                assign_defaults_value(&mut config.default_profile.defaults, trimmed)?;
            }
            4 if current_profile.is_some() && trimmed == "gateway:" => {
                in_gateway = true;
                in_defaults = false;
            }
            4 if current_profile.is_some() && trimmed == "defaults:" => {
                in_defaults = true;
                in_gateway = false;
            }
            4 if current_profile.is_some() => {
                in_gateway = false;
                in_defaults = false;
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
            6 if current_profile.is_some() && in_defaults => {
                let profile = config
                    .profiles
                    .get_mut(current_profile.as_deref().unwrap_or_default())
                    .ok_or_else(|| anyhow!("unknown profile while parsing"))?;
                assign_defaults_value(&mut profile.defaults, trimmed)?;
            }
            4 if current_voice_profile.is_some() && in_voice_profiles => {
                let profile = config
                    .voice_profiles
                    .get_mut(current_voice_profile.as_deref().unwrap_or_default())
                    .ok_or_else(|| anyhow!("unknown voice profile while parsing"))?;
                assign_voice_profile_value(profile, trimmed)?;
            }
            4 if current_project_binding.is_some() && in_project_voice_profiles => {
                let binding = config
                    .project_voice_profiles
                    .get_mut(current_project_binding.as_deref().unwrap_or_default())
                    .ok_or_else(|| anyhow!("unknown project voice binding while parsing"))?;
                assign_project_voice_binding_value(binding, trimmed)?;
            }
            _ => {}
        }
    }

    Ok(config)
}

fn assign_voice_profile_value(profile: &mut VoiceProfileConfig, trimmed: &str) -> Result<()> {
    let Some((key, value)) = split_key_value(trimmed) else {
        return Ok(());
    };
    match key {
        "provider" => profile.provider = Some(value.to_string()),
        "voice" => profile.voice = Some(value.to_string()),
        "language" => profile.language = Some(value.to_string()),
        "rate" => profile.rate = Some(parse_number(value, key)?),
        "pitch" => profile.pitch = Some(parse_number(value, key)?),
        "volume" => profile.volume = Some(parse_number(value, key)?),
        _ => {}
    }
    Ok(())
}

fn assign_project_voice_binding_value(
    binding: &mut ProjectVoiceBinding,
    trimmed: &str,
) -> Result<()> {
    let Some((key, value)) = split_key_value(trimmed) else {
        return Ok(());
    };
    match key {
        "root" => binding.root = Some(PathBuf::from(value)),
        "voice_profile" => binding.voice_profile = Some(value.to_string()),
        _ => {}
    }
    Ok(())
}

fn select_voice_profile<'a>(
    cli: &'a RuntimeConfig,
    selected: Option<&str>,
) -> Result<Option<&'a VoiceProfileConfig>> {
    if let Some(name) = selected {
        let profile = cli
            .voice_profiles
            .get(name)
            .ok_or_else(|| anyhow!("unknown voice profile: {name}"))?;
        return Ok(Some(profile));
    }
    let cwd = match std::env::current_dir() {
        Ok(path) => path,
        Err(_) => return Ok(None),
    };
    Ok(select_voice_profile_for_path(cli, &cwd))
}

fn select_voice_profile_for_path<'a>(
    cli: &'a RuntimeConfig,
    cwd: &Path,
) -> Option<&'a VoiceProfileConfig> {
    let cwd = canonicalize_if_exists(cwd);
    cli.project_voice_profiles
        .values()
        .filter_map(|binding| {
            let root = binding.root.as_ref()?;
            let profile_name = binding.voice_profile.as_ref()?;
            let root = canonicalize_if_exists(root);
            if !path_has_prefix(&cwd, &root) {
                return None;
            }
            let profile = cli.voice_profiles.get(profile_name)?;
            Some((root.components().count(), profile))
        })
        .max_by_key(|(depth, _)| *depth)
        .map(|(_, profile)| profile)
}

fn canonicalize_if_exists(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

fn path_has_prefix(path: &Path, prefix: &Path) -> bool {
    path.starts_with(prefix)
}

fn assign_profile_value(profile: &mut ProfileConfig, trimmed: &str) -> Result<()> {
    if trimmed == "gateway:" {
        return Ok(());
    }
    let Some((key, value)) = split_key_value(trimmed) else {
        return Ok(());
    };
    match key {
        "client_name" => profile.client_name = Some(value.to_string()),
        "ws_url" => profile.gateway.ws_url = Some(value.to_string()),
        "control_url" => profile.gateway.control_url = Some(value.to_string()),
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
        "control_url" => gateway.control_url = Some(value.to_string()),
        _ => {}
    }
    Ok(())
}

fn assign_defaults_value(defaults: &mut DefaultsConfig, trimmed: &str) -> Result<()> {
    let Some((key, value)) = split_key_value(trimmed) else {
        return Ok(());
    };
    match key {
        "device" => defaults.device = Some(value.to_string()),
        "provider" => defaults.provider = Some(value.to_string()),
        "require" => defaults.require = parse_csv_list(value),
        "prefer" => defaults.prefer = parse_csv_list(value),
        "voice" => defaults.voice = Some(value.to_string()),
        "language" => defaults.language = Some(value.to_string()),
        "rate" => defaults.rate = Some(parse_number(value, key)?),
        "pitch" => defaults.pitch = Some(parse_number(value, key)?),
        "volume" => defaults.volume = Some(parse_number(value, key)?),
        "format" => defaults.format = Some(parse_output_encoding(value)?),
        "sample_rate" => defaults.sample_rate = Some(parse_number(value, key)?),
        "channels" => defaults.channels = Some(parse_number(value, key)?),
        "control_url" => defaults.control_url = Some(value.to_string()),
        "chunk_size_bytes" => defaults.chunk_size_bytes = Some(parse_number(value, key)?),
        "asr_encoding" => defaults.asr_encoding = Some(parse_audio_encoding(value)?),
        "asr_sample_rate" => defaults.asr_sample_rate = Some(parse_number(value, key)?),
        "asr_channels" => defaults.asr_channels = Some(parse_number(value, key)?),
        "asr_language" => defaults.asr_language = Some(value.to_string()),
        "asr_interim" => defaults.asr_interim = Some(parse_bool(value, key)?),
        "asr_punctuation" => defaults.asr_punctuation = Some(parse_bool(value, key)?),
        "asr_timestamps" => defaults.asr_timestamps = Some(parse_bool(value, key)?),
        "asr_hints" => defaults.asr_hints = parse_csv_list(value),
        _ => {}
    }
    Ok(())
}

fn parse_number<T: std::str::FromStr>(value: &str, key: &str) -> Result<T>
where
    T::Err: std::fmt::Display,
{
    value
        .parse::<T>()
        .map_err(|err| anyhow!("invalid value for {key}: {value} ({err})"))
}

fn parse_bool(value: &str, key: &str) -> Result<bool> {
    match value {
        "true" | "yes" | "on" | "1" => Ok(true),
        "false" | "no" | "off" | "0" => Ok(false),
        _ => bail!("invalid value for {key}: {value}"),
    }
}

fn parse_csv_list(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

fn parse_output_encoding(value: &str) -> Result<OutputEncodingArg> {
    match value {
        "mp3" => Ok(OutputEncodingArg::Mp3),
        "wav" => Ok(OutputEncodingArg::Wav),
        "flac" => Ok(OutputEncodingArg::Flac),
        _ => bail!("invalid value for format: {value}"),
    }
}

fn parse_audio_encoding(value: &str) -> Result<AudioEncodingArg> {
    match value {
        "pcm-s16le" => Ok(AudioEncodingArg::PcmS16Le),
        "pcm-f32le" => Ok(AudioEncodingArg::PcmF32Le),
        "opus" => Ok(AudioEncodingArg::Opus),
        "mp3" => Ok(AudioEncodingArg::Mp3),
        "aac" => Ok(AudioEncodingArg::Aac),
        "flac" => Ok(AudioEncodingArg::Flac),
        "wav" => Ok(AudioEncodingArg::Wav),
        _ => bail!("invalid value for asr_encoding: {value}"),
    }
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

fn derive_control_url(ws_url: &str) -> String {
    if let Some(base) = ws_url.strip_suffix("/ws") {
        format!("{base}/control")
    } else {
        format!("{}/control", ws_url.trim_end_matches('/'))
    }
}

async fn send_play_audio(
    url: &str,
    payload: ControlPlayAudioPayload,
) -> Result<ControlPlayAudioAcceptedPayload> {
    let request = ControlRequest::PlayAudio { payload };
    request_control_to_url(url, request, |message| match message {
        ControlResponse::PlayAudioAccepted { payload } => Some(payload),
        _ => None,
    })
    .await
}

async fn request_control<T, F>(
    cli: &RuntimeConfig,
    request: ControlRequest,
    extractor: F,
) -> Result<T>
where
    F: Fn(ControlResponse) -> Option<T>,
{
    let control_url = cli
        .defaults
        .control_url
        .clone()
        .or_else(|| cli.control_url.clone())
        .unwrap_or_else(|| derive_control_url(&cli.url));
    request_control_to_url(&control_url, request, extractor).await
}

async fn request_control_to_url<T, F>(url: &str, request: ControlRequest, extractor: F) -> Result<T>
where
    F: Fn(ControlResponse) -> Option<T>,
{
    let encoded = serde_json::to_string(&request).context("failed to encode control payload")?;

    let (mut websocket, response) = connect_async(url)
        .await
        .with_context(|| format!("failed to connect to {url}"))?;
    if response.status().as_u16() != 101 {
        bail!(
            "control websocket handshake failed with {}",
            response.status()
        );
    }

    websocket
        .send(Message::Text(encoded.into()))
        .await
        .context("failed to send play_audio control request")?;

    while let Some(frame) = websocket.next().await {
        let frame = frame.context("failed to read /control response frame")?;
        match frame {
            Message::Text(text) => {
                let response: ControlResponse = serde_json::from_str(&text)
                    .map_err(|error| anyhow!("unexpected /control response ({error}): {text}"))?;
                if let Some(payload) = extractor(response) {
                    return Ok(payload);
                }
                let response: ControlResponse = serde_json::from_str(&text)
                    .map_err(|error| anyhow!("unexpected /control response ({error}): {text}"))?;
                match response {
                    ControlResponse::Error { payload } => {
                        bail!("control request failed: {}", payload.message);
                    }
                    ControlResponse::Pong {} => {}
                    _ => {}
                }
            }
            Message::Ping(payload) => {
                websocket
                    .send(Message::Pong(payload))
                    .await
                    .context("failed to send pong")?;
            }
            Message::Close(_) => bail!("control websocket closed before response"),
            Message::Binary(_) | Message::Pong(_) | Message::Frame(_) => {}
        }
    }

    bail!("control websocket closed without response")
}

fn annotate_control_capability_error(error: anyhow::Error, command: &str) -> anyhow::Error {
    let message = format!("{error:#}");
    if message.contains("unknown variant") {
        return anyhow!(
            "gateway control plane does not support `{command}` yet; deploy the updated `speechmeshd` before retrying"
        );
    }
    error
}

impl From<AudioEncodingArg> for AudioEncoding {
    fn from(value: AudioEncodingArg) -> Self {
        match value {
            AudioEncodingArg::PcmS16Le => AudioEncoding::PcmS16Le,
            AudioEncodingArg::PcmF32Le => AudioEncoding::PcmF32Le,
            AudioEncodingArg::Opus => AudioEncoding::Opus,
            AudioEncodingArg::Mp3 => AudioEncoding::Mp3,
            AudioEncodingArg::Aac => AudioEncoding::Aac,
            AudioEncodingArg::Flac => AudioEncoding::Flac,
            AudioEncodingArg::Wav => AudioEncoding::Wav,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn runtime_with_voice_profiles() -> RuntimeConfig {
        let mut voice_profiles = HashMap::new();
        voice_profiles.insert(
            "repo".to_string(),
            VoiceProfileConfig {
                provider: Some("minimax.tts".to_string()),
                voice: Some("repo-voice".to_string()),
                language: Some("zh-CN".to_string()),
                rate: Some(1.05),
                pitch: Some(0.95),
                volume: Some(1.1),
            },
        );
        voice_profiles.insert(
            "nested".to_string(),
            VoiceProfileConfig {
                provider: Some("minimax.tts".to_string()),
                voice: Some("nested-voice".to_string()),
                language: Some("en-US".to_string()),
                rate: Some(1.2),
                pitch: None,
                volume: None,
            },
        );

        let mut project_voice_profiles = HashMap::new();
        project_voice_profiles.insert(
            "repo".to_string(),
            ProjectVoiceBinding {
                root: Some(PathBuf::from("/tmp/work")),
                voice_profile: Some("repo".to_string()),
            },
        );
        project_voice_profiles.insert(
            "nested".to_string(),
            ProjectVoiceBinding {
                root: Some(PathBuf::from("/tmp/work/nested")),
                voice_profile: Some("nested".to_string()),
            },
        );

        RuntimeConfig {
            url: "ws://127.0.0.1:8765/ws".to_string(),
            control_url: None,
            defaults: DefaultsConfig {
                provider: Some("default.tts".to_string()),
                voice: Some("default-voice".to_string()),
                language: Some("ja-JP".to_string()),
                rate: Some(0.9),
                pitch: Some(0.8),
                volume: Some(0.7),
                ..DefaultsConfig::default()
            },
            voice_profiles,
            project_voice_profiles,
            client_name: "speechmesh".to_string(),
            json: false,
            jsonl: false,
        }
    }

    #[test]
    fn parse_file_config_reads_voice_profile_sections() {
        let parsed = parse_file_config(
            r#"
voice_profiles:
  codex:
    provider: minimax.tts
    voice: female-shaonv
    language: zh-CN
    rate: 1.1

project_voice_profiles:
  speechmesh:
    root: /Users/breaker/src/speechmesh
    voice_profile: codex
"#,
        )
        .expect("config should parse");

        let profile = parsed
            .voice_profiles
            .get("codex")
            .expect("voice profile should exist");
        assert_eq!(profile.provider.as_deref(), Some("minimax.tts"));
        assert_eq!(profile.voice.as_deref(), Some("female-shaonv"));
        assert_eq!(profile.language.as_deref(), Some("zh-CN"));
        assert_eq!(profile.rate, Some(1.1));

        let binding = parsed
            .project_voice_profiles
            .get("speechmesh")
            .expect("project binding should exist");
        assert_eq!(
            binding.root.as_deref(),
            Some(Path::new("/Users/breaker/src/speechmesh"))
        );
        assert_eq!(binding.voice_profile.as_deref(), Some("codex"));
    }

    #[test]
    fn select_voice_profile_uses_longest_matching_root_prefix() {
        let runtime = runtime_with_voice_profiles();
        let selected = select_voice_profile_for_path(&runtime, Path::new("/tmp/work/nested/app"))
            .expect("voice profile should be selected");
        assert_eq!(selected.voice.as_deref(), Some("nested-voice"));
    }

    #[test]
    fn explicit_flags_override_voice_profile_values() {
        let runtime = runtime_with_voice_profiles();
        let args = TtsRunArgs {
            provider: ProviderArgs {
                provider: Some("cli.tts".to_string()),
                required_capabilities: Vec::new(),
                preferred_capabilities: Vec::new(),
            },
            input: TextInputArgs {
                text: Some("hello".to_string()),
                text_file: None,
                stdin: false,
            },
            voice_profile: Some("repo".to_string()),
            voice: Some("cli-voice".to_string()),
            language: Some("en-GB".to_string()),
            rate: Some(1.3),
            pitch: Some(1.4),
            volume: Some(1.5),
            format: None,
            sample_rate: None,
            channels: None,
        };

        let request =
            build_tts_request_with_stream(&runtime, &args, false).expect("request should build");
        assert_eq!(request.provider.provider_id.as_deref(), Some("cli.tts"));
        assert_eq!(request.options.voice.as_deref(), Some("cli-voice"));
        assert_eq!(request.options.language.as_deref(), Some("en-GB"));
        assert_eq!(request.options.rate, Some(1.3));
        assert_eq!(request.options.pitch, Some(1.4));
        assert_eq!(request.options.volume, Some(1.5));
        assert!(!request.options.stream);
    }
}
