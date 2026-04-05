use anyhow::{Context, Result, anyhow, bail};
use clap::Parser;
use hound::{SampleFormat, WavReader};
use speechmesh_asr::{RecognitionOptions, StreamRequest};
use speechmesh_core::{AudioFormat, ProviderSelector};
use speechmesh_sdk::{Client, ClientConfig};
use speechmesh_transport::ServerMessage;
use std::path::PathBuf;
use std::time::Duration;
use tokio::time::{Instant, sleep, timeout};

#[derive(Debug, Parser)]
#[command(name = "speechmesh-ws-asr-e2e")]
#[command(about = "End-to-end WebSocket ASR streaming test client for SpeechMesh.")]
struct Args {
    #[arg(long, default_value = "ws://127.0.0.1:8080/ws")]
    url: String,

    #[arg(long)]
    wav: PathBuf,

    #[arg(long, default_value = "en-US")]
    language: String,

    #[arg(long)]
    expected: Option<String>,

    #[arg(long)]
    provider_id: Option<String>,

    #[arg(long, default_value_t = true)]
    interim_results: bool,

    #[arg(long, default_value_t = false)]
    prefer_on_device: bool,

    #[arg(long, default_value_t = false)]
    timestamps: bool,

    #[arg(long, default_value_t = 16000)]
    sample_rate_hz: u32,

    #[arg(long, default_value_t = 1)]
    channels: u16,

    #[arg(long, default_value_t = 100)]
    chunk_ms: u64,

    #[arg(long, default_value_t = 30)]
    timeout_secs: u64,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    let pcm = read_wav_as_pcm_s16le(&args.wav, args.sample_rate_hz, args.channels)?;
    let expected_lc = args.expected.as_ref().map(|s| s.to_lowercase());

    println!(
        "loaded wav={}, pcm_bytes={}, sample_rate={}, channels={}",
        args.wav.display(),
        pcm.len(),
        args.sample_rate_hz,
        args.channels
    );

    let mut client = Client::connect(ClientConfig::new(args.url.clone()))
        .await
        .with_context(|| format!("failed to connect websocket: {}", args.url))?;
    println!("connected url={}", client.url());

    let mut provider = if let Some(provider_id) = &args.provider_id {
        ProviderSelector::provider(provider_id.clone())
    } else {
        ProviderSelector::default()
    };
    provider.required_capabilities = vec!["streaming-input".to_string()];
    if args.provider_id.is_none() {
        provider.preferred_capabilities = vec!["on-device".to_string()];
    }

    let started = client
        .start_asr(StreamRequest {
            provider,
            input_format: AudioFormat::pcm_s16le(args.sample_rate_hz, args.channels),
            options: RecognitionOptions {
                language: Some(args.language.clone()),
                hints: vec![
                    "speechmesh".to_string(),
                    "e2e".to_string(),
                    "streaming".to_string(),
                ],
                interim_results: args.interim_results,
                timestamps: args.timestamps,
                punctuation: true,
                prefer_on_device: args.prefer_on_device,
                provider_options: serde_json::Value::Object(Default::default()),
            },
        })
        .await
        .context("failed to start asr stream")?;
    println!("session started: {:?}", started.session_id);

    let bytes_per_chunk = calc_chunk_bytes(args.sample_rate_hz, args.channels, args.chunk_ms);
    for chunk in pcm.chunks(bytes_per_chunk) {
        client
            .send_audio(chunk)
            .await
            .context("failed to send audio chunk")?;
        sleep(Duration::from_millis(args.chunk_ms)).await;
    }

    client.commit().await.context("failed to send asr.commit")?;
    println!("audio stream committed, waiting for final transcript...");

    let deadline = Instant::now() + Duration::from_secs(args.timeout_secs);
    let final_text = wait_for_final_transcript(&mut client, deadline).await?;
    println!("final transcript: {}", final_text);

    if let Some(expected) = expected_lc {
        if !final_text.to_lowercase().contains(&expected) {
            bail!(
                "assertion failed: transcript does not contain expected text; expected={:?} got={:?}",
                expected,
                final_text
            );
        }
        println!("assertion passed: transcript contains expected text");
    } else {
        println!("no expected text provided; skipped assertion");
    }

    Ok(())
}

async fn wait_for_final_transcript(client: &mut Client, deadline: Instant) -> Result<String> {
    loop {
        let message = next_with_deadline(client, deadline).await?;
        match message {
            ServerMessage::AsrResult { payload, .. } => {
                let delta = payload.delta.as_deref().unwrap_or("");
                println!(
                    "result rev={} final={} speech_final={} delta={:?} text={}",
                    payload.revision, payload.is_final, payload.speech_final, delta, payload.text
                );
                if payload.is_final && payload.speech_final {
                    return Ok(payload.text);
                }
            }
            ServerMessage::SessionEnded { .. } => {
                bail!("session ended before final speech result");
            }
            ServerMessage::Error { payload, .. } => {
                bail!("server returned error: {}", payload.error.message);
            }
            other => {
                println!(
                    "ignoring message while waiting for final asr.result: {:?}",
                    other
                );
            }
        }
    }
}

async fn next_with_deadline(client: &mut Client, deadline: Instant) -> Result<ServerMessage> {
    let now = Instant::now();
    if now >= deadline {
        bail!("timed out waiting for websocket message");
    }
    let remaining = deadline.duration_since(now);
    timeout(remaining, client.recv())
        .await
        .context("timed out waiting for websocket frame")?
        .map_err(|error| anyhow!(error).context("websocket read failed"))
}

fn calc_chunk_bytes(sample_rate_hz: u32, channels: u16, chunk_ms: u64) -> usize {
    let samples_per_channel = sample_rate_hz as usize * chunk_ms as usize / 1000;
    samples_per_channel * channels as usize * 2
}

fn read_wav_as_pcm_s16le(
    path: &PathBuf,
    expected_sample_rate: u32,
    expected_channels: u16,
) -> Result<Vec<u8>> {
    let mut reader = WavReader::open(path)
        .with_context(|| format!("failed to open wav file {}", path.display()))?;
    let spec = reader.spec();

    if spec.channels != expected_channels {
        bail!(
            "wav channel mismatch: expected {}, got {}",
            expected_channels,
            spec.channels
        );
    }
    if spec.sample_rate != expected_sample_rate {
        bail!(
            "wav sample rate mismatch: expected {}, got {}",
            expected_sample_rate,
            spec.sample_rate
        );
    }

    let pcm_bytes = match (spec.sample_format, spec.bits_per_sample) {
        (SampleFormat::Int, 16) => {
            let samples: Result<Vec<i16>, _> = reader.samples::<i16>().collect();
            let samples = samples.context("failed to decode 16-bit wav samples")?;
            let mut bytes = Vec::with_capacity(samples.len() * 2);
            for sample in samples {
                bytes.extend_from_slice(&sample.to_le_bytes());
            }
            bytes
        }
        (SampleFormat::Float, 32) => {
            let samples: Result<Vec<f32>, _> = reader.samples::<f32>().collect();
            let samples = samples.context("failed to decode float wav samples")?;
            let mut bytes = Vec::with_capacity(samples.len() * 2);
            for sample in samples {
                let clamped = sample.clamp(-1.0, 1.0);
                let converted = (clamped * i16::MAX as f32).round() as i16;
                bytes.extend_from_slice(&converted.to_le_bytes());
            }
            bytes
        }
        _ => bail!(
            "unsupported wav format: sample_format={:?}, bits_per_sample={}",
            spec.sample_format,
            spec.bits_per_sample
        ),
    };

    Ok(pcm_bytes)
}
