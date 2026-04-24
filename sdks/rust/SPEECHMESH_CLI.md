# speechmesh

`speechmesh` is the unified SpeechMesh client binary.

It talks to a SpeechMesh gateway over the stable `/ws` WebSocket endpoint and is intended to be usable by both humans and automation/agents without reading the source code.

## Core commands

```bash
speechmesh discover providers --json
speechmesh tts voices --provider minimax.tts --json
speechmesh tts play --provider minimax.tts --text "Hello from SpeechMesh"
speechmesh tts stream --provider minimax.tts --text "Hello" > out.mp3
speechmesh asr transcribe --provider mock.asr --stdin < audio.pcm
speechmesh say --device mac03 --text "Route this to a remote speaker"
speechmesh say --text "Use the configured default target device"
speechmesh agent run --agent-id mac03-speaker-agent --device-id mac03
```

## Important behavior

- `tts play` streams audio to the local speaker and requires `ffplay` on `PATH`.
- `tts stream` writes raw audio bytes to `stdout`; progress and status messages go to `stderr`.
- `asr transcribe` reads bytes from `--file` or `--stdin` and prints the final text to `stdout`.
- All commands accept `--url` to point at a different SpeechMesh `/ws` endpoint.
- Use `--json` for one-shot structured output and `--jsonl` for streaming machine-readable events.
- `say` sends TTS to the gateway and then tells a remote `speechmesh agent run` process to play it on that machine's current OS default output device.
- Most runtime options can be omitted when matching values are set under `profiles.<name>.defaults` in `~/.speechmesh/config.yml`.
- `speechmesh say` and `speechmesh tts play/stream` can also pull provider/voice/language/rate/pitch/volume from a named `voice_profile`; if `--voice-profile` is omitted the CLI auto-selects one from `project_voice_profiles` using the current working directory's longest matching root prefix.

## Client config defaults

Default config path: `~/.speechmesh/config.yml`

```yaml
active_profile: default
profiles:
  default:
    gateway:
      ws_url: wss://speechmesh.svc.lan.breaker.host/ws
      control_url: wss://speechmesh.svc.lan.breaker.host/control
      agent_url: wss://speechmesh.svc.lan.breaker.host/agent
    client_name: speechmesh
    shared_secret: change-me
    defaults:
      device: mac03
      provider: minimax.tts
      voice: female-shaonv
      language: zh-CN
      volume: 1.2
      format: mp3
      sample_rate: 32000
      channels: 1
      rate: 1.0
      pitch: 1.0
      require: low-latency,streaming
      prefer: natural
      control_url: wss://speechmesh.svc.lan.breaker.host/control
      chunk_size_bytes: 16384
      asr_encoding: wav
      asr_sample_rate: 16000
      asr_channels: 1
      asr_language: zh-CN
      asr_interim: false
      asr_punctuation: true
      asr_timestamps: false
      asr_hints: speechmesh,codex

voice_profiles:
  speechmesh:
    provider: minimax.tts
    voice: calm_male
    language: zh-CN
    rate: 1.0
  docs:
    provider: minimax.tts
    voice: bright_female
    language: zh-CN

project_voice_profiles:
  speechmesh:
    root: /Users/breaker/src/speechmesh
    voice_profile: speechmesh
  docs:
    root: /Users/breaker/src/docs
    voice_profile: docs
```

- `active_profile` selects the profile used by default.
- `profiles.<name>.gateway.*` provides the gateway endpoints for `speechmesh` and `speechmesh agent run`.
- `profiles.<name>.shared_secret` is the agent handshake secret.
- `profiles.<name>.defaults.*` provides per-client fallbacks for `say`, `tts`, and `asr`. Any flag (`--device`, `--provider`, `--volume`, `--encoding`, etc.) passed on the command line overrides the defaults; if a flag is omitted and a matching `defaults` entry exists the CLI uses it automatically, otherwise the provider or gateway picks a sensible value.
- `voice_profiles.<name>` is the minimal reusable voice bundle for TTS behavior: `provider`, `voice`, `language`, `rate`, `pitch`, and `volume`.
- `project_voice_profiles.<name>` binds a project root path to a `voice_profile`; when multiple roots match the current working directory, the most specific root wins.

### Voice profile precedence

- Explicit TTS flags win first: `--provider`, `--voice`, `--language`, `--rate`, `--pitch`, `--volume`
- Then explicit `--voice-profile`
- Then auto-selected `project_voice_profiles` entry from the current working directory
- Then `profiles.<name>.defaults.*`

Example:

```bash
cd /Users/breaker/src/speechmesh
speechmesh say --device mac01 --text "自动带 speechmesh 项目的 voice profile"
speechmesh say --device mac01 --voice-profile docs --text "临时改用 docs profile"
speechmesh say --device mac01 --voice-profile docs --voice custom-voice --text "显式 --voice 仍然覆盖 profile"
```

### Default coverage

- `say`:
  - `device`, `chunk_size_bytes`, `control_url`
  - TTS-specific parameters inherited from `tts play/stream`: `provider`, `voice`, `language`, `rate`, `pitch`, `volume`, `format`, `sample_rate`, `channels`
- `tts`:
  - `provider`, `voice`, `language`, `rate`, `pitch`, `volume`, `format`, `sample_rate`, `channels`, `require`, `prefer`
- `asr`:
  - `provider`, `language`, `encoding` (`asr_encoding`), `sample_rate` (`asr_sample_rate`), `channels` (`asr_channels`), `interim_results`, `punctuation`, `timestamps`, `hints`

The CLI aggregates these `defaults` once per run, so you can keep your commands concise while still overriding any value by passing an explicit flag.

## Transport assumptions

- The gateway must be reachable at `--url`.
- TTS/ASR providers must already be installed on the target gateway.
- `tts play` depends on a local audio player; the current implementation uses `ffplay`.
- `speechmesh doctor` is available today and runs a `/ws` health check plus optional `/control` play test.
- `speechmesh devices` and `speechmesh agent status` are available and read from `/control` endpoints exposed by `speechmeshd`.
