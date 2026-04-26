use std::ffi::OsString;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, anyhow, bail};
use clap::{Parser, ValueEnum};
use ring::digest::{Context as Sha256Context, SHA256};
use semver::Version;
use serde::{Deserialize, Serialize};

const APP_VERSION: &str = env!("CARGO_PKG_VERSION");
const DEFAULT_CHANNEL: &str = "stable";

fn main() {
    let args: Vec<OsString> = std::env::args_os().collect();
    let result = match args.get(1).and_then(|arg| arg.to_str()) {
        None => {
            print_help();
            Ok(())
        }
        Some("help") | Some("-h") | Some("--help") => {
            print_help();
            Ok(())
        }
        Some("version") => run_version_with_args(
            std::iter::once(OsString::from("speechmesh version")).chain(args.into_iter().skip(2)),
        ),
        Some("check-update") => run_check_update_with_args(
            std::iter::once(OsString::from("speechmesh check-update"))
                .chain(args.into_iter().skip(2)),
        ),
        Some("self-update") => run_self_update_with_args(
            std::iter::once(OsString::from("speechmesh self-update"))
                .chain(args.into_iter().skip(2)),
        ),
        Some("auto-update") => run_auto_update_with_args(
            std::iter::once(OsString::from("speechmesh auto-update"))
                .chain(args.into_iter().skip(2)),
        ),
        Some("agent") if should_run_device_agent(&args) => {
            let forwarded = std::iter::once(OsString::from("speechmesh agent"))
                .chain(args.into_iter().skip(2))
                .collect::<Vec<_>>();
            speechmeshd::device_agent_app::run_with_args(forwarded)
        }
        _ => speechmesh_sdk::cli_app::run_with_args(args),
    };

    if let Err(error) = result {
        eprintln!("error: {error:#}");
        std::process::exit(1);
    }
}

fn print_help() {
    println!(
        "\
SpeechMesh unified binary

Usage:
  speechmesh <command> [options]
  speechmesh agent <command> [options]

Root commands:
  version       Print the unified client version and binary metadata
  check-update  Resolve the latest available update for this platform/channel
  self-update   Download and replace the current `speechmesh` binary
  auto-update   Run continuous update checks and apply updates automatically
  versions      Summarize registered agent versions and update state
  say           Synthesize text and route playback to a device agent
  tts           Text-to-speech tools
  asr           Speech-to-text tools
  devices       List registered agents/devices
  doctor        Run gateway and playback diagnostics
  discover      Inspect providers exposed by the gateway
  agent         Run the long-lived device agent loop

Examples:
  speechmesh version --json
  speechmesh check-update --manifest-url https://example.com/speechmesh.json
  speechmesh self-update --manifest-url https://example.com/speechmesh.json --dry-run
  speechmesh auto-update --manifest-url https://example.com/speechmesh.json --interval-secs 300
  speechmesh say --device mac01 --text \"你好\"
  speechmesh versions --json
  speechmesh agent status --device mac01 --json
  speechmesh agent run --agent-id mac01-speaker-agent --device-id mac01

Notes:
  - Legacy wrapper binaries (`speechmesh-cli`, `speechmesh-agent`) are optional compatibility shims and are only present if installed via `--legacy-compat wrap`.
  - New deployments should call `speechmesh` directly.
  - Use `speechmesh <command> --help`, `speechmesh agent status --help`, or `speechmesh agent run --help` for full flags."
    );
}

fn should_run_device_agent(args: &[OsString]) -> bool {
    args.iter().skip(2).any(|arg| arg == "run")
}

#[derive(Debug, Clone, Serialize)]
struct ClientTarget {
    platform: String,
    arch: String,
}

fn current_target() -> ClientTarget {
    ClientTarget {
        platform: canonical_platform(std::env::consts::OS),
        arch: canonical_arch(std::env::consts::ARCH),
    }
}

fn canonical_platform(value: &str) -> String {
    match value.trim().to_ascii_lowercase().as_str() {
        "darwin" | "macos" | "mac" | "osx" => "macos".to_string(),
        "linux" => "linux".to_string(),
        other => other.to_string(),
    }
}

fn canonical_arch(value: &str) -> String {
    match value.trim().to_ascii_lowercase().as_str() {
        "x86_64" | "amd64" => "x86_64".to_string(),
        "aarch64" | "arm64" => "aarch64".to_string(),
        other => other.to_string(),
    }
}

#[derive(Debug, Parser)]
#[command(name = "speechmesh version")]
struct VersionArgs {
    #[arg(long, help = "Print structured JSON")]
    json: bool,
}

#[derive(Debug, Serialize)]
struct VersionInfo {
    version: String,
    executable: String,
    platform: String,
    channel: String,
}

fn run_version_with_args<I, T>(args: I) -> Result<()>
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    let args = VersionArgs::parse_from(args);
    let target = current_target();
    let info = VersionInfo {
        version: APP_VERSION.to_string(),
        executable: std::env::current_exe()
            .context("failed to resolve current executable")?
            .display()
            .to_string(),
        platform: format!("{}/{}", target.platform, target.arch),
        channel: DEFAULT_CHANNEL.to_string(),
    };
    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&info).context("failed to encode version output")?
        );
    } else {
        println!("speechmesh {}", info.version);
        println!("executable: {}", info.executable);
        println!("platform: {}", info.platform);
        println!("channel: {}", info.channel);
    }
    Ok(())
}

#[derive(Debug, Parser)]
#[command(name = "speechmesh check-update")]
struct CheckUpdateArgs {
    #[arg(long, help = "Manifest URL describing available releases")]
    manifest_url: String,
    #[arg(long, default_value = DEFAULT_CHANNEL, help = "Release channel to resolve")]
    channel: String,
    #[arg(long, help = "Print structured JSON")]
    json: bool,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum UpdateStatus {
    UpdateAvailable,
    UpToDate,
    DowngradeAvailable,
    VersionUnknown,
}

#[derive(Debug, Clone, Serialize)]
struct ResolvedUpdate {
    version: String,
    channel: String,
    url: String,
    sha256: String,
    target: ClientTarget,
    notes_url: Option<String>,
    published_at: Option<String>,
}

#[derive(Debug, Serialize)]
struct CheckUpdateReport {
    current_version: String,
    status: UpdateStatus,
    executable: String,
    release: ResolvedUpdate,
}

fn run_check_update_with_args<I, T>(args: I) -> Result<()>
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    let args = CheckUpdateArgs::parse_from(args);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("failed to initialize tokio runtime")?;
    runtime.block_on(async move {
        let release =
            resolve_manifest_release(&args.manifest_url, &args.channel, &current_target()).await?;
        let report = CheckUpdateReport {
            current_version: APP_VERSION.to_string(),
            status: compare_versions(APP_VERSION, &release.version),
            executable: std::env::current_exe()
                .context("failed to resolve current executable")?
                .display()
                .to_string(),
            release,
        };
        if args.json {
            println!(
                "{}",
                serde_json::to_string_pretty(&report).context("failed to encode update report")?
            );
        } else {
            println!("current: {}", report.current_version);
            println!("status: {}", update_status_label(report.status));
            println!("target: {}", report.release.version);
            println!("channel: {}", report.release.channel);
            println!(
                "asset: {}/{}",
                report.release.target.platform, report.release.target.arch
            );
            println!("url: {}", report.release.url);
            if let Some(notes_url) = &report.release.notes_url {
                println!("notes: {}", notes_url);
            }
        }
        Ok(())
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum RestartMode {
    None,
    Launchd,
    SystemdUser,
}

#[derive(Debug, Parser)]
#[command(name = "speechmesh self-update")]
struct SelfUpdateArgs {
    #[arg(long, help = "Manifest URL describing available releases")]
    manifest_url: Option<String>,
    #[arg(long, default_value = DEFAULT_CHANNEL, help = "Release channel when using --manifest-url")]
    channel: String,
    #[arg(long, help = "Direct asset URL for the new binary")]
    asset_url: Option<String>,
    #[arg(long, help = "Expected SHA-256 for --asset-url downloads")]
    sha256: Option<String>,
    #[arg(long, help = "Optional target version when using --asset-url directly")]
    version: Option<String>,
    #[arg(
        long,
        help = "Download and verify only; do not replace the current binary"
    )]
    dry_run: bool,
    #[arg(
        long,
        help = "Allow replacing even if the target version is unchanged or older"
    )]
    force: bool,
    #[arg(long, value_enum, default_value_t = RestartMode::None, help = "Restart a managed service after replacing the binary")]
    restart_mode: RestartMode,
    #[arg(long, help = "launchd label or systemd --user service name to restart")]
    service_name: Option<String>,
    #[arg(long, hide = true)]
    binary_path: Option<PathBuf>,
}

#[derive(Debug, Parser, Clone)]
#[command(name = "speechmesh auto-update")]
struct AutoUpdateArgs {
    #[arg(long, help = "Manifest URL describing available releases")]
    manifest_url: String,
    #[arg(long, default_value = DEFAULT_CHANNEL, help = "Release channel to resolve")]
    channel: String,
    #[arg(long, default_value_t = 300, help = "Polling interval in seconds")]
    interval_secs: u64,
    #[arg(long, help = "Run one check/apply cycle and exit")]
    once: bool,
    #[arg(long, value_enum, default_value_t = RestartMode::None, help = "Restart a managed service after replacing the binary")]
    restart_mode: RestartMode,
    #[arg(long, help = "launchd label or systemd --user service name to restart")]
    service_name: Option<String>,
    #[arg(long, help = "Persist the latest auto-update status as JSON")]
    status_file: Option<PathBuf>,
    #[arg(long, hide = true)]
    binary_path: Option<PathBuf>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum UpdateManifestEnvelope {
    Rich(UpdateManifest),
    Legacy(LegacyUpdateManifest),
}

#[derive(Debug, Deserialize)]
struct LegacyUpdateManifest {
    version: String,
    #[serde(alias = "asset_url", alias = "download_url")]
    url: String,
    sha256: String,
}

#[derive(Debug, Deserialize)]
struct UpdateManifest {
    #[serde(default)]
    schema: Option<String>,
    #[serde(default)]
    default_channel: Option<String>,
    releases: Vec<ManifestRelease>,
}

#[derive(Debug, Deserialize)]
struct ManifestRelease {
    version: String,
    #[serde(default)]
    channel: Option<String>,
    #[serde(default)]
    notes_url: Option<String>,
    #[serde(default)]
    published_at: Option<String>,
    assets: Vec<ManifestAsset>,
}

#[derive(Debug, Clone, Deserialize)]
struct ManifestAsset {
    platform: String,
    arch: String,
    url: String,
    sha256: String,
    #[serde(default)]
    binary_name: Option<String>,
}

#[derive(Debug)]
struct UpdatePlan {
    release: ResolvedUpdate,
}

#[derive(Debug, Serialize)]
struct RestartPlan {
    requested_mode: String,
    service_name: Option<String>,
    performed: bool,
    hint: Option<String>,
}

#[derive(Debug, Serialize)]
struct SelfUpdateReport {
    current_version: String,
    target_version: String,
    status: UpdateStatus,
    executable: String,
    channel: String,
    url: String,
    sha256: String,
    bytes: usize,
    dry_run: bool,
    applied: bool,
    restart: RestartPlan,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct AutoUpdateCycleReport {
    unix_time_secs: u64,
    event: String,
    status: String,
    current_version: String,
    target_version: Option<String>,
    executable: String,
    manifest_url: String,
    channel: String,
    interval_secs: u64,
    release_url: Option<String>,
    release_sha256: Option<String>,
    bytes: Option<usize>,
    applied: bool,
    restart_performed: bool,
    error: Option<String>,
}

fn run_self_update_with_args<I, T>(args: I) -> Result<()>
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    let args = SelfUpdateArgs::parse_from(args);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("failed to initialize tokio runtime")?;
    runtime.block_on(async move {
        let report = execute_self_update(&args).await?;
        println!(
            "{}",
            serde_json::to_string_pretty(&report).context("failed to encode update output")?
        );
        Ok(())
    })
}

fn run_auto_update_with_args<I, T>(args: I) -> Result<()>
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    let args = AutoUpdateArgs::parse_from(args);
    if !args.once && args.interval_secs == 0 {
        bail!("--interval-secs must be greater than 0 when --once is not set");
    }

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("failed to initialize tokio runtime")?;
    runtime.block_on(run_auto_update_loop(args))
}

async fn run_auto_update_loop(args: AutoUpdateArgs) -> Result<()> {
    loop {
        let report = run_auto_update_cycle(&args).await;
        let report_line =
            serde_json::to_string(&report).context("failed to encode auto-update cycle report")?;
        println!("{report_line}");
        std::io::stdout()
            .flush()
            .context("failed to flush auto-update report to stdout")?;

        if let Some(path) = args.status_file.as_deref() {
            if let Err(error) = write_auto_update_state(path, &report) {
                if args.once {
                    return Err(error);
                }
                eprintln!("warning: failed to write auto-update status file: {error:#}");
            }
        }

        if args.once {
            if let Some(error) = report.error {
                bail!("auto-update --once failed: {error}");
            }
            return Ok(());
        }

        // After replacing the binary, exit so process supervisors can restart from the new binary.
        if report.applied {
            return Ok(());
        }

        tokio::time::sleep(Duration::from_secs(args.interval_secs)).await;
    }
}

async fn run_auto_update_cycle(args: &AutoUpdateArgs) -> AutoUpdateCycleReport {
    let executable = args
        .binary_path
        .clone()
        .unwrap_or_else(|| std::env::current_exe().unwrap_or_else(|_| PathBuf::from("speechmesh")))
        .display()
        .to_string();

    let mut report = AutoUpdateCycleReport {
        unix_time_secs: now_unix_time_secs(),
        event: "check_failed".to_string(),
        status: "error".to_string(),
        current_version: APP_VERSION.to_string(),
        target_version: None,
        executable,
        manifest_url: args.manifest_url.clone(),
        channel: args.channel.clone(),
        interval_secs: args.interval_secs,
        release_url: None,
        release_sha256: None,
        bytes: None,
        applied: false,
        restart_performed: false,
        error: None,
    };

    let target = current_target();
    match resolve_manifest_release(&args.manifest_url, &args.channel, &target).await {
        Ok(release) => {
            let status = compare_versions(APP_VERSION, &release.version);
            report.status = update_status_label(status).to_string();
            report.target_version = Some(release.version.clone());
            report.release_url = Some(release.url.clone());
            report.release_sha256 = Some(release.sha256.clone());

            if status == UpdateStatus::UpdateAvailable {
                let self_update_args = build_auto_update_self_update_args(args);
                match execute_self_update(&self_update_args).await {
                    Ok(update_report) => {
                        report.event = "updated".to_string();
                        report.status = update_status_label(update_report.status).to_string();
                        report.target_version = Some(update_report.target_version);
                        report.release_url = Some(update_report.url);
                        report.release_sha256 = Some(update_report.sha256);
                        report.bytes = Some(update_report.bytes);
                        report.applied = update_report.applied;
                        report.restart_performed = update_report.restart.performed;
                    }
                    Err(error) => {
                        report.event = "update_failed".to_string();
                        report.error = Some(format!("{error:#}"));
                    }
                }
            } else {
                report.event = "checked".to_string();
            }
        }
        Err(error) => {
            report.error = Some(format!("{error:#}"));
        }
    }

    report
}

fn build_auto_update_self_update_args(args: &AutoUpdateArgs) -> SelfUpdateArgs {
    SelfUpdateArgs {
        manifest_url: Some(args.manifest_url.clone()),
        channel: args.channel.clone(),
        asset_url: None,
        sha256: None,
        version: None,
        dry_run: false,
        force: false,
        restart_mode: args.restart_mode,
        service_name: args.service_name.clone(),
        binary_path: args.binary_path.clone(),
    }
}

fn write_auto_update_state(path: &Path, report: &AutoUpdateCycleReport) -> Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent).with_context(|| {
                format!(
                    "failed to create auto-update status directory {}",
                    parent.display()
                )
            })?;
        }
    }

    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or("speechmesh-auto-update-status.json");
    let temp_path = path.with_file_name(format!(".{file_name}.tmp"));
    let body = serde_json::to_vec_pretty(report).context("failed to encode auto-update status")?;

    fs::write(&temp_path, body)
        .with_context(|| format!("failed to write {}", temp_path.display()))?;
    fs::rename(&temp_path, path).with_context(|| {
        format!(
            "failed to move auto-update status {} into place",
            path.display()
        )
    })
}

fn now_unix_time_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_secs())
        .unwrap_or(0)
}

async fn execute_self_update(args: &SelfUpdateArgs) -> Result<SelfUpdateReport> {
    let plan = resolve_update_plan(args).await?;
    let binary_path = args
        .binary_path
        .clone()
        .unwrap_or(std::env::current_exe().context("failed to resolve current executable")?);
    let status = compare_versions(APP_VERSION, &plan.release.version);
    if !args.force {
        match status {
            UpdateStatus::UpdateAvailable => {}
            UpdateStatus::UpToDate => bail!(
                "target version {} matches current version {}; pass --force to replace anyway",
                plan.release.version,
                APP_VERSION
            ),
            UpdateStatus::DowngradeAvailable => bail!(
                "target version {} is older than current version {}; pass --force to replace anyway",
                plan.release.version,
                APP_VERSION
            ),
            UpdateStatus::VersionUnknown => bail!(
                "unable to compare current version {} with target {}; pass --force to replace anyway",
                APP_VERSION,
                plan.release.version
            ),
        }
    }

    let bytes = download_binary(&plan.release.url).await?;
    let actual_sha = sha256_hex(&bytes);
    if !actual_sha.eq_ignore_ascii_case(plan.release.sha256.trim()) {
        bail!(
            "sha256 mismatch: expected {}, got {}",
            plan.release.sha256,
            actual_sha
        );
    }

    let mut restart = RestartPlan {
        requested_mode: restart_mode_label(args.restart_mode).to_string(),
        service_name: args.service_name.clone(),
        performed: false,
        hint: restart_hint(args.restart_mode, args.service_name.as_deref()),
    };

    if !args.dry_run {
        replace_binary(&binary_path, &bytes)?;
        if args.restart_mode != RestartMode::None {
            restart_managed_service(args.restart_mode, args.service_name.as_deref())?;
            restart.performed = true;
        }
    }

    Ok(SelfUpdateReport {
        current_version: APP_VERSION.to_string(),
        target_version: plan.release.version.clone(),
        status,
        executable: binary_path.display().to_string(),
        channel: plan.release.channel.clone(),
        url: plan.release.url.clone(),
        sha256: actual_sha,
        bytes: bytes.len(),
        dry_run: args.dry_run,
        applied: !args.dry_run,
        restart,
    })
}

async fn resolve_update_plan(args: &SelfUpdateArgs) -> Result<UpdatePlan> {
    let release = match (&args.manifest_url, &args.asset_url) {
        (Some(manifest_url), None) => {
            resolve_manifest_release(manifest_url, &args.channel, &current_target()).await?
        }
        (None, Some(asset_url)) => {
            let sha256 = args
                .sha256
                .clone()
                .ok_or_else(|| anyhow!("--sha256 is required with --asset-url"))?;
            ResolvedUpdate {
                version: args
                    .version
                    .clone()
                    .unwrap_or_else(|| "unknown".to_string()),
                channel: args.channel.clone(),
                url: asset_url.clone(),
                sha256,
                target: current_target(),
                notes_url: None,
                published_at: None,
            }
        }
        (Some(_), Some(_)) => bail!("use either --manifest-url or --asset-url, not both"),
        (None, None) => bail!("either --manifest-url or --asset-url is required"),
    };
    Ok(UpdatePlan { release })
}

async fn resolve_manifest_release(
    url: &str,
    requested_channel: &str,
    target: &ClientTarget,
) -> Result<ResolvedUpdate> {
    let envelope = fetch_manifest(url).await?;
    match envelope {
        UpdateManifestEnvelope::Legacy(manifest) => Ok(ResolvedUpdate {
            version: manifest.version,
            channel: requested_channel.to_string(),
            url: manifest.url,
            sha256: manifest.sha256,
            target: target.clone(),
            notes_url: None,
            published_at: None,
        }),
        UpdateManifestEnvelope::Rich(manifest) => {
            select_manifest_release(manifest, requested_channel, target)
        }
    }
}

fn select_manifest_release(
    manifest: UpdateManifest,
    requested_channel: &str,
    target: &ClientTarget,
) -> Result<ResolvedUpdate> {
    if let Some(schema) = manifest.schema.as_deref() {
        if !schema.is_empty() && schema != "speechmesh/update-manifest.v1" {
            bail!("unsupported update manifest schema: {schema}");
        }
    }

    let channel = if requested_channel.trim().is_empty() {
        manifest
            .default_channel
            .clone()
            .unwrap_or_else(|| DEFAULT_CHANNEL.to_string())
    } else {
        requested_channel.trim().to_string()
    };
    let normalized_channel = channel.to_ascii_lowercase();

    let mut candidates = manifest
        .releases
        .into_iter()
        .filter_map(|release| {
            let release_channel = release.channel.clone().unwrap_or_else(|| {
                manifest
                    .default_channel
                    .clone()
                    .unwrap_or_else(|| DEFAULT_CHANNEL.to_string())
            });
            if release_channel.to_ascii_lowercase() != normalized_channel {
                return None;
            }
            let asset = release
                .assets
                .iter()
                .find(|asset| asset_matches_target(asset, target))
                .cloned()?;
            Some((release, release_channel, asset))
        })
        .collect::<Vec<_>>();

    candidates.sort_by(|(left_release, _, _), (right_release, _, _)| {
        compare_manifest_versions(&left_release.version, &right_release.version).reverse()
    });

    let (release, release_channel, asset) = candidates.into_iter().next().ok_or_else(|| {
        anyhow!(
            "no matching release found for channel {} and platform {}/{}",
            channel,
            target.platform,
            target.arch
        )
    })?;

    if let Some(binary_name) = asset.binary_name.as_deref() {
        if binary_name != "speechmesh" {
            bail!(
                "manifest asset binary_name {} does not match expected speechmesh",
                binary_name
            );
        }
    }

    Ok(ResolvedUpdate {
        version: release.version,
        channel: release_channel,
        url: asset.url,
        sha256: asset.sha256,
        target: ClientTarget {
            platform: canonical_platform(&asset.platform),
            arch: canonical_arch(&asset.arch),
        },
        notes_url: release.notes_url,
        published_at: release.published_at,
    })
}

fn asset_matches_target(asset: &ManifestAsset, target: &ClientTarget) -> bool {
    canonical_platform(&asset.platform) == target.platform
        && canonical_arch(&asset.arch) == target.arch
}

fn compare_manifest_versions(left: &str, right: &str) -> std::cmp::Ordering {
    match (parse_semver(left), parse_semver(right)) {
        (Some(left), Some(right)) => left.cmp(&right),
        _ => left.cmp(right),
    }
}

async fn fetch_manifest(url: &str) -> Result<UpdateManifestEnvelope> {
    let response = reqwest::get(url)
        .await
        .with_context(|| format!("failed to fetch manifest {url}"))?;
    let response = response
        .error_for_status()
        .with_context(|| format!("manifest request failed for {url}"))?;
    response
        .json::<UpdateManifestEnvelope>()
        .await
        .with_context(|| format!("failed to decode manifest {url}"))
}

async fn download_binary(url: &str) -> Result<Vec<u8>> {
    let response = reqwest::get(url)
        .await
        .with_context(|| format!("failed to download asset {url}"))?;
    let response = response
        .error_for_status()
        .with_context(|| format!("asset request failed for {url}"))?;
    response
        .bytes()
        .await
        .with_context(|| format!("failed to read asset body {url}"))
        .map(|bytes| bytes.to_vec())
}

fn replace_binary(target: &Path, bytes: &[u8]) -> Result<()> {
    let parent = target.parent().ok_or_else(|| {
        anyhow!(
            "target binary has no parent directory: {}",
            target.display()
        )
    })?;
    let file_name = target
        .file_name()
        .ok_or_else(|| anyhow!("target binary has no file name: {}", target.display()))?
        .to_string_lossy()
        .to_string();
    let temp_path = parent.join(format!(".{file_name}.update"));
    let backup_path = parent.join(format!(".{file_name}.prev"));

    fs::write(&temp_path, bytes)
        .with_context(|| format!("failed to write temp update {}", temp_path.display()))?;
    copy_permissions(target, &temp_path)?;

    if backup_path.exists() {
        fs::remove_file(&backup_path).with_context(|| {
            format!(
                "failed to remove old backup binary {}",
                backup_path.display()
            )
        })?;
    }
    fs::rename(target, &backup_path)
        .with_context(|| format!("failed to back up current binary {}", target.display()))?;
    if let Err(error) = fs::rename(&temp_path, target) {
        let _ = fs::rename(&backup_path, target);
        bail!("failed to replace binary {}: {error}", target.display());
    }
    Ok(())
}

fn copy_permissions(source: &Path, destination: &Path) -> Result<()> {
    let permissions = fs::metadata(source)
        .with_context(|| format!("failed to read permissions for {}", source.display()))?
        .permissions();
    fs::set_permissions(destination, permissions)
        .with_context(|| format!("failed to apply permissions to {}", destination.display()))
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256Context::new(&SHA256);
    hasher.update(bytes);
    let digest = hasher.finish();
    digest
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn parse_semver(value: &str) -> Option<Version> {
    Version::parse(value.trim_start_matches('v')).ok()
}

fn compare_versions(current: &str, target: &str) -> UpdateStatus {
    match (parse_semver(current), parse_semver(target)) {
        (Some(current), Some(target)) => {
            if target > current {
                UpdateStatus::UpdateAvailable
            } else if target == current {
                UpdateStatus::UpToDate
            } else {
                UpdateStatus::DowngradeAvailable
            }
        }
        _ if current == target => UpdateStatus::UpToDate,
        _ => UpdateStatus::VersionUnknown,
    }
}

fn update_status_label(status: UpdateStatus) -> &'static str {
    match status {
        UpdateStatus::UpdateAvailable => "update_available",
        UpdateStatus::UpToDate => "up_to_date",
        UpdateStatus::DowngradeAvailable => "downgrade_available",
        UpdateStatus::VersionUnknown => "version_unknown",
    }
}

fn restart_mode_label(mode: RestartMode) -> &'static str {
    match mode {
        RestartMode::None => "none",
        RestartMode::Launchd => "launchd",
        RestartMode::SystemdUser => "systemd-user",
    }
}

fn restart_hint(mode: RestartMode, service_name: Option<&str>) -> Option<String> {
    match mode {
        RestartMode::None => match std::env::consts::OS {
            "macos" => Some(
                "If this binary is managed by launchd, run `launchctl kickstart -k gui/$(id -u)/<label>` after updating.".to_string(),
            ),
            "linux" => Some(
                "If this binary is managed by systemd --user, run `systemctl --user restart <service>` after updating.".to_string(),
            ),
            _ => None,
        },
        RestartMode::Launchd => Some(format!(
            "Service restart command: launchctl kickstart -k gui/$(id -u)/{}",
            service_name.unwrap_or("<label>")
        )),
        RestartMode::SystemdUser => Some(format!(
            "Service restart command: systemctl --user restart {}",
            service_name.unwrap_or("<service>")
        )),
    }
}

fn restart_managed_service(mode: RestartMode, service_name: Option<&str>) -> Result<()> {
    match mode {
        RestartMode::None => Ok(()),
        RestartMode::Launchd => {
            let label = service_name
                .ok_or_else(|| anyhow!("--service-name is required with --restart-mode launchd"))?;
            let uid = String::from_utf8(
                Command::new("id")
                    .arg("-u")
                    .output()
                    .context("failed to determine current uid")?
                    .stdout,
            )
            .context("launchd uid output was not utf-8")?;
            let domain = format!("gui/{}/{}", uid.trim(), label);
            let status = Command::new("launchctl")
                .args(["kickstart", "-k", &domain])
                .status()
                .with_context(|| format!("failed to restart launchd service {label}"))?;
            if !status.success() {
                bail!("launchd restart failed for {label} with status {status}");
            }
            Ok(())
        }
        RestartMode::SystemdUser => {
            let service = service_name.ok_or_else(|| {
                anyhow!("--service-name is required with --restart-mode systemd-user")
            })?;
            let status = Command::new("systemctl")
                .args(["--user", "restart", service])
                .status()
                .with_context(|| format!("failed to restart systemd --user service {service}"))?;
            if !status.success() {
                bail!("systemd --user restart failed for {service} with status {status}");
            }
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir() -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let path = std::env::temp_dir().join(format!("speechmesh-app-test-{unique}"));
        fs::create_dir_all(&path).expect("create temp dir");
        path
    }

    #[test]
    fn sha256_matches_known_value() {
        assert_eq!(
            sha256_hex(b"speechmesh"),
            "5d8a0c8c129bdf46847757d988ec8ba079d340b39258a262641bc70167dccffb"
        );
    }

    #[test]
    fn replace_binary_swaps_in_new_bytes_and_keeps_backup() {
        let dir = temp_dir();
        let binary = dir.join("speechmesh");
        fs::write(&binary, b"old-binary").expect("write old binary");
        replace_binary(&binary, b"new-binary").expect("replace binary");

        let backup = dir.join(".speechmesh.prev");
        assert_eq!(fs::read(&binary).expect("read updated"), b"new-binary");
        assert_eq!(fs::read(&backup).expect("read backup"), b"old-binary");

        fs::remove_dir_all(dir).expect("cleanup");
    }

    #[tokio::test]
    async fn direct_asset_requires_sha() {
        let args = SelfUpdateArgs {
            manifest_url: None,
            channel: DEFAULT_CHANNEL.to_string(),
            asset_url: Some("https://example.com/speechmesh".to_string()),
            sha256: None,
            version: None,
            dry_run: true,
            force: false,
            restart_mode: RestartMode::None,
            service_name: None,
            binary_path: None,
        };
        let error = resolve_update_plan(&args).await.expect_err("missing sha");
        assert!(error.to_string().contains("--sha256"));
    }

    #[test]
    fn rich_manifest_prefers_matching_channel_and_platform() {
        let manifest = UpdateManifest {
            schema: Some("speechmesh/update-manifest.v1".to_string()),
            default_channel: Some(DEFAULT_CHANNEL.to_string()),
            releases: vec![
                ManifestRelease {
                    version: "0.2.0".to_string(),
                    channel: Some("beta".to_string()),
                    notes_url: None,
                    published_at: None,
                    assets: vec![ManifestAsset {
                        platform: "macos".to_string(),
                        arch: "arm64".to_string(),
                        url: "https://example.com/beta-arm64".to_string(),
                        sha256: "abc".to_string(),
                        binary_name: Some("speechmesh".to_string()),
                    }],
                },
                ManifestRelease {
                    version: "0.1.2".to_string(),
                    channel: Some(DEFAULT_CHANNEL.to_string()),
                    notes_url: Some("https://example.com/notes".to_string()),
                    published_at: Some("2026-04-11T00:00:00Z".to_string()),
                    assets: vec![ManifestAsset {
                        platform: "darwin".to_string(),
                        arch: "arm64".to_string(),
                        url: "https://example.com/stable-arm64".to_string(),
                        sha256: "def".to_string(),
                        binary_name: Some("speechmesh".to_string()),
                    }],
                },
            ],
        };
        let target = ClientTarget {
            platform: "macos".to_string(),
            arch: "aarch64".to_string(),
        };
        let release =
            select_manifest_release(manifest, DEFAULT_CHANNEL, &target).expect("select release");
        assert_eq!(release.version, "0.1.2");
        assert_eq!(release.channel, DEFAULT_CHANNEL);
        assert_eq!(release.url, "https://example.com/stable-arm64");
        assert_eq!(release.target.platform, "macos");
        assert_eq!(release.target.arch, "aarch64");
    }

    #[test]
    fn compare_versions_reports_update_states() {
        assert_eq!(
            compare_versions("0.1.0", "0.2.0"),
            UpdateStatus::UpdateAvailable
        );
        assert_eq!(compare_versions("0.1.0", "0.1.0"), UpdateStatus::UpToDate);
        assert_eq!(
            compare_versions("0.2.0", "0.1.0"),
            UpdateStatus::DowngradeAvailable
        );
        assert_eq!(
            compare_versions("dev", "nightly"),
            UpdateStatus::VersionUnknown
        );
    }

    #[test]
    fn auto_update_builds_self_update_args_from_manifest_config() {
        let args = AutoUpdateArgs {
            manifest_url: "https://updates.example.com/manifest.json".to_string(),
            channel: "stable".to_string(),
            interval_secs: 120,
            once: false,
            restart_mode: RestartMode::SystemdUser,
            service_name: Some("speechmesh-device-agent.service".to_string()),
            status_file: Some(PathBuf::from("/tmp/speechmesh-auto-update.json")),
            binary_path: Some(PathBuf::from("/tmp/speechmesh")),
        };
        let mapped = build_auto_update_self_update_args(&args);
        assert_eq!(
            mapped.manifest_url.as_deref(),
            Some("https://updates.example.com/manifest.json")
        );
        assert_eq!(mapped.channel, "stable");
        assert_eq!(mapped.restart_mode, RestartMode::SystemdUser);
        assert_eq!(
            mapped.service_name.as_deref(),
            Some("speechmesh-device-agent.service")
        );
        assert_eq!(mapped.binary_path, Some(PathBuf::from("/tmp/speechmesh")));
        assert!(!mapped.dry_run);
        assert!(!mapped.force);
    }

    #[test]
    fn auto_update_status_file_is_written_as_json() {
        let dir = temp_dir();
        let status_file = dir.join("state").join("auto-update.json");
        let report = AutoUpdateCycleReport {
            unix_time_secs: 42,
            event: "checked".to_string(),
            status: "up_to_date".to_string(),
            current_version: "0.1.0".to_string(),
            target_version: Some("0.1.0".to_string()),
            executable: "/tmp/speechmesh".to_string(),
            manifest_url: "https://updates.example.com/manifest.json".to_string(),
            channel: "stable".to_string(),
            interval_secs: 300,
            release_url: Some("https://updates.example.com/speechmesh".to_string()),
            release_sha256: Some("abc123".to_string()),
            bytes: None,
            applied: false,
            restart_performed: false,
            error: None,
        };

        write_auto_update_state(&status_file, &report).expect("write status file");
        let payload = fs::read_to_string(&status_file).expect("read status file");
        let decoded: AutoUpdateCycleReport =
            serde_json::from_str(&payload).expect("decode status report");
        assert_eq!(decoded.unix_time_secs, 42);
        assert_eq!(decoded.event, "checked");
        assert_eq!(decoded.status, "up_to_date");
        assert_eq!(decoded.current_version, "0.1.0");
        assert_eq!(decoded.target_version.as_deref(), Some("0.1.0"));
        assert_eq!(decoded.interval_secs, 300);
        assert_eq!(
            decoded.release_url.as_deref(),
            Some("https://updates.example.com/speechmesh")
        );

        fs::remove_dir_all(dir).expect("cleanup");
    }
}
