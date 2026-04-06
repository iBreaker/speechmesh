use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use speechmesh_asr::{RecognitionOptions, StreamRequest};
use speechmesh_core::{AudioFormat, ProviderSelector};
use speechmesh_sdk::{
    Client, ClientConfig, SynthesisInputKind, SynthesisOptions, TtsStreamRequest,
};
use speechmesh_transport::ServerMessage;
use speechmeshd::asr_bridge::MockAsrBridge;
use speechmeshd::server::{ServerConfig, run_server};
use speechmeshd::tts_bridge::MockTtsBridge;

#[tokio::test]
async fn rust_sdk_discovers_and_streams_against_mock_bridge() {
    let listen = reserve_local_addr();
    let bridge: speechmeshd::asr_bridge::SharedAsrBridge = Arc::new(MockAsrBridge::new("mock.asr"));
    let server = tokio::spawn(run_server(
        ServerConfig {
            listen,
            protocol_version: "v1".to_string(),
            server_name: "speechmesh-test".to_string(),
        },
        bridge,
        None,
        None,
    ));

    tokio::time::sleep(Duration::from_millis(100)).await;

    let mut client = Client::connect(ClientConfig::new(format!("ws://{listen}/ws")))
        .await
        .expect("connect sdk client");

    let discover = client.discover_asr().await.expect("discover asr providers");
    assert_eq!(discover.providers.len(), 1);
    assert_eq!(discover.providers[0].id, "mock.asr");

    let started = client
        .start_asr(StreamRequest {
            provider: ProviderSelector::default(),
            input_format: AudioFormat::pcm_s16le(16_000, 1),
            options: RecognitionOptions {
                language: Some("en-US".to_string()),
                interim_results: true,
                punctuation: true,
                ..RecognitionOptions::default()
            },
        })
        .await
        .expect("start asr");
    assert_eq!(started.payload.provider_id, "mock.asr");

    client
        .send_audio(&vec![0_u8; 3200])
        .await
        .expect("send audio");
    client.commit().await.expect("commit audio");

    let mut saw_partial = false;
    let mut saw_final = false;
    let mut saw_ended = false;

    while !(saw_final && saw_ended) {
        match client.recv().await.expect("read event") {
            ServerMessage::AsrResult { payload, .. } => {
                if payload.is_final {
                    saw_final = true;
                    assert!(payload.text.contains("mock transcript bytes=3200"));
                } else {
                    saw_partial = true;
                    assert!(payload.text.contains("mock partial bytes=3200"));
                }
            }
            ServerMessage::SessionEnded { .. } => {
                saw_ended = true;
            }
            other => panic!("unexpected message: {other:?}"),
        }
    }

    assert!(saw_partial);
    client.close().await.expect("close sdk client");
    server.abort();
}

#[tokio::test]
async fn rust_sdk_lists_voices_and_streams_tts_against_mock_bridge() {
    let listen = reserve_local_addr();
    let asr_bridge: speechmeshd::asr_bridge::SharedAsrBridge =
        Arc::new(MockAsrBridge::new("mock.asr"));
    let tts_bridge: speechmeshd::tts_bridge::SharedTtsBridge =
        Arc::new(MockTtsBridge::new("mock.tts"));
    let server = tokio::spawn(run_server(
        ServerConfig {
            listen,
            protocol_version: "v1".to_string(),
            server_name: "speechmesh-test".to_string(),
        },
        asr_bridge,
        Some(tts_bridge),
        None,
    ));

    tokio::time::sleep(Duration::from_millis(100)).await;

    let mut client = Client::connect(ClientConfig::new(format!("ws://{listen}/ws")))
        .await
        .expect("connect sdk client");

    let discover = client.discover_tts().await.expect("discover tts providers");
    assert_eq!(discover.providers.len(), 1);
    assert_eq!(discover.providers[0].id, "mock.tts");

    let voices = client
        .list_tts_voices(ProviderSelector::provider("mock.tts"), None)
        .await
        .expect("list tts voices");
    assert_eq!(voices.voices.len(), 1);
    assert_eq!(voices.voices[0].id, "mock.default");

    let started = client
        .start_tts(TtsStreamRequest {
            provider: ProviderSelector::provider("mock.tts"),
            input_kind: SynthesisInputKind::Text,
            output_format: None,
            options: SynthesisOptions::default(),
        })
        .await
        .expect("start tts");
    assert_eq!(started.payload.provider_id, "mock.tts");

    client
        .append_tts_input("hello from ")
        .await
        .expect("append tts input");
    client
        .append_tts_input("rust sdk")
        .await
        .expect("append more tts input");
    client.commit().await.expect("commit tts input");

    let mut audio = Vec::new();
    let mut saw_done = false;
    let mut saw_ended = false;

    while !(saw_done && saw_ended) {
        match client.recv().await.expect("read event") {
            ServerMessage::TtsAudioDelta { payload, .. } => {
                let decoded = BASE64_STANDARD
                    .decode(payload.audio_base64)
                    .expect("decode audio chunk");
                audio.extend(decoded);
            }
            ServerMessage::TtsAudioDone { payload, .. } => {
                saw_done = true;
                assert_eq!(payload.total_chunks, 1);
                assert_eq!(payload.total_bytes as usize, audio.len());
            }
            ServerMessage::SessionEnded { .. } => {
                saw_ended = true;
            }
            other => panic!("unexpected message: {other:?}"),
        }
    }

    assert_eq!(audio, b"MOCK_TTS:hello from rust sdk");
    client.close().await.expect("close sdk client");
    server.abort();
}

fn reserve_local_addr() -> std::net::SocketAddr {
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("bind free port");
    let addr = listener.local_addr().expect("read local addr");
    drop(listener);
    addr
}
