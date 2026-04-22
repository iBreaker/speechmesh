use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use anyhow::{Context, Result};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use speechmesh_core::{AudioEncoding, AudioFormat, CapabilityDomain};
use speechmeshd::agent::{
    AgentDeviceIdentity, AgentEmptyPayload, AgentErrorPayload, AgentHelloPayload, AgentKind,
    AgentSessionEndedPayload, AgentTaskState, AgentTaskStatusPayload, AgentToGatewayMessage,
    AgentUpdateStatus, GatewayToAgentMessage,
};
use tokio::io::AsyncWriteExt;
use tokio::process::{Child, ChildStderr, ChildStdin, Command};
use tokio::sync::mpsc;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::protocol::Message;
use tracing::{debug, info, warn};

#[derive(Debug, Clone)]
pub struct AgentRuntimeConfig {
    pub gateway_url: String,
    pub agent_id: String,
    pub agent_name: String,
    pub device_id: String,
    pub provider_id: String,
    pub shared_secret: Option<String>,
    pub capabilities: Vec<String>,
    pub reconnect_delay: Duration,
}

#[derive(Debug, Deserialize)]
struct LocalAutoUpdateState {
    #[serde(default)]
    unix_time_secs: Option<u64>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    current_version: Option<String>,
    #[serde(default)]
    target_version: Option<String>,
    #[serde(default)]
    applied: Option<bool>,
    #[serde(default)]
    restart_performed: Option<bool>,
    #[serde(default)]
    error: Option<String>,
}

#[derive(Default)]
struct TaskTracker {
    play_audio_tasks: HashMap<String, PlaybackTask>,
    failed_play_audio_starts: HashSet<String>,
}

struct PlaybackTask {
    child: Child,
    stdin: ChildStdin,
    stderr: Option<ChildStderr>,
    chunk_count: usize,
    byte_count: usize,
}

const PLAYBACK_FINISH_TIMEOUT: Duration = Duration::from_secs(20);
const PLAYBACK_CMD_ENV: &str = "SPEECHMESH_PLAYBACK_CMD";

/// Interval between agent-initiated WebSocket pings.
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(30);
/// If no frame (data or pong) is received within this window, treat the
/// connection as dead and reconnect.
const HEARTBEAT_TIMEOUT: Duration = Duration::from_secs(90);

enum PlaybackLauncher {
    ShellCommand(String),
    Ffplay(String),
    Mpv(String),
}

pub async fn run_forever(config: AgentRuntimeConfig) -> Result<()> {
    loop {
        match run_once(&config).await {
            Ok(()) => warn!("agent {} disconnected; reconnecting", config.agent_id),
            Err(error) => warn!("agent {} connection failed: {error:#}", config.agent_id),
        }
        tokio::time::sleep(config.reconnect_delay).await;
    }
}

async fn run_once(config: &AgentRuntimeConfig) -> Result<()> {
    let (websocket, response) = connect_async(&config.gateway_url)
        .await
        .with_context(|| format!("failed to connect to gateway {}", config.gateway_url))?;

    // Enable TCP keepalive on the underlying socket to prevent
    // NAT/VPN/firewall middleboxes from killing idle connections.
    if let tokio_tungstenite::MaybeTlsStream::Rustls(tls_stream) = websocket.get_ref() {
        let tcp = tls_stream.get_ref().0;
        if let Err(e) =
            speechmeshd::tcp_keepalive::configure(tcp, speechmeshd::tcp_keepalive::DEFAULT_INTERVAL)
        {
            warn!("failed to set TCP keepalive: {e}");
        }
    } else if let tokio_tungstenite::MaybeTlsStream::Plain(tcp) = websocket.get_ref() {
        if let Err(e) =
            speechmeshd::tcp_keepalive::configure(tcp, speechmeshd::tcp_keepalive::DEFAULT_INTERVAL)
        {
            warn!("failed to set TCP keepalive: {e}");
        }
    }

    info!(
        "device agent {} connected to {} status={} device_id={}",
        config.agent_id,
        config.gateway_url,
        response.status(),
        config.device_id
    );

    let (mut sink, mut source) = websocket.split();
    let (out_tx, mut out_rx) = mpsc::channel::<AgentToGatewayMessage>(64);
    let mut task_tracker = TaskTracker::default();

    let writer = tokio::spawn(async move {
        while let Some(message) = out_rx.recv().await {
            let encoded =
                serde_json::to_string(&message).context("failed to encode outbound agent frame")?;
            sink.send(Message::Text(encoded.into()))
                .await
                .context("failed to write outbound agent frame")?;
        }
        Ok::<(), anyhow::Error>(())
    });

    let hello = AgentToGatewayMessage::Hello {
        payload: AgentHelloPayload {
            agent_id: config.agent_id.clone(),
            agent_name: config.agent_name.clone(),
            provider_id: Some(config.provider_id.clone()),
            capabilities: config.capabilities.clone(),
            capability_domains: vec![CapabilityDomain::Tts],
            agent_kind: AgentKind::Device,
            device: Some(AgentDeviceIdentity {
                device_id: config.device_id.clone(),
                hostname: Some(hostname_fallback()),
                platform: Some(std::env::consts::OS.to_string()),
            }),
            client_version: Some(env!("CARGO_PKG_VERSION").to_string()),
            update_status: load_update_status(),
            shared_secret: config.shared_secret.clone(),
        },
    };
    out_tx
        .send(hello)
        .await
        .context("agent writer channel closed before hello")?;

    wait_hello_ok(&mut source).await?;

    let mut heartbeat = tokio::time::interval(HEARTBEAT_INTERVAL);
    heartbeat.tick().await; // consume the immediate first tick
    let mut last_recv = tokio::time::Instant::now();

    loop {
        tokio::select! {
            frame = source.next() => {
                let Some(frame) = frame else { break };
                let frame = frame.context("failed reading gateway frame")?;
                last_recv = tokio::time::Instant::now();
                match frame {
                    Message::Text(text) => {
                        let inbound: GatewayToAgentMessage =
                            serde_json::from_str(&text).context("invalid gateway /agent payload")?;
                        handle_gateway_message(inbound, &out_tx, &mut task_tracker).await?;
                    }
                    Message::Ping(_) => {
                        out_tx
                            .send(AgentToGatewayMessage::Pong {
                                payload: AgentEmptyPayload {},
                            })
                            .await
                            .context("agent writer channel closed while sending pong")?;
                    }
                    Message::Close(_) => break,
                    Message::Binary(_) | Message::Pong(_) | Message::Frame(_) => {}
                }
            }
            _ = heartbeat.tick() => {
                if last_recv.elapsed() > HEARTBEAT_TIMEOUT {
                    warn!(
                        "no data received for {:?}; treating connection as dead",
                        last_recv.elapsed()
                    );
                    break;
                }
                // Send an application-level ping so the gateway knows we are alive,
                // and so we exercise the write path to detect broken pipes early.
                if out_tx
                    .send(AgentToGatewayMessage::Pong {
                        payload: AgentEmptyPayload {},
                    })
                    .await
                    .is_err()
                {
                    break;
                }
                debug!("heartbeat ping sent");
            }
        }
    }

    abort_all_playback_tasks(&mut task_tracker).await;
    drop(out_tx);
    let _ = writer.await;
    Ok(())
}

async fn wait_hello_ok(
    source: &mut futures_util::stream::SplitStream<
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
    >,
) -> Result<()> {
    while let Some(frame) = source.next().await {
        let frame = frame.context("failed reading handshake frame")?;
        match frame {
            Message::Text(text) => {
                let message: GatewayToAgentMessage = serde_json::from_str(&text)
                    .context("invalid handshake payload from gateway")?;
                match message {
                    GatewayToAgentMessage::HelloOk { .. } => return Ok(()),
                    other => warn!("ignoring pre-hello-ok frame: {other:?}"),
                }
            }
            Message::Close(_) => {
                anyhow::bail!("gateway closed during hello handshake");
            }
            Message::Ping(_) | Message::Pong(_) | Message::Binary(_) | Message::Frame(_) => {}
        }
    }
    anyhow::bail!("gateway closed before hello.ok")
}

async fn handle_gateway_message(
    message: GatewayToAgentMessage,
    out_tx: &mpsc::Sender<AgentToGatewayMessage>,
    task_tracker: &mut TaskTracker,
) -> Result<()> {
    match message {
        GatewayToAgentMessage::HelloOk { .. } => {}
        GatewayToAgentMessage::Ping { .. } => {
            out_tx
                .send(AgentToGatewayMessage::Pong {
                    payload: AgentEmptyPayload {},
                })
                .await
                .context("agent writer channel closed while replying ping")?;
        }
        GatewayToAgentMessage::SessionStart { session_id, .. } => {
            warn!(
                "session.start received for {:?}; task handling is not implemented yet",
                session_id
            );
            out_tx
                .send(AgentToGatewayMessage::Error {
                    session_id: Some(session_id.clone()),
                    payload: AgentErrorPayload {
                        message: "device task handling is not implemented yet".to_string(),
                    },
                })
                .await
                .context("agent writer channel closed while sending stub error")?;
            out_tx
                .send(AgentToGatewayMessage::SessionEnded {
                    session_id,
                    payload: AgentSessionEndedPayload {
                        reason: Some("not_implemented".to_string()),
                    },
                })
                .await
                .context("agent writer channel closed while sending session.ended")?;
        }
        GatewayToAgentMessage::SessionAudio { session_id, .. }
        | GatewayToAgentMessage::SessionCommit { session_id, .. }
        | GatewayToAgentMessage::SessionStop { session_id, .. } => {
            debug!("received unsupported session frame for {:?}", session_id);
            out_tx
                .send(AgentToGatewayMessage::Error {
                    session_id: Some(session_id),
                    payload: AgentErrorPayload {
                        message: "session command unsupported by runtime skeleton".to_string(),
                    },
                })
                .await
                .context("agent writer channel closed while sending unsupported-session error")?;
        }
        GatewayToAgentMessage::TaskPlayAudioStart { task_id, payload } => {
            if task_tracker.play_audio_tasks.contains_key(&task_id) {
                out_tx
                    .send(AgentToGatewayMessage::TaskStatus {
                        task_id,
                        payload: AgentTaskStatusPayload {
                            state: AgentTaskState::Failed,
                            message: Some("duplicate play_audio task.start".to_string()),
                        },
                    })
                    .await
                    .context("agent writer channel closed while reporting duplicate task.start")?;
            } else {
                match start_playback_task(&payload.format).await {
                    Ok(task) => {
                        task_tracker.failed_play_audio_starts.remove(&task_id);
                        task_tracker.play_audio_tasks.insert(task_id.clone(), task);
                        info!("play_audio started task_id={task_id}");
                        out_tx
                            .send(AgentToGatewayMessage::TaskStatus {
                                task_id,
                                payload: AgentTaskStatusPayload {
                                    state: AgentTaskState::Started,
                                    message: Some(
                                        "play_audio started with local default output".to_string(),
                                    ),
                                },
                            })
                            .await
                            .context("agent writer channel closed while sending task started")?;
                    }
                    Err(error) => {
                        warn!("play_audio failed to start task_id={task_id}: {error:#}");
                        task_tracker
                            .failed_play_audio_starts
                            .insert(task_id.clone());
                        out_tx
                            .send(AgentToGatewayMessage::TaskStatus {
                                task_id,
                                payload: AgentTaskStatusPayload {
                                    state: AgentTaskState::Failed,
                                    message: Some(format!(
                                        "failed to start local playback: {error}"
                                    )),
                                },
                            })
                            .await
                            .context(
                                "agent writer channel closed while reporting task start failure",
                            )?;
                    }
                }
            }
        }
        GatewayToAgentMessage::TaskPlayAudioChunk { task_id, payload } => {
            if let Some(task) = task_tracker.play_audio_tasks.get_mut(&task_id) {
                let bytes = match BASE64_STANDARD.decode(payload.data_base64.as_bytes()) {
                    Ok(bytes) => bytes,
                    Err(error) => {
                        finish_failed_task(
                            task_tracker,
                            out_tx,
                            task_id,
                            format!("invalid play_audio chunk base64: {error}"),
                        )
                        .await?;
                        return Ok(());
                    }
                };
                if let Err(error) = task.stdin.write_all(&bytes).await {
                    finish_failed_task(
                        task_tracker,
                        out_tx,
                        task_id,
                        format!("failed writing audio chunk to player stdin: {error}"),
                    )
                    .await?;
                    return Ok(());
                }
                task.chunk_count += 1;
                task.byte_count += bytes.len();
                debug!(
                    "task.play_audio.chunk task_id={} chunk_index={} bytes(base64)={}",
                    task_id,
                    task.chunk_count,
                    payload.data_base64.len()
                );
            } else if task_tracker.failed_play_audio_starts.contains(&task_id) {
                debug!(
                    "dropping play_audio chunk after start failure task_id={} bytes(base64)={}",
                    task_id,
                    payload.data_base64.len()
                );
            } else {
                out_tx
                    .send(AgentToGatewayMessage::TaskStatus {
                        task_id,
                        payload: AgentTaskStatusPayload {
                            state: AgentTaskState::Failed,
                            message: Some("chunk received before task.start".to_string()),
                        },
                    })
                    .await
                    .context("agent writer channel closed while reporting out-of-order chunk")?;
            }
        }
        GatewayToAgentMessage::TaskPlayAudioFinish { task_id, .. } => {
            let task = task_tracker.play_audio_tasks.remove(&task_id);
            let finish_after_start_failure = task_tracker.failed_play_audio_starts.remove(&task_id);
            let (state, message) = if let Some(task) = task {
                let chunk_count = task.chunk_count;
                let byte_count = task.byte_count;
                let mut child = task.child;
                let mut stdin = task.stdin;
                let stderr = task.stderr;
                let _ = stdin.shutdown().await;
                drop(stdin);

                match tokio::time::timeout(PLAYBACK_FINISH_TIMEOUT, child.wait()).await {
                    Ok(Ok(status)) if status.success() => {
                        info!(
                            "play_audio finished task_id={} chunks={} bytes={}",
                            task_id, chunk_count, byte_count
                        );
                        (
                            AgentTaskState::Finished,
                            Some(format!(
                                "play_audio finished (chunks={}, bytes={})",
                                chunk_count, byte_count
                            )),
                        )
                    }
                    Ok(Ok(status)) => {
                        let stderr_summary = collect_child_stderr(stderr).await;
                        warn!(
                            "play_audio failed task_id={task_id}: player exited with {status}; stderr={stderr_summary}"
                        );
                        (
                            AgentTaskState::Failed,
                            Some(format!(
                                "local playback process exited with status {status}; stderr: {stderr_summary}"
                            )),
                        )
                    }
                    Ok(Err(error)) => {
                        let stderr_summary = collect_child_stderr(stderr).await;
                        warn!(
                            "play_audio failed task_id={task_id}: wait error: {error}; stderr={stderr_summary}"
                        );
                        (
                            AgentTaskState::Failed,
                            Some(format!(
                                "failed waiting for local playback process: {error}; stderr: {stderr_summary}"
                            )),
                        )
                    }
                    Err(_) => {
                        let _ = child.start_kill();
                        let _ = child.wait().await;
                        let stderr_summary = collect_child_stderr(stderr).await;
                        warn!(
                            "play_audio timed out task_id={task_id} after {:?}; stderr={stderr_summary}",
                            PLAYBACK_FINISH_TIMEOUT
                        );
                        (
                            AgentTaskState::Failed,
                            Some(format!(
                                "local playback timed out after {:?}; stderr: {stderr_summary}",
                                PLAYBACK_FINISH_TIMEOUT
                            )),
                        )
                    }
                }
            } else if finish_after_start_failure {
                debug!("dropping play_audio finish after start failure task_id={task_id}");
                return Ok(());
            } else {
                (
                    AgentTaskState::Failed,
                    Some("finish received before task.start".to_string()),
                )
            };
            out_tx
                .send(AgentToGatewayMessage::TaskStatus {
                    task_id,
                    payload: AgentTaskStatusPayload { state, message },
                })
                .await
                .context("agent writer channel closed while sending task finish status")?;
        }
    }
    Ok(())
}

fn hostname_fallback() -> String {
    std::env::var("HOSTNAME").unwrap_or_else(|_| "unknown-host".to_string())
}

fn load_update_status() -> Option<AgentUpdateStatus> {
    let path = default_update_status_path()?;
    let bytes = fs::read(path).ok()?;
    let state: LocalAutoUpdateState = serde_json::from_slice(&bytes).ok()?;
    Some(AgentUpdateStatus {
        state: state.status,
        current_version: state.current_version,
        target_version: state.target_version,
        checked_at_unix_secs: state.unix_time_secs,
        applied: state.applied,
        restart_performed: state.restart_performed,
        error: state.error,
    })
}

fn default_update_status_path() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from)?;
    let relative = if cfg!(target_os = "macos") {
        PathBuf::from("Library/Application Support/SpeechMesh/device-agent-update.json")
    } else if cfg!(target_os = "linux") {
        PathBuf::from(".local/state/speechmesh/device-agent-update.json")
    } else {
        return None;
    };
    Some(home.join(relative))
}

async fn start_playback_task(format: &Option<AudioFormat>) -> Result<PlaybackTask> {
    let launcher = resolve_playback_launcher();
    let mut command = match launcher {
        PlaybackLauncher::ShellCommand(shell_cmd) => {
            let mut cmd = Command::new("sh");
            cmd.arg("-lc").arg(shell_cmd);
            cmd
        }
        PlaybackLauncher::Ffplay(ffplay_path) => {
            let mut cmd = Command::new(ffplay_path);
            cmd.arg("-nodisp")
                .arg("-autoexit")
                .arg("-loglevel")
                .arg("error");
            if let Some(expected) = ffplay_input_format(format.as_ref())? {
                cmd.arg("-f").arg(expected);
            }
            cmd.arg("-i").arg("pipe:0");
            cmd
        }
        PlaybackLauncher::Mpv(mpv_path) => {
            let mut cmd = Command::new(mpv_path);
            // Keep this mode simple for mobile hosts where ffplay is unavailable.
            cmd.arg("--no-config")
                .arg("--no-terminal")
                .arg("--really-quiet")
                .arg("--no-video")
                .arg("--")
                .arg("-");
            cmd
        }
    };

    command.stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::piped());

    let mut child = command
        .spawn()
        .context(
            "failed to spawn local playback command; set SPEECHMESH_PLAYBACK_CMD or install ffplay/mpv",
        )?;
    let stdin = child
        .stdin
        .take()
        .context("playback process stdin unavailable after spawn")?;
    let stderr = child.stderr.take();
    Ok(PlaybackTask {
        child,
        stdin,
        stderr,
        chunk_count: 0,
        byte_count: 0,
    })
}

async fn collect_child_stderr(stderr: Option<ChildStderr>) -> String {
    let Some(mut stderr) = stderr else {
        return "stderr unavailable".to_string();
    };
    let mut bytes = Vec::new();
    match tokio::time::timeout(
        Duration::from_millis(500),
        tokio::io::AsyncReadExt::read_to_end(&mut stderr, &mut bytes),
    )
    .await
    {
        Ok(Ok(_)) => summarize_stderr(&bytes),
        Ok(Err(error)) => format!("failed to read stderr: {error}"),
        Err(_) => "stderr read timed out".to_string(),
    }
}

fn summarize_stderr(bytes: &[u8]) -> String {
    if bytes.is_empty() {
        return "(empty)".to_string();
    }
    let text = String::from_utf8_lossy(bytes);
    let normalized = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.is_empty() {
        return "(empty)".to_string();
    }
    const MAX_LEN: usize = 240;
    if normalized.len() > MAX_LEN {
        format!("{}...", &normalized[..MAX_LEN])
    } else {
        normalized
    }
}

fn resolve_ffplay_path() -> String {
    if cfg!(target_os = "macos") {
        for candidate in ["/opt/homebrew/bin/ffplay", "/usr/local/bin/ffplay"] {
            if fs::metadata(candidate).is_ok() {
                return candidate.to_string();
            }
        }
    }
    "ffplay".to_string()
}

fn resolve_playback_launcher() -> PlaybackLauncher {
    if let Ok(raw) = std::env::var(PLAYBACK_CMD_ENV) {
        let trimmed = raw.trim();
        if !trimmed.is_empty() {
            return PlaybackLauncher::ShellCommand(trimmed.to_string());
        }
    }
    if let Some(path) = resolve_command_path(&resolve_ffplay_path()) {
        return PlaybackLauncher::Ffplay(path);
    }
    if let Some(path) = resolve_command_path("mpv") {
        return PlaybackLauncher::Mpv(path);
    }
    PlaybackLauncher::Ffplay(resolve_ffplay_path())
}

fn resolve_command_path(command: &str) -> Option<String> {
    let as_path = Path::new(command);
    if as_path.is_absolute() {
        if fs::metadata(as_path).is_ok() {
            return Some(command.to_string());
        }
        return None;
    }
    if command.contains('/') {
        if fs::metadata(as_path).is_ok() {
            return Some(command.to_string());
        }
        return None;
    }
    let path_var = std::env::var_os("PATH")?;
    for base in std::env::split_paths(&path_var) {
        let candidate = base.join(command);
        if fs::metadata(&candidate).is_ok() {
            return Some(candidate.to_string_lossy().to_string());
        }
    }
    None
}

fn ffplay_input_format(format: Option<&AudioFormat>) -> Result<Option<&'static str>> {
    let Some(format) = format else {
        return Ok(None);
    };
    match format.encoding {
        AudioEncoding::Mp3 => Ok(Some("mp3")),
        AudioEncoding::Wav => Ok(Some("wav")),
        AudioEncoding::Flac => Ok(Some("flac")),
        other => anyhow::bail!("unsupported play_audio encoding for local player: {other:?}"),
    }
}

async fn finish_failed_task(
    tracker: &mut TaskTracker,
    out_tx: &mpsc::Sender<AgentToGatewayMessage>,
    task_id: String,
    reason: String,
) -> Result<()> {
    warn!("play_audio failed task_id={task_id}: {reason}");
    if let Some(mut task) = tracker.play_audio_tasks.remove(&task_id) {
        let _ = task.stdin.shutdown().await;
        let _ = task.child.start_kill();
    }
    out_tx
        .send(AgentToGatewayMessage::TaskStatus {
            task_id,
            payload: AgentTaskStatusPayload {
                state: AgentTaskState::Failed,
                message: Some(reason),
            },
        })
        .await
        .context("agent writer channel closed while reporting play_audio failure")
}

async fn abort_all_playback_tasks(tracker: &mut TaskTracker) {
    let task_ids: Vec<String> = tracker.play_audio_tasks.keys().cloned().collect();
    for task_id in task_ids {
        if let Some(mut task) = tracker.play_audio_tasks.remove(&task_id) {
            let _ = task.stdin.shutdown().await;
            let _ = task.child.start_kill();
        }
    }
}
