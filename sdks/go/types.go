package speechmesh

import "encoding/json"

type CapabilityDomain string

const (
	CapabilityDomainASR       CapabilityDomain = "asr"
	CapabilityDomainTTS       CapabilityDomain = "tts"
	CapabilityDomainTransport CapabilityDomain = "transport"
)

type ProviderSelectionMode string

const (
	ProviderSelectionModeAuto     ProviderSelectionMode = "auto"
	ProviderSelectionModeProvider ProviderSelectionMode = "provider"
)

type ProviderSelector struct {
	Mode                  ProviderSelectionMode `json:"mode"`
	ProviderID            *string               `json:"provider_id"`
	RequiredCapabilities  []string              `json:"required_capabilities"`
	PreferredCapabilities []string              `json:"preferred_capabilities"`
}

func DefaultProviderSelector() ProviderSelector {
	return ProviderSelector{
		Mode:                  ProviderSelectionModeAuto,
		RequiredCapabilities:  []string{},
		PreferredCapabilities: []string{},
	}
}

type Capability struct {
	Key     string `json:"key"`
	Enabled bool   `json:"enabled"`
}

type RuntimeMode string

const (
	RuntimeModeInProcess     RuntimeMode = "in_process"
	RuntimeModeLocalDaemon   RuntimeMode = "local_daemon"
	RuntimeModeRemoteGateway RuntimeMode = "remote_gateway"
)

type ProviderDescriptor struct {
	ID           string           `json:"id"`
	Name         string           `json:"name"`
	Domain       CapabilityDomain `json:"domain"`
	Runtime      RuntimeMode      `json:"runtime"`
	Capabilities []Capability     `json:"capabilities"`
}

type AudioEncoding string

const (
	AudioEncodingPCMS16LE AudioEncoding = "pcm_s16le"
	AudioEncodingPCMF32LE AudioEncoding = "pcm_f32le"
	AudioEncodingOpus     AudioEncoding = "opus"
	AudioEncodingMP3      AudioEncoding = "mp3"
	AudioEncodingAAC      AudioEncoding = "aac"
	AudioEncodingFLAC     AudioEncoding = "flac"
	AudioEncodingWAV      AudioEncoding = "wav"
)

type AudioFormat struct {
	Encoding     AudioEncoding `json:"encoding"`
	SampleRateHz uint32        `json:"sample_rate_hz"`
	Channels     uint16        `json:"channels"`
}

func PCMS16LE(sampleRateHz uint32, channels uint16) AudioFormat {
	return AudioFormat{
		Encoding:     AudioEncodingPCMS16LE,
		SampleRateHz: sampleRateHz,
		Channels:     channels,
	}
}

type RecognitionOptions struct {
	Language        *string        `json:"language,omitempty"`
	Hints           []string       `json:"hints"`
	InterimResults  bool           `json:"interim_results"`
	Timestamps      bool           `json:"timestamps"`
	Punctuation     bool           `json:"punctuation"`
	PreferOnDevice  bool           `json:"prefer_on_device"`
	ProviderOptions map[string]any `json:"provider_options,omitempty"`
}

func DefaultRecognitionOptions() RecognitionOptions {
	return RecognitionOptions{
		Hints: []string{},
	}
}

type StreamRequest struct {
	Provider    ProviderSelector   `json:"provider"`
	InputFormat AudioFormat        `json:"input_format"`
	Options     RecognitionOptions `json:"options"`
}

type HelloResponse struct {
	ProtocolVersion         string `json:"protocol_version"`
	ServerName              string `json:"server_name"`
	OneSessionPerConnection bool   `json:"one_session_per_connection"`
}

type DiscoverRequest struct {
	Domains []CapabilityDomain `json:"domains"`
}

type DiscoverResult struct {
	Providers []ProviderDescriptor `json:"providers"`
}

type SessionStartedPayload struct {
	Domain               CapabilityDomain `json:"domain"`
	ProviderID           string           `json:"provider_id"`
	AcceptedInputFormat  *AudioFormat     `json:"accepted_input_format,omitempty"`
	AcceptedOutputFormat *AudioFormat     `json:"accepted_output_format,omitempty"`
}

type AsrWordPayload struct {
	Text    string  `json:"text"`
	StartMS *uint64 `json:"start_ms,omitempty"`
	EndMS   *uint64 `json:"end_ms,omitempty"`
	IsFinal bool    `json:"is_final"`
}

type AsrResultPayload struct {
	SegmentID   uint64           `json:"segment_id"`
	Revision    uint64           `json:"revision"`
	Text        string           `json:"text"`
	Delta       *string          `json:"delta,omitempty"`
	IsFinal     bool             `json:"is_final"`
	SpeechFinal bool             `json:"speech_final"`
	BeginTimeMS *uint64          `json:"begin_time_ms,omitempty"`
	EndTimeMS   *uint64          `json:"end_time_ms,omitempty"`
	Words       []AsrWordPayload `json:"words"`
}

type SessionEndedPayload struct {
	Reason *string `json:"reason,omitempty"`
}

type VoiceListRequest struct {
	Provider ProviderSelector `json:"provider"`
	Language *string          `json:"language,omitempty"`
}

type VoiceDescriptor struct {
	ID           string   `json:"id"`
	Language     string   `json:"language"`
	DisplayName  string   `json:"display_name"`
	Gender       *string  `json:"gender,omitempty"`
	Capabilities []string `json:"capabilities"`
}

type VoiceListResult struct {
	Voices []VoiceDescriptor `json:"voices"`
}

type SynthesisInputKind string

const (
	SynthesisInputKindText SynthesisInputKind = "text"
	SynthesisInputKindSsml SynthesisInputKind = "ssml"
)

type TtsSynthesisOptions struct {
	Language        *string        `json:"language,omitempty"`
	Voice           *string        `json:"voice,omitempty"`
	Stream          bool           `json:"stream"`
	Rate            *float32       `json:"rate,omitempty"`
	Pitch           *float32       `json:"pitch,omitempty"`
	Volume          *float32       `json:"volume,omitempty"`
	ProviderOptions map[string]any `json:"provider_options,omitempty"`
}

type TtsStreamRequest struct {
	Provider     ProviderSelector    `json:"provider"`
	InputKind    SynthesisInputKind  `json:"input_kind"`
	OutputFormat *AudioFormat        `json:"output_format,omitempty"`
	Options      TtsSynthesisOptions `json:"options"`
}

type TtsInputAppendPayload struct {
	Delta string `json:"delta"`
}

type TtsAudioDeltaPayload struct {
	ChunkID     uint64       `json:"chunk_id"`
	AudioBase64 string       `json:"audio_base64"`
	IsFinal     bool         `json:"is_final"`
	Format      *AudioFormat `json:"format,omitempty"`
}

type TtsAudioDonePayload struct {
	InputKind   SynthesisInputKind `json:"input_kind"`
	TotalChunks uint64             `json:"total_chunks"`
	TotalBytes  uint64             `json:"total_bytes"`
}

type ErrorInfo struct {
	Code      string          `json:"code"`
	Message   string          `json:"message"`
	Retryable bool            `json:"retryable"`
	Details   json.RawMessage `json:"details"`
}

type ErrorPayload struct {
	Error ErrorInfo `json:"error"`
}

type EventType string

const (
	EventTypeHelloOK         EventType = "hello.ok"
	EventTypeDiscoverResult  EventType = "discover.result"
	EventTypeSessionStarted  EventType = "session.started"
	EventTypeASRResult       EventType = "asr.result"
	EventTypeSessionEnded    EventType = "session.ended"
	EventTypeError           EventType = "error"
	EventTypePong            EventType = "pong"
	EventTypeTtsVoicesResult EventType = "tts.voices.result"
	EventTypeTtsAudioDelta   EventType = "tts.audio.delta"
	EventTypeTtsAudioDone    EventType = "tts.audio.done"
)

type Event struct {
	Type            EventType
	RequestID       *string
	SessionID       *string
	Sequence        uint64
	HelloOK         *HelloResponse
	DiscoverResult  *DiscoverResult
	SessionStarted  *SessionStartedPayload
	AsrResult       *AsrResultPayload
	SessionEnded    *SessionEndedPayload
	Error           *ErrorPayload
	TtsVoicesResult *VoiceListResult
	TtsAudioDelta   *TtsAudioDeltaPayload
	TtsAudioDone    *TtsAudioDonePayload
}
