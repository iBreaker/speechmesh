package speechmesh

import (
	"context"
	"encoding/json"
	"fmt"

	"github.com/coder/websocket"
	"github.com/coder/websocket/wsjson"
)

type ClientConfig struct {
	URL             string
	ProtocolVersion string
	ClientName      string
}

func (c ClientConfig) withDefaults() ClientConfig {
	if c.ProtocolVersion == "" {
		c.ProtocolVersion = "v1"
	}
	if c.ClientName == "" {
		c.ClientName = "speechmesh-go-sdk"
	}
	return c
}

type Client struct {
	config              ClientConfig
	conn                *websocket.Conn
	nextRequestID       uint64
	activeSessionID     string
	activeSessionDomain CapabilityDomain
}

type ServerError struct {
	RequestID *string
	SessionID *string
	Info      ErrorInfo
}

func (e *ServerError) Error() string {
	return fmt.Sprintf("speechmesh server error request_id=%v session_id=%v code=%s message=%s", e.RequestID, e.SessionID, e.Info.Code, e.Info.Message)
}

func Dial(ctx context.Context, config ClientConfig) (*Client, error) {
	config = config.withDefaults()
	conn, _, err := websocket.Dial(ctx, config.URL, nil)
	if err != nil {
		return nil, fmt.Errorf("dial websocket: %w", err)
	}
	client := &Client{config: config, conn: conn}
	if err := client.handshake(ctx); err != nil {
		_ = conn.Close(websocket.StatusProtocolError, "handshake failed")
		return nil, err
	}
	return client, nil
}

func (c *Client) URL() string {
	return c.config.URL
}

func (c *Client) ActiveSessionID() string {
	return c.activeSessionID
}

func (c *Client) ActiveSessionDomain() CapabilityDomain {
	return c.activeSessionDomain
}

func (c *Client) Discover(ctx context.Context, domains []CapabilityDomain) (*DiscoverResult, error) {
	requestID := c.nextID()
	if err := c.sendJSON(ctx, map[string]any{
		"type":       "discover",
		"request_id": requestID,
		"payload":    DiscoverRequest{Domains: domains},
	}); err != nil {
		return nil, err
	}
	for {
		event, err := c.Recv(ctx)
		if err != nil {
			return nil, err
		}
		switch event.Type {
		case EventTypeDiscoverResult:
			if event.RequestID != nil && *event.RequestID == requestID {
				return event.DiscoverResult, nil
			}
		case EventTypeError:
			if event.RequestID != nil && *event.RequestID == requestID {
				return nil, &ServerError{RequestID: event.RequestID, SessionID: event.SessionID, Info: event.Error.Error}
			}
		}
	}
}

func (c *Client) DiscoverASR(ctx context.Context) (*DiscoverResult, error) {
	return c.Discover(ctx, []CapabilityDomain{CapabilityDomainASR})
}

func (c *Client) DiscoverTTS(ctx context.Context) (*DiscoverResult, error) {
	return c.Discover(ctx, []CapabilityDomain{CapabilityDomainTTS})
}

func (c *Client) startSession(ctx context.Context, messageType string, request any) (string, *SessionStartedPayload, error) {
	if c.activeSessionID != "" {
		return "", nil, fmt.Errorf("speechmesh server allows only one active session per connection")
	}
	requestID := c.nextID()
	if err := c.sendJSON(ctx, map[string]any{
		"type":       messageType,
		"request_id": requestID,
		"payload":    request,
	}); err != nil {
		return "", nil, err
	}

	for {
		event, err := c.Recv(ctx)
		if err != nil {
			return "", nil, err
		}
		switch event.Type {
		case EventTypeSessionStarted:
			if event.RequestID != nil && *event.RequestID == requestID && event.SessionID != nil {
				c.activeSessionID = *event.SessionID
				if event.SessionStarted != nil {
					c.activeSessionDomain = event.SessionStarted.Domain
				}
				return *event.SessionID, event.SessionStarted, nil
			}
		case EventTypeError:
			if event.RequestID != nil && *event.RequestID == requestID {
				return "", nil, &ServerError{RequestID: event.RequestID, SessionID: event.SessionID, Info: event.Error.Error}
			}
		}
	}
}

func (c *Client) StartASR(ctx context.Context, request StreamRequest) (string, *SessionStartedPayload, error) {
	return c.startSession(ctx, "asr.start", request)
}

func (c *Client) StartTTS(ctx context.Context, request TtsStreamRequest) (string, *SessionStartedPayload, error) {
	return c.startSession(ctx, "tts.start", request)
}

func (c *Client) ensureActiveSession(domain CapabilityDomain) error {
	if c.activeSessionID == "" {
		return fmt.Errorf("no active session")
	}
	if domain != "" && c.activeSessionDomain != "" && domain != c.activeSessionDomain {
		return fmt.Errorf("active session domain %s does not match expected %s", c.activeSessionDomain, domain)
	}
	return nil
}

func (c *Client) TtsListVoices(ctx context.Context, request VoiceListRequest) (*VoiceListResult, error) {
	requestID := c.nextID()
	if err := c.sendJSON(ctx, map[string]any{
		"type":       "tts.voices",
		"request_id": requestID,
		"payload":    request,
	}); err != nil {
		return nil, err
	}
	for {
		event, err := c.Recv(ctx)
		if err != nil {
			return nil, err
		}
		switch event.Type {
		case EventTypeTtsVoicesResult:
			if event.RequestID != nil && *event.RequestID == requestID {
				return event.TtsVoicesResult, nil
			}
		case EventTypeError:
			if event.RequestID != nil && *event.RequestID == requestID {
				return nil, &ServerError{RequestID: event.RequestID, SessionID: event.SessionID, Info: event.Error.Error}
			}
		}
	}
}

func (c *Client) TtsAppendInput(ctx context.Context, delta string) error {
	if err := c.ensureActiveSession(CapabilityDomainTTS); err != nil {
		return err
	}
	return c.sendJSON(ctx, map[string]any{
		"type":       "tts.input.append",
		"session_id": c.activeSessionID,
		"payload":    TtsInputAppendPayload{Delta: delta},
	})
}

func (c *Client) TtsCommit(ctx context.Context) error {
	if err := c.ensureActiveSession(CapabilityDomainTTS); err != nil {
		return err
	}
	return c.sendJSON(ctx, map[string]any{
		"type":       "tts.commit",
		"session_id": c.activeSessionID,
		"payload":    map[string]any{},
	})
}

func (c *Client) SendAudio(ctx context.Context, chunk []byte) error {
	if err := c.ensureActiveSession(CapabilityDomainASR); err != nil {
		return err
	}
	return c.conn.Write(ctx, websocket.MessageBinary, chunk)
}

func (c *Client) Commit(ctx context.Context) error {
	if err := c.ensureActiveSession(""); err != nil {
		return err
	}
	messageType := "asr.commit"
	if c.activeSessionDomain == CapabilityDomainTTS {
		messageType = "tts.commit"
	}
	return c.sendJSON(ctx, map[string]any{
		"type":       messageType,
		"session_id": c.activeSessionID,
		"payload":    map[string]any{},
	})
}

func (c *Client) Stop(ctx context.Context) error {
	if c.activeSessionID == "" {
		return fmt.Errorf("no active session")
	}
	return c.sendJSON(ctx, map[string]any{
		"type":       "session.stop",
		"session_id": c.activeSessionID,
		"payload":    map[string]any{},
	})
}

func (c *Client) Recv(ctx context.Context) (*Event, error) {
	messageType, data, err := c.conn.Read(ctx)
	if err != nil {
		return nil, fmt.Errorf("read websocket frame: %w", err)
	}
	if messageType != websocket.MessageText {
		return nil, fmt.Errorf("unexpected non-text frame from server: %v", messageType)
	}
	var envelope struct {
		Type      string          `json:"type"`
		RequestID *string         `json:"request_id,omitempty"`
		SessionID *string         `json:"session_id,omitempty"`
		Sequence  uint64          `json:"sequence,omitempty"`
		Payload   json.RawMessage `json:"payload"`
	}
	if err := json.Unmarshal(data, &envelope); err != nil {
		return nil, fmt.Errorf("decode server message envelope: %w", err)
	}
	event := &Event{
		Type:      EventType(envelope.Type),
		RequestID: envelope.RequestID,
		SessionID: envelope.SessionID,
		Sequence:  envelope.Sequence,
	}
	decode := func(target any) error {
		if len(envelope.Payload) == 0 {
			return nil
		}
		if err := json.Unmarshal(envelope.Payload, target); err != nil {
			return fmt.Errorf("decode %s payload: %w", envelope.Type, err)
		}
		return nil
	}
	var errDecode error
	switch event.Type {
	case EventTypeHelloOK:
		var payload HelloResponse
		errDecode = decode(&payload)
		event.HelloOK = &payload
	case EventTypeDiscoverResult:
		var payload DiscoverResult
		errDecode = decode(&payload)
		event.DiscoverResult = &payload
	case EventTypeSessionStarted:
		var payload SessionStartedPayload
		errDecode = decode(&payload)
		event.SessionStarted = &payload
		if event.SessionID != nil {
			c.activeSessionID = *event.SessionID
			if event.SessionStarted != nil {
				c.activeSessionDomain = event.SessionStarted.Domain
			}
		}
	case EventTypeASRResult:
		var payload AsrResultPayload
		errDecode = decode(&payload)
		event.AsrResult = &payload
	case EventTypeSessionEnded:
		var payload SessionEndedPayload
		errDecode = decode(&payload)
		event.SessionEnded = &payload
		if event.SessionID != nil && c.activeSessionID == *event.SessionID {
			c.activeSessionID = ""
			c.activeSessionDomain = ""
		}
	case EventTypeError:
		var payload ErrorPayload
		errDecode = decode(&payload)
		event.Error = &payload
	case EventTypeTtsVoicesResult:
		var payload VoiceListResult
		errDecode = decode(&payload)
		event.TtsVoicesResult = &payload
	case EventTypeTtsAudioDelta:
		var payload TtsAudioDeltaPayload
		errDecode = decode(&payload)
		event.TtsAudioDelta = &payload
	case EventTypeTtsAudioDone:
		var payload TtsAudioDonePayload
		errDecode = decode(&payload)
		event.TtsAudioDone = &payload
	case EventTypePong:
		// empty payload is fine
	default:
		return nil, fmt.Errorf("unsupported server message type: %s", envelope.Type)
	}
	if errDecode != nil {
		return nil, errDecode
	}
	return event, nil
}

func (c *Client) Close() error {
	return c.conn.Close(websocket.StatusNormalClosure, "client closed")
}

func (c *Client) handshake(ctx context.Context) error {
	if err := c.sendJSON(ctx, map[string]any{
		"type": "hello",
		"payload": map[string]any{
			"protocol_version": c.config.ProtocolVersion,
			"client_name":      c.config.ClientName,
		},
	}); err != nil {
		return err
	}
	for {
		event, err := c.Recv(ctx)
		if err != nil {
			return err
		}
		switch event.Type {
		case EventTypeHelloOK:
			return nil
		case EventTypeError:
			return &ServerError{RequestID: event.RequestID, SessionID: event.SessionID, Info: event.Error.Error}
		}
	}
}

func (c *Client) sendJSON(ctx context.Context, payload any) error {
	if err := wsjson.Write(ctx, c.conn, payload); err != nil {
		return fmt.Errorf("write websocket frame: %w", err)
	}
	return nil
}

func (c *Client) nextID() string {
	c.nextRequestID++
	return fmt.Sprintf("req_%d", c.nextRequestID)
}
