package speechmesh

import (
	"context"
	"encoding/json"
	"fmt"
	"net/http"
	"net/http/httptest"
	"testing"
	"time"

	"github.com/coder/websocket"
)

func TestClientDiscoverAndStream(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		conn, err := websocket.Accept(w, r, nil)
		if err != nil {
			t.Errorf("accept websocket: %v", err)
			return
		}
		defer conn.Close(websocket.StatusNormalClosure, "done")

		var sessionID string = "sess-123"
		var bufferedBytes int
		for {
			messageType, data, err := conn.Read(context.Background())
			if err != nil {
				return
			}
			if messageType == websocket.MessageBinary {
				bufferedBytes += len(data)
				continue
			}
			var envelope map[string]any
			if err := json.Unmarshal(data, &envelope); err != nil {
				t.Errorf("decode client message: %v", err)
				return
			}
			switch envelope["type"] {
			case "hello":
				mustWriteJSON(t, conn, map[string]any{
					"type": "hello.ok",
					"payload": map[string]any{
						"protocol_version":           "v1",
						"server_name":                "mock-server",
						"one_session_per_connection": true,
					},
				})
			case "discover":
				mustWriteJSON(t, conn, map[string]any{
					"type":       "discover.result",
					"request_id": envelope["request_id"],
					"payload": map[string]any{
						"providers": []map[string]any{{
							"id":      "mock.asr",
							"name":    "Mock ASR",
							"domain":  "asr",
							"runtime": "local_daemon",
							"capabilities": []map[string]any{{
								"key":     "streaming-input",
								"enabled": true,
							}},
						}},
					},
				})
			case "asr.start":
				mustWriteJSON(t, conn, map[string]any{
					"type":       "session.started",
					"request_id": envelope["request_id"],
					"session_id": sessionID,
					"payload": map[string]any{
						"domain":      "asr",
						"provider_id": "mock.asr",
						"accepted_input_format": map[string]any{
							"encoding":       "pcm_s16le",
							"sample_rate_hz": 16000,
							"channels":       1,
						},
					},
				})
			case "asr.commit":
				mustWriteJSON(t, conn, map[string]any{
					"type":       "asr.result",
					"session_id": sessionID,
					"sequence":   1,
					"payload": map[string]any{
						"segment_id":   0,
						"revision":     1,
						"text":         fmt.Sprintf("mock partial bytes=%d", bufferedBytes),
						"delta":        fmt.Sprintf("mock partial bytes=%d", bufferedBytes),
						"is_final":     false,
						"speech_final": false,
						"words":        []any{},
					},
				})
				mustWriteJSON(t, conn, map[string]any{
					"type":       "asr.result",
					"session_id": sessionID,
					"sequence":   2,
					"payload": map[string]any{
						"segment_id":   0,
						"revision":     2,
						"text":         fmt.Sprintf("mock transcript bytes=%d", bufferedBytes),
						"delta":        fmt.Sprintf("mock transcript bytes=%d", bufferedBytes),
						"is_final":     true,
						"speech_final": true,
						"words":        []any{},
					},
				})
				mustWriteJSON(t, conn, map[string]any{
					"type":       "session.ended",
					"session_id": sessionID,
					"payload":    map[string]any{},
				})
			}
		}
	}))
	defer server.Close()

	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	client, err := Dial(ctx, ClientConfig{URL: wsURL(server.URL)})
	if err != nil {
		t.Fatalf("dial client: %v", err)
	}
	defer client.Close()

	discover, err := client.DiscoverASR(ctx)
	if err != nil {
		t.Fatalf("discover asr: %v", err)
	}
	if got := len(discover.Providers); got != 1 {
		t.Fatalf("expected one provider, got %d", got)
	}

	language := "en-US"
	_, started, err := client.StartASR(ctx, StreamRequest{
		Provider:    DefaultProviderSelector(),
		InputFormat: PCMS16LE(16000, 1),
		Options: RecognitionOptions{
			Language:       &language,
			Hints:          []string{"speechmesh"},
			InterimResults: true,
			Punctuation:    true,
		},
	})
	if err != nil {
		t.Fatalf("start asr: %v", err)
	}
	if started.ProviderID != "mock.asr" {
		t.Fatalf("unexpected provider id: %s", started.ProviderID)
	}

	if err := client.SendAudio(ctx, make([]byte, 3200)); err != nil {
		t.Fatalf("send audio: %v", err)
	}
	if err := client.Commit(ctx); err != nil {
		t.Fatalf("commit: %v", err)
	}

	var sawFinal bool
	var sawEnded bool
	for !(sawFinal && sawEnded) {
		event, err := client.Recv(ctx)
		if err != nil {
			t.Fatalf("recv event: %v", err)
		}
		switch event.Type {
		case EventTypeASRResult:
			if event.AsrResult.IsFinal {
				sawFinal = true
				if event.AsrResult.Text != "mock transcript bytes=3200" {
					t.Fatalf("unexpected final text: %s", event.AsrResult.Text)
				}
			}
		case EventTypeSessionEnded:
			sawEnded = true
		}
	}
}

func TestClientRejectsAudioWithoutSession(t *testing.T) {
	client := &Client{}
	if err := client.SendAudio(context.Background(), []byte{1, 2, 3}); err == nil {
		t.Fatal("expected error when sending audio without active session")
	}
}

func mustWriteJSON(t *testing.T, conn *websocket.Conn, payload any) {
	t.Helper()
	data, err := json.Marshal(payload)
	if err != nil {
		t.Fatalf("marshal payload: %v", err)
	}
	if err := conn.Write(context.Background(), websocket.MessageText, data); err != nil {
		t.Fatalf("write websocket message: %v", err)
	}
}

func wsURL(serverURL string) string {
	return "ws" + serverURL[len("http"):] + "/ws"
}
