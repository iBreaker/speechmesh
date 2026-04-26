# Deployment

SpeechMesh supports multiple deployment shapes, but the current production-grade path is a split runtime:

- Linux or Kubernetes for the gateway and public ingress
- macOS for Apple-native speech execution
- optional local or remote TTS engines behind the same gateway

Before a provider becomes discoverable on a gateway, it should be installed into provider state explicitly. That keeps supported providers separate from actually exposed providers.

## Deployment Modes

### Local Mock Mode

Useful for protocol development and SDK tests.

```bash
cargo run -p speechmeshd --bin speechmeshd -- \
  --listen 127.0.0.1:8765 \
  --server-name speechmesh-dev \
  --asr-bridge-mode mock \
  --asr-provider-id mock.asr
```

Then validate with:

```bash
scripts/run_ws_asr_e2e.sh ws://127.0.0.1:8765/ws "speech mesh"
```

### Local Bridge Subprocess Mode

Use `stdio` when the provider bridge should be launched locally by the gateway.

```bash
cargo run -p speechmeshd --bin speechmeshd -- \
  --listen 127.0.0.1:8765 \
  --asr-bridge-mode stdio \
  --asr-provider-id bridge.asr \
  --asr-bridge-command /path/to/bridge-binary
```

### Remote TCP Bridge Mode

Use `tcp` when the bridge already exists on another host and should be reached over a trusted network.

```bash
cargo run -p speechmeshd --bin speechmeshd -- \
  --listen 0.0.0.0:8765 \
  --asr-bridge-mode tcp \
  --asr-provider-id bridge.asr \
  --asr-bridge-address bridge-host:9654
```

### Remote Agent Mode

Use `agent` for the current Apple Speech production path.

```bash
cargo run -p speechmeshd --bin speechmeshd -- \
  --listen 0.0.0.0:8765 \
  --server-name speechmesh-gateway \
  --asr-bridge-mode agent \
  --asr-provider-id apple.asr \
  --agent-shared-secret change-me
```

### Installed Provider State

The gateway can also load an installed-provider state file directly.

```bash
cargo run -p speechmeshd --bin speechmeshd -- \
  --listen 0.0.0.0:8765 \
  --server-name speechmesh-gateway \
  --asr-providers-state /etc/speechmesh/providers.state.json
```

Populate that state explicitly:

```bash
cargo run -p speechmeshd --bin speechmeshd -- providers install apple.asr \
  --catalog deploy/providers.catalog.example.json \
  --state /etc/speechmesh/providers.state.json
```

### Local MeloTTS Sidecar Or Host Service

The first concrete TTS provider path is a MeloTTS HTTP service plus `speechmeshd` in `melo-http` mode.

Example gateway process:

```bash
cargo run -p speechmeshd --bin speechmeshd -- \
  --listen 127.0.0.1:8765 \
  --server-name speechmesh-dev \
  --asr-bridge-mode mock \
  --asr-provider-id mock.asr \
  --tts-bridge-mode melo-http \
  --tts-provider-id melo.tts \
  --tts-provider-name MeloTTS \
  --tts-melo-base-url http://127.0.0.1:7797
```

In this shape:

- `speechmeshd` owns the public WebSocket contract
- the MeloTTS service stays behind HTTP
- SpeechMesh normalizes it into `tts.start` / `tts.input.append` / `tts.commit` / `tts.audio.delta`

## Production Split: Linux Gateway + macOS Agent

### Kubernetes Gateway

The repository includes:

- image build context: `Dockerfile`
- Kubernetes manifest: `deploy/k8s/speechmesh.yaml`
- deployment helper: `scripts/deploy_k8s.sh`

Example:

```bash
./scripts/deploy_k8s.sh \
  --image-tag 20260405-1 \
  --ingress-host speechmesh.example.com \
  --agent-shared-secret change-me \
  --remote-host k3s-host
```

The helper script:

1. builds a Linux image locally
2. copies the image tar to the remote k3s host
3. imports it into containerd
4. applies `deploy/k8s/speechmesh.yaml` with runtime substitutions
5. waits for the `apps/speechmesh` rollout

The Kubernetes manifest enables MiniMax TTS by default for production playback. The
gateway pod expects a namespace-local secret named `speechmesh-minimax` with:

- `MINIMAX_API_KEY`
- `MINIMAX_GROUP_ID`

Without that secret, `discover` and `doctor` will show no TTS providers and
`speechmesh say` cannot synthesize audio for device playback.

### macOS Apple ASR Agent

The repository includes:

- LaunchAgent plist template: `deploy/macos/io.speechmesh.apple-agent.plist`
- installer helper: `scripts/install_apple_agent_service.sh`
- provider catalog example: `deploy/providers.catalog.example.json`

Example:

```bash
./scripts/install_apple_agent_service.sh install \
  --gateway-url wss://speechmesh.example.com/agent \
  --agent-id apple-agent-1 \
  --agent-name "Apple ASR Agent" \
  --shared-secret change-me
```

That script:

1. builds `apple_agent` and the Swift Apple bridge unless `--skip-build` is used
2. renders a LaunchAgent plist into `~/Library/LaunchAgents`
3. bootstraps the service through `launchctl`
4. tails status through `launchctl print`

Logs land in:

- `~/Library/Logs/SpeechMesh/apple-agent.log`
- `~/Library/Logs/SpeechMesh/apple-agent.err.log`

### macOS Device Speaker Agent

The repository includes:

- LaunchAgent plist template: `deploy/macos/io.speechmesh.device-agent.plist`
- LaunchAgent auto-update template: `deploy/macos/io.speechmesh.device-agent-updater.plist`
- Linux user service template: `deploy/linux/speechmesh-device-agent.service`
- Linux user auto-update templates: `deploy/linux/speechmesh-device-agent-updater.service`, `deploy/linux/speechmesh-device-agent-updater.timer`
- installer helper: `scripts/install_device_agent_service.sh`

Example:

```bash
./scripts/install_device_agent_service.sh install \
  --gateway-url wss://speechmesh.example.com/agent \
  --agent-id mac03-speaker-agent \
  --agent-name "Mac 03 Speaker Agent" \
  --device-id mac03 \
  --shared-secret change-me \
  --update-manifest-url https://speechmesh.example.com/releases/speechmesh.json
```

That script:

1. builds the unified client binary `speechmesh` unless `--skip-build` is used
2. installs it into `~/bin/speechmesh` unless `--skip-install-binary` is used
3. renders a LaunchAgent plist that runs `speechmesh agent run`
4. optionally renders a second LaunchAgent that runs `speechmesh auto-update --once` on `StartInterval`
5. bootstraps the service through `launchctl`

Device speaker agent rollout should use the unified `speechmesh` client binary only.

Logs land in:

- `~/Library/Logs/SpeechMesh/device-agent.log`
- `~/Library/Logs/SpeechMesh/device-agent.err.log`
- `~/Library/Logs/SpeechMesh/device-agent-updater.log`
- `~/Library/Logs/SpeechMesh/device-agent-updater.err.log`

Auto-update status lands in:

- `~/Library/Application Support/SpeechMesh/device-agent-update.json` by default

### Linux Device Speaker Agent

The same installer helper manages Linux user services through `systemd --user`.

Example:

```bash
./scripts/install_device_agent_service.sh install \
  --platform linux \
  --gateway-url wss://speechmesh.example.com/agent \
  --agent-id linux01-speaker-agent \
  --agent-name "Linux 01 Speaker Agent" \
  --device-id linux01 \
  --shared-secret change-me \
  --update-manifest-url https://speechmesh.example.com/releases/speechmesh.json
```

That path:

1. builds and installs `speechmesh` unless `--skip-build` / `--skip-install-binary` is used
2. renders `~/.config/systemd/user/speechmesh-device-agent.service`
3. optionally renders `speechmesh-device-agent-updater.service` and `.timer`
4. enables and starts the service through `systemctl --user`

Logs land in:

- `~/.local/state/speechmesh/device-agent.log`
- `~/.local/state/speechmesh/device-agent.err.log`
- `~/.local/state/speechmesh/device-agent-updater.log`
- `~/.local/state/speechmesh/device-agent-updater.err.log`

Auto-update status lands in:

- `~/.local/state/speechmesh/device-agent-update.json` by default

### Android Device Speaker Agent (Termux MVP)

For Android-first validation, use Termux as the runtime host for the same unified `speechmesh` binary.

- quickstart guide: `docs/android-device-agent.md`
- installer helper: `scripts/install_termux_device_agent_service.sh`

Typical flow inside Termux:

```bash
pkg install rust termux-services
source $PREFIX/etc/profile.d/start-services.sh
./scripts/install_termux_device_agent_service.sh install \
  --gateway-url wss://speechmesh.example.com/agent \
  --agent-id android01-speaker-agent \
  --device-id android01 \
  --shared-secret change-me
```

The Android runtime supports `SPEECHMESH_PLAYBACK_CMD` for hosts where `ffplay` is not practical.
When unset, playback tries `ffplay` first and falls back to `mpv`.

### Auto-Update CLI

The unified client can also run update checks directly:

```bash
speechmesh auto-update \
  --manifest-url https://speechmesh.example.com/releases/speechmesh.json \
  --channel stable \
  --once \
  --restart-mode systemd-user \
  --service-name speechmesh-device-agent.service \
  --status-file ~/.local/state/speechmesh/device-agent-update.json
```

For the public GitHub release flow, point clients at the stable raw manifest:

```bash
speechmesh auto-update \
  --manifest-url https://raw.githubusercontent.com/iBreaker/speechmesh/main/releases/stable.json \
  --channel stable \
  --once
```

## Public Release And Update Pipeline

The repository now supports a public GitHub-driven release/update path for the unified `speechmesh` client.

- `build-artifacts.yml` builds public artifacts for Linux x86_64 and macOS arm64
- `release-artifacts.yml` publishes versioned release assets plus `stable.json`
- `scripts/build_update_manifest.sh` generates the update manifest consumed by `check-update`, `self-update`, and `auto-update`
- successful release publication writes `releases/stable.json` back to `main`

Current stable manifest URL:

```text
https://raw.githubusercontent.com/iBreaker/speechmesh/main/releases/stable.json
```

Recommended operator checks:

```bash
gh release view v0.1.0 --repo iBreaker/speechmesh
curl -fsS https://raw.githubusercontent.com/iBreaker/speechmesh/main/releases/stable.json | jq .
speechmesh check-update \
  --manifest-url https://raw.githubusercontent.com/iBreaker/speechmesh/main/releases/stable.json \
  --json
```

If you are continuing rollout work on another machine, read `docs/release-handoff-2026-04.md` first.

That command emits one JSON status line to stdout and updates the status file so fleet checks can compare current version, target version, and whether an update was applied.

### Legacy Client Binary Compatibility

`scripts/install_device_agent_service.sh` supports `--legacy-compat` to manage old command names:

- `wrap` (default): install wrapper shims for `speechmesh-agent` and `speechmesh-cli` that forward to `speechmesh`
- `keep`: do not touch old binaries
- `remove`: remove old `speechmesh-agent` and `speechmesh-cli` files from the install directory

## Network Shape

In the current split deployment:

- public or internal clients connect to `wss://<host>/ws`
- the macOS agent connects outbound to `wss://<host>/agent`
- the Apple bridge stays local to the macOS host and is launched by the agent

This means the macOS host does not need a public listener for the bridge process.

## Verification

After deployment, verify from the repository root:

```bash
./scripts/run_ws_asr_e2e.sh wss://speechmesh.example.com/ws "speech mesh"
./scripts/run_go_sdk_e2e.sh wss://speechmesh.example.com/ws "speech mesh"
```

## Security Notes

Current hardening recommendations:

- terminate TLS at the ingress layer and use `wss://` externally
- set a non-default `--agent-shared-secret`
- expose `/agent` only through the gateway, never the local Apple bridge directly
- treat the macOS host as trusted infrastructure because it runs the platform-native provider process
- prefer private networking between the ingress layer, gateway, and operator hosts

What SpeechMesh does not yet provide natively:

- end-user OAuth on the `/ws` endpoint
- role-based access control inside the gateway
- multi-tenant provider isolation

What the provider lifecycle layer does not yet automate:

- model downloads for third-party ASR providers
- bridge rollout orchestration
- in-place hot reload of provider state in a running gateway

If you need those today, put an auth proxy or ingress policy in front of the gateway.
