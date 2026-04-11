# Release Handoff: 2026-04

This note captures the current release and auto-update state so development can continue cleanly on another machine without relying on chat history.

## What Shipped

- `speechmesh` is now the unified client binary for CLI usage, device playback agents, and self-update flows.
- Device speaker services should run `speechmesh agent run`; do not deploy a separate legacy client binary for that role anymore.
- The client now exposes:
  - `speechmesh check-update`
  - `speechmesh self-update`
  - `speechmesh auto-update`
  - `speechmesh versions`
- Agent status reporting now includes client version and update status so rollout state can be checked centrally.
- Public GitHub Actions workflows now build release artifacts, publish GitHub release assets, generate the update manifest, and write `releases/stable.json` back to `main`.

## Current Canonical Public Update Endpoints

- Release tag: `v0.1.0`
- Release page: `https://github.com/iBreaker/speechmesh/releases/tag/v0.1.0`
- Release manifest asset: `https://github.com/iBreaker/speechmesh/releases/download/v0.1.0/stable.json`
- Main-branch manifest: `https://raw.githubusercontent.com/iBreaker/speechmesh/main/releases/stable.json`

The main-branch manifest is the preferred stable URL for installed clients because it stays constant across releases.

## Release Pipeline State

The current release pipeline is implemented by:

- `.github/workflows/build-artifacts.yml`
- `.github/workflows/release-artifacts.yml`
- `scripts/build_update_manifest.sh`

The pipeline now does all of the following:

1. builds `speechmesh` and `speechmeshd` for Linux x86_64 and macOS arm64
2. stages versioned release assets and sha256 files
3. uploads those assets to the GitHub release
4. generates `stable.json`
5. uploads `stable.json` to the GitHub release
6. commits `releases/stable.json` back to `main`

Important fixes already landed:

- directory assets are no longer passed to `gh release upload`
- upload uses explicit unique asset names so Linux/macOS files cannot collide by basename
- manifest publication stages the file before diffing, so a new `releases/stable.json` is committed correctly

## Relevant Commits

These commits are the core release/update milestones and are useful when reconstructing context:

- `4cd22fb` - unify the client and add self-update support
- `fb385e1` - add scheduled auto-update and status reporting
- `11718c4` - add public artifact build workflow
- `d7789dd` - add public release automation
- `3fd22c7` - upload only files for release assets
- `ebf7108` - use explicit unique release asset names
- `8ddaf5e` - detect manifest changes after staging
- `95dce63` - update `releases/stable.json` on `main` for `v0.1.0`

## Validated End-To-End State

The following has been validated against the published GitHub release and the raw `main` manifest:

- `build-artifacts.yml` succeeds
- `release-artifacts.yml` succeeds for the fixed pipeline
- release `v0.1.0` contains the expected binaries, tarballs, checksum files, and `stable.json`
- `https://raw.githubusercontent.com/iBreaker/speechmesh/main/releases/stable.json` returns `200`
- `speechmesh check-update --manifest-url <raw-main-manifest> --json` succeeds
- `speechmesh self-update --manifest-url <raw-main-manifest> --dry-run --force` succeeds
- manifest asset URLs are downloadable and their sha256 values match the manifest

Representative successful release run for the fixed flow:

- GitHub Actions run `24293773553`

## Release Assets Expected On GitHub

For `v0.1.0`, the release should include at least:

- `speechmesh-v0.1.0-linux-x86_64`
- `speechmesh-v0.1.0-linux-x86_64.sha256`
- `speechmesh-v0.1.0-macos-arm64`
- `speechmesh-v0.1.0-macos-arm64.sha256`
- `speechmeshd-v0.1.0-linux-x86_64`
- `speechmeshd-v0.1.0-linux-x86_64.sha256`
- `speechmeshd-v0.1.0-macos-arm64`
- `speechmeshd-v0.1.0-macos-arm64.sha256`
- `speechmesh-0.1.0-linux-x86_64.tar.gz`
- `speechmesh-0.1.0-macos-arm64.tar.gz`
- `stable.json`

Older failed runs may have left extra generic assets named `speechmesh` or `speechmeshd` on the release. They are historical leftovers, not the intended canonical asset names.

## Operational Commands

Check current release metadata from a local clone:

```bash
git log --oneline -n 8 origin/main
curl -fsS https://raw.githubusercontent.com/iBreaker/speechmesh/main/releases/stable.json | jq .
gh release view v0.1.0 --repo iBreaker/speechmesh
```

Trigger a stable release publish manually:

```bash
gh workflow run release-artifacts.yml \
  --repo iBreaker/speechmesh \
  -f channel=stable \
  -f publish_manifest=true
```

Watch the latest release workflow:

```bash
gh run list --workflow release-artifacts.yml --repo iBreaker/speechmesh --limit 5
gh run watch <run-id> --repo iBreaker/speechmesh --exit-status
```

Validate client update resolution:

```bash
speechmesh check-update \
  --manifest-url https://raw.githubusercontent.com/iBreaker/speechmesh/main/releases/stable.json \
  --json
```

Validate the download and checksum path without replacing the local binary:

```bash
speechmesh self-update \
  --manifest-url https://raw.githubusercontent.com/iBreaker/speechmesh/main/releases/stable.json \
  --dry-run \
  --force
```

## Device Deployment Notes

- Device speaker agents should be installed through `scripts/install_device_agent_service.sh`.
- The runtime service should execute `speechmesh agent run`.
- Auto-update should point at the raw main manifest URL when using the public GitHub release flow.
- macOS uses LaunchAgents; Linux uses `systemd --user`.
- The updater writes a local JSON status file that can be surfaced through agent status reporting.

## Privacy Guardrails

This repository is public. Keep release automation and docs free of:

- private LAN hostnames or domains
- local workstation paths
- tokens, secrets, or shared-secret values
- internal service URLs

Use public GitHub URLs and placeholder hostnames in committed examples.

## Known Follow-Up

One non-blocking cleanup remains:

- add workflow-level concurrency control to `release-artifacts.yml` so duplicate manual triggers for the same version do not race while pushing `releases/stable.json`

This is a robustness improvement, not a blocker for the current release/update flow.
