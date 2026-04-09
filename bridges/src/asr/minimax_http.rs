use std::time::Duration;

use async_trait::async_trait;
use reqwest::Client;
use serde::Deserialize;
use serde_json::Value;
use speechmesh_asr::{AsrSession, StreamRequest, Transcript, TranscriptSegment};
use speechmesh_core::{
    Capability, CapabilityDomain, ProviderDescriptor, RuntimeMode, SessionId, StreamMode,
};
use tokio::sync::mpsc;

use super::{
    AsrBridge, BridgeAsrEvent, BridgeAsrSessionHandle, BridgeCommand,
    asr_descriptor_with_io_modes, audio_mime_type, build_multipart_form_body,
    default_audio_filename, encode_minimax_upload_audio, filter_reserved_provider_options,
    provider_option_to_form_value, requested_asr_input_mode, requested_asr_output_mode,
    seconds_to_ms, streaming_partial_trigger_bytes,
};
use crate::BridgeError;

#[derive(Debug, Clone)]
pub struct MiniMaxHttpAsrBridgeConfig {
    pub provider_id: String,
    pub display_name: Option<String>,
    pub base_url: String,
    pub api_key: String,
    pub default_model: String,
    pub request_timeout: Duration,
    pub streaming_partial_min_bytes: usize,
}

pub struct MiniMaxHttpAsrBridge {
    config: MiniMaxHttpAsrBridgeConfig,
    client: Client,
}

impl MiniMaxHttpAsrBridge {
    pub fn new(config: MiniMaxHttpAsrBridgeConfig) -> Result<Self, BridgeError> {
        let client = Client::builder()
            .timeout(config.request_timeout)
            .build()
            .map_err(|error| {
                BridgeError::Unavailable(format!("failed to build MiniMax ASR client: {error}"))
            })?;
        Ok(Self { config, client })
    }
}

#[async_trait]
impl AsrBridge for MiniMaxHttpAsrBridge {
    fn descriptors(&self) -> Vec<ProviderDescriptor> {
        vec![asr_descriptor_with_io_modes(
            ProviderDescriptor::new(
                self.config.provider_id.clone(),
                self.config
                    .display_name
                    .clone()
                    .unwrap_or_else(|| "MiniMax ASR".to_string()),
                CapabilityDomain::Asr,
                RuntimeMode::RemoteGateway,
            )
            .with_capability(Capability::enabled("cloud-provider")),
            false,
        )]
    }

    async fn start_stream(
        &self,
        request: StreamRequest,
    ) -> Result<BridgeAsrSessionHandle, BridgeError> {
        let output_mode = requested_asr_output_mode(&request);
        let accepted_input_format = request.input_format.clone();
        let session = AsrSession {
            id: SessionId::new(),
            provider_id: self.config.provider_id.clone(),
            accepted_input_format,
            input_mode: requested_asr_input_mode(&request),
            output_mode,
        };

        let config = self.config.clone();
        let client = self.client.clone();
        let (command_tx, mut command_rx) = mpsc::channel::<BridgeCommand>(64);
        let (event_tx, event_rx) = mpsc::channel::<BridgeAsrEvent>(64);

        tokio::spawn(async move {
            let mut buffered_audio = Vec::new();
            let mut last_partial_bytes = 0_usize;
            let mut last_partial_text: Option<String> = None;
            let partial_min_bytes = streaming_partial_trigger_bytes(
                &request.input_format,
                config.streaming_partial_min_bytes,
            );
            while let Some(command) = command_rx.recv().await {
                match command {
                    BridgeCommand::PushAudio(chunk) => {
                        buffered_audio.extend_from_slice(&chunk);
                        if matches!(output_mode, StreamMode::Streaming)
                            && buffered_audio.len().saturating_sub(last_partial_bytes)
                                >= partial_min_bytes
                        {
                            match transcribe_minimax(&client, &config, &request, &buffered_audio)
                                .await
                            {
                                Ok(transcript) => {
                                    let text = transcript.text.trim().to_string();
                                    if !text.is_empty()
                                        && last_partial_text.as_deref() != Some(text.as_str())
                                    {
                                        last_partial_bytes = buffered_audio.len();
                                        last_partial_text = Some(text.clone());
                                        if event_tx
                                            .send(BridgeAsrEvent::Partial { text })
                                            .await
                                            .is_err()
                                        {
                                            return;
                                        }
                                    }
                                }
                                Err(error) => {
                                    let _ = event_tx
                                        .send(BridgeAsrEvent::Error {
                                            message: error.to_string(),
                                        })
                                        .await;
                                    return;
                                }
                            }
                        }
                    }
                    BridgeCommand::Commit => {
                        match transcribe_minimax(&client, &config, &request, &buffered_audio).await
                        {
                            Ok(transcript) => {
                                if event_tx
                                    .send(BridgeAsrEvent::Final { transcript })
                                    .await
                                    .is_err()
                                {
                                    return;
                                }
                                let _ =
                                    event_tx.send(BridgeAsrEvent::Ended { reason: None }).await;
                            }
                            Err(error) => {
                                let _ = event_tx
                                    .send(BridgeAsrEvent::Error {
                                        message: error.to_string(),
                                    })
                                    .await;
                            }
                        }
                        return;
                    }
                    BridgeCommand::Stop => {
                        let _ = event_tx
                            .send(BridgeAsrEvent::Ended {
                                reason: Some("stopped".to_string()),
                            })
                            .await;
                        return;
                    }
                }
            }
        });

        Ok(BridgeAsrSessionHandle::new(session, command_tx, event_rx))
    }
}

// ---- MiniMax ASR 响应类型 ----

#[derive(Debug, Deserialize)]
struct MiniMaxAsrResponse {
    text: Option<String>,
    language: Option<String>,
    segments: Option<Vec<MiniMaxAsrSegment>>,
    words: Option<Vec<MiniMaxAsrWord>>,
    error: Option<MiniMaxAsrError>,
}

#[derive(Debug, Deserialize)]
struct MiniMaxAsrSegment {
    text: Option<String>,
    start: Option<f64>,
    end: Option<f64>,
}

#[derive(Debug, Deserialize)]
struct MiniMaxAsrWord {
    word: Option<String>,
    text: Option<String>,
    start: Option<f64>,
    end: Option<f64>,
}

#[derive(Debug, Deserialize)]
struct MiniMaxAsrError {
    message: Option<String>,
}

// ---- MiniMax ASR 转写逻辑 ----

async fn transcribe_minimax(
    client: &Client,
    config: &MiniMaxHttpAsrBridgeConfig,
    request: &StreamRequest,
    buffered_audio: &[u8],
) -> Result<Transcript, BridgeError> {
    if buffered_audio.is_empty() {
        return Err(BridgeError::Protocol(
            "MiniMax ASR commit received without any buffered audio".to_string(),
        ));
    }

    let model = request
        .options
        .provider_options
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or(&config.default_model);
    let response_format = request
        .options
        .provider_options
        .get("response_format")
        .and_then(Value::as_str)
        .unwrap_or("verbose_json");
    let filename = request
        .options
        .provider_options
        .get("filename")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| default_audio_filename(&request.input_format).to_string());
    let mime_type = audio_mime_type(&request.input_format)?;
    let file_bytes = encode_minimax_upload_audio(&request.input_format, buffered_audio)?;
    let url = format!(
        "{}/audio/transcriptions",
        config.base_url.trim_end_matches('/')
    );

    let mut form_fields = vec![
        ("model".to_string(), model.to_string()),
        ("response_format".to_string(), response_format.to_string()),
    ];
    if let Some(language) = request.options.language.as_deref() {
        form_fields.push(("language".to_string(), language.to_string()));
    }
    if request.options.timestamps {
        form_fields.push(("timestamp_granularities[]".to_string(), "word".to_string()));
    }

    let extra = filter_reserved_provider_options(
        &request.options.provider_options,
        &["model", "response_format", "filename"],
    );
    if let Value::Object(entries) = extra {
        for (key, value) in entries {
            form_fields.push((key, provider_option_to_form_value(value)));
        }
    }
    let boundary = format!(
        "speechmesh-minimax-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    );
    let body =
        build_multipart_form_body(&boundary, &form_fields, &filename, mime_type, &file_bytes);

    let response = client
        .post(&url)
        .bearer_auth(&config.api_key)
        .header(
            "Content-Type",
            format!("multipart/form-data; boundary={boundary}"),
        )
        .body(body)
        .send()
        .await
        .map_err(|error| {
            BridgeError::Unavailable(format!(
                "failed to call MiniMax ASR endpoint {url}: {error}"
            ))
        })?;
    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(BridgeError::Unavailable(format!(
            "MiniMax ASR endpoint {url} returned status {status}: {body}"
        )));
    }

    let payload = response
        .json::<MiniMaxAsrResponse>()
        .await
        .map_err(|error| {
            BridgeError::Protocol(format!("failed to decode MiniMax ASR response: {error}"))
        })?;
    if let Some(error) = payload.error.and_then(|value| value.message) {
        return Err(BridgeError::Unavailable(format!(
            "MiniMax ASR failed: {error}"
        )));
    }

    let text = payload
        .text
        .clone()
        .or_else(|| {
            payload.segments.as_ref().map(|segments| {
                segments
                    .iter()
                    .filter_map(|segment| segment.text.as_deref())
                    .collect::<Vec<_>>()
                    .join("")
            })
        })
        .filter(|text| !text.trim().is_empty())
        .ok_or_else(|| BridgeError::Protocol("MiniMax ASR response missing text".to_string()))?;

    let mut segments = payload
        .words
        .unwrap_or_default()
        .into_iter()
        .filter_map(|word| {
            let text = word.word.or(word.text)?;
            Some(TranscriptSegment {
                text,
                is_final: true,
                start_ms: word.start.map(seconds_to_ms),
                end_ms: word.end.map(seconds_to_ms),
            })
        })
        .collect::<Vec<_>>();
    if segments.is_empty() {
        segments = payload
            .segments
            .unwrap_or_default()
            .into_iter()
            .filter_map(|segment| {
                let text = segment.text?;
                Some(TranscriptSegment {
                    text,
                    is_final: true,
                    start_ms: segment.start.map(seconds_to_ms),
                    end_ms: segment.end.map(seconds_to_ms),
                })
            })
            .collect();
    }

    Ok(Transcript {
        text,
        language: payload.language,
        segments,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use speechmesh_core::{AudioFormat, ProviderSelectionMode};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    fn request_with_selector(
        mode: ProviderSelectionMode,
        provider_id: Option<&str>,
    ) -> StreamRequest {
        StreamRequest {
            provider: speechmesh_core::ProviderSelector {
                mode,
                provider_id: provider_id.map(ToString::to_string),
                required_capabilities: Vec::new(),
                preferred_capabilities: Vec::new(),
            },
            input_format: AudioFormat::pcm_s16le(16_000, 1),
            options: speechmesh_asr::RecognitionOptions {
                provider_options: serde_json::Value::Null,
                ..speechmesh_asr::RecognitionOptions::default()
            },
        }
    }

    #[tokio::test]
    async fn minimax_http_asr_bridge_commits_buffered_audio() {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let address = listener.local_addr().expect("addr");
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.expect("accept");
            let mut buffer = Vec::new();
            let mut header_end = None;
            loop {
                let mut chunk = [0_u8; 4096];
                let read = socket.read(&mut chunk).await.expect("read");
                if read == 0 {
                    break;
                }
                buffer.extend_from_slice(&chunk[..read]);
                if header_end.is_none() {
                    if let Some(index) = buffer.windows(4).position(|window| window == b"\r\n\r\n")
                    {
                        header_end = Some(index + 4);
                    }
                }
                if let Some(header_end) = header_end {
                    let header_text = String::from_utf8_lossy(&buffer[..header_end]);
                    let content_length = header_text
                        .lines()
                        .find_map(|line| {
                            let (name, value) = line.split_once(':')?;
                            name.eq_ignore_ascii_case("content-length")
                                .then(|| value.trim().parse::<usize>().ok())
                                .flatten()
                        })
                        .expect("content-length");
                    if buffer.len() >= header_end + content_length {
                        let body = String::from_utf8_lossy(
                            &buffer[header_end..header_end + content_length],
                        );
                        assert!(header_text.starts_with("POST /audio/transcriptions HTTP/1.1"));
                        assert!(
                            header_text
                                .to_ascii_lowercase()
                                .contains("authorization: bearer test-key")
                        );
                        assert!(body.contains("name=\"model\""));
                        assert!(body.contains("speech-01-turbo"));
                        assert!(body.contains("name=\"language\""));
                        assert!(body.contains("zh-CN"));
                        assert!(body.contains("name=\"file\"; filename=\"audio.wav\""));
                        assert!(body.contains("RIFF"));
                        let response_body = r#"{"text":"hello world","language":"zh","words":[{"word":"hello","start":0.0,"end":0.4},{"word":"world","start":0.4,"end":0.8}]}"#;
                        let response = format!(
                            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                            response_body.len(),
                            response_body
                        );
                        socket.write_all(response.as_bytes()).await.expect("write");
                        return;
                    }
                }
            }
            panic!("server did not receive full request");
        });

        let bridge = MiniMaxHttpAsrBridge::new(MiniMaxHttpAsrBridgeConfig {
            provider_id: "minimax.asr".to_string(),
            display_name: Some("MiniMax ASR".to_string()),
            base_url: format!("http://{}", address),
            api_key: "test-key".to_string(),
            default_model: "speech-01-turbo".to_string(),
            request_timeout: Duration::from_secs(5),
            streaming_partial_min_bytes: 4,
        })
        .expect("bridge");

        let mut request =
            request_with_selector(ProviderSelectionMode::Provider, Some("minimax.asr"));
        request.options.language = Some("zh-CN".to_string());
        request.options.provider_options = serde_json::json!({ "output_mode": "buffered" });

        let mut session = bridge.start_stream(request).await.expect("start");
        session
            .push_audio(vec![0x01, 0x00, 0x02, 0x00])
            .await
            .expect("push");
        session.commit().await.expect("commit");
        let mut events = session.take_event_rx().expect("event rx");

        match events.recv().await.expect("first event") {
            BridgeAsrEvent::Final { transcript } => {
                assert_eq!(transcript.text, "hello world");
                assert_eq!(transcript.language.as_deref(), Some("zh"));
                assert_eq!(transcript.segments.len(), 2);
                assert_eq!(transcript.segments[0].start_ms, Some(0));
                assert_eq!(transcript.segments[1].end_ms, Some(800));
            }
            other => panic!("unexpected event: {other:?}"),
        }
        match events.recv().await.expect("second event") {
            BridgeAsrEvent::Ended { reason } => assert_eq!(reason, None),
            other => panic!("unexpected event: {other:?}"),
        }

        server.await.expect("server");
    }

    #[tokio::test]
    async fn minimax_http_asr_bridge_emits_streaming_partial_before_commit() {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let address = listener.local_addr().expect("addr");
        let server = tokio::spawn(async move {
            for response_body in [
                r#"{"text":"hello","language":"zh"}"#,
                r#"{"text":"hello world","language":"zh"}"#,
            ] {
                let (mut socket, _) = listener.accept().await.expect("accept");
                let mut buffer = Vec::new();
                let mut header_end = None;
                loop {
                    let mut chunk = [0_u8; 4096];
                    let read = socket.read(&mut chunk).await.expect("read");
                    if read == 0 {
                        break;
                    }
                    buffer.extend_from_slice(&chunk[..read]);
                    if header_end.is_none() {
                        if let Some(index) =
                            buffer.windows(4).position(|window| window == b"\r\n\r\n")
                        {
                            header_end = Some(index + 4);
                        }
                    }
                    if let Some(header_end) = header_end {
                        let header_text = String::from_utf8_lossy(&buffer[..header_end]);
                        let content_length = header_text
                            .lines()
                            .find_map(|line| {
                                let (name, value) = line.split_once(':')?;
                                name.eq_ignore_ascii_case("content-length")
                                    .then(|| value.trim().parse::<usize>().ok())
                                    .flatten()
                            })
                            .expect("content-length");
                        if buffer.len() >= header_end + content_length {
                            let response = format!(
                                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                                response_body.len(),
                                response_body
                            );
                            socket.write_all(response.as_bytes()).await.expect("write");
                            break;
                        }
                    }
                }
            }
        });

        let bridge = MiniMaxHttpAsrBridge::new(MiniMaxHttpAsrBridgeConfig {
            provider_id: "minimax.asr".to_string(),
            display_name: Some("MiniMax ASR".to_string()),
            base_url: format!("http://{}", address),
            api_key: "test-key".to_string(),
            default_model: "speech-01-turbo".to_string(),
            request_timeout: Duration::from_secs(5),
            streaming_partial_min_bytes: 4,
        })
        .expect("bridge");

        let mut request =
            request_with_selector(ProviderSelectionMode::Provider, Some("minimax.asr"));
        request.options.interim_results = true;

        let mut session = bridge.start_stream(request).await.expect("start");
        session
            .push_audio(vec![0x01, 0x00, 0x02, 0x00])
            .await
            .expect("push");
        let mut events = session.take_event_rx().expect("event rx");

        match events.recv().await.expect("partial event") {
            BridgeAsrEvent::Partial { text } => assert_eq!(text, "hello"),
            other => panic!("unexpected event: {other:?}"),
        }

        session.commit().await.expect("commit");

        match events.recv().await.expect("final event") {
            BridgeAsrEvent::Final { transcript } => assert_eq!(transcript.text, "hello world"),
            other => panic!("unexpected event: {other:?}"),
        }
        match events.recv().await.expect("ended event") {
            BridgeAsrEvent::Ended { reason } => assert_eq!(reason, None),
            other => panic!("unexpected event: {other:?}"),
        }

        server.await.expect("server");
    }
}
