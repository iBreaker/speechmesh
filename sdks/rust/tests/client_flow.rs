use std::sync::Arc;
use std::time::Duration;

use speechmesh_asr::{RecognitionOptions, StreamRequest};
use speechmesh_core::{AudioFormat, ProviderSelector};
use speechmesh_sdk::{Client, ClientConfig};
use speechmesh_transport::ServerMessage;
use speechmeshd::bridge::MockAsrBridge;
use speechmeshd::server::{ServerConfig, run_server};

#[tokio::test]
async fn rust_sdk_discovers_and_streams_against_mock_bridge() {
    let listen = reserve_local_addr();
    let bridge: speechmeshd::bridge::SharedAsrBridge = Arc::new(MockAsrBridge::new("mock.asr"));
    let server = tokio::spawn(run_server(
        ServerConfig {
            listen,
            protocol_version: "v1".to_string(),
            server_name: "speechmesh-test".to_string(),
        },
        bridge,
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

fn reserve_local_addr() -> std::net::SocketAddr {
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("bind free port");
    let addr = listener.local_addr().expect("read local addr");
    drop(listener);
    addr
}
