# Android Device Agent (Termux MVP)

This guide is for bringing a real Android phone online as a SpeechMesh speaker agent quickly, without building a full native Android app first.

The runtime path is:

1. install Termux on the phone
2. run the unified `speechmesh` binary inside Termux
3. keep it alive with `termux-services` (and optionally Termux:Boot)

## Why this path

- fastest path to validate 24h online behavior
- keeps the same `speechmesh agent run` runtime as Linux/macOS
- no protocol fork between desktop and mobile

## Prerequisites

- Android phone connected over `adb`
- network path to your `speechmeshd` gateway `/agent` endpoint
- shared secret matching gateway configuration

Inside Termux you need:

- `pkg install rust termux-services`
- optional player package:
  - `pkg install ffmpeg` (for `ffplay`)
  - or `pkg install mpv` (fallback player)

## Install Termux

Use an official Termux build (package id `com.termux`).  
If you install through `adb install`, Android may show a manual security confirmation prompt.

## Prepare Termux environment

Run these on the phone in Termux:

```bash
pkg update
pkg install rust termux-services
source $PREFIX/etc/profile.d/start-services.sh
```

Optional but recommended for 24h tests:

```bash
termux-wake-lock
```

## Deploy speechmesh binary

Clone the repo inside Termux, then run:

```bash
./scripts/install_termux_device_agent_service.sh install \
  --gateway-url wss://speechmesh.example.com/agent \
  --agent-id android01-speaker-agent \
  --agent-name "Android 01 Speaker Agent" \
  --device-id android01 \
  --shared-secret change-me
```

If you prefer a specific playback command, set it during install:

```bash
./scripts/install_termux_device_agent_service.sh install \
  --playback-cmd "mpv --no-config --no-video --no-terminal --really-quiet -"
```

This writes a runit service under:

- `~/.termux/service/speechmesh-device-agent/run`
- `~/.termux/service/speechmesh-device-agent/log/run`

## Service operations

```bash
./scripts/install_termux_device_agent_service.sh status
./scripts/install_termux_device_agent_service.sh restart
./scripts/install_termux_device_agent_service.sh stop
./scripts/install_termux_device_agent_service.sh uninstall
```

## Optional: start on phone reboot

Install Termux:Boot app, then re-run install with:

```bash
./scripts/install_termux_device_agent_service.sh install --enable-boot
```

This writes:

- `~/.termux/boot/speechmesh-device-agent.sh`

## Verify end to end

On your operator machine:

```bash
speechmesh devices --json
speechmesh say --device android01 --text "Android agent test."
```

Expected:

- device list includes `android01-speaker-agent`
- `say` routes to that agent and finishes successfully

## Notes

- runtime now supports `SPEECHMESH_PLAYBACK_CMD` for mobile hosts
- when unset, runtime tries `ffplay` first, then `mpv`
- playback still follows Android's current default output route (speaker / Bluetooth headset)
