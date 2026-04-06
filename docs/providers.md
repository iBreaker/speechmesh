# Provider Lifecycle

SpeechMesh distinguishes three different states for providers:

- supported: present in a catalog and known to the operator
- installed: explicitly written into the gateway state by an install action
- enabled: installed and exposed by the running gateway

This boundary matters because model-backed providers should not be treated as "running" or "available" just because the codebase knows how to talk to them.

## Why The Install Boundary Exists

For heavier providers such as SenseVoice or Paraformer:

- model artifacts may need to be downloaded separately
- bridge services may need to be deployed separately
- operators may want only a subset of providers exposed on a given gateway

SpeechMesh therefore does not auto-enable every supported provider at daemon boot.

## Files

- provider catalog: declares what can be installed
- provider state: declares what is actually installed on this gateway

Example catalog:

- `deploy/providers.catalog.example.json`

Example runtime/state file:

- `deploy/providers.example.json`

The state file is intentionally a superset of the old runtime config, so the gateway can load it directly.

## CLI Workflow

List supported and installed providers:

```bash
cargo run -p speechmeshd --bin speechmeshd -- providers list \
  --catalog deploy/providers.catalog.example.json \
  --state /etc/speechmesh/providers.state.json
```

Install a provider explicitly:

```bash
cargo run -p speechmeshd --bin speechmeshd -- providers install apple.asr \
  --catalog deploy/providers.catalog.example.json \
  --state /etc/speechmesh/providers.state.json
```

Disable or remove a provider later:

```bash
cargo run -p speechmeshd --bin speechmeshd -- providers disable apple.asr \
  --state /etc/speechmesh/providers.state.json

cargo run -p speechmeshd --bin speechmeshd -- providers uninstall apple.asr \
  --state /etc/speechmesh/providers.state.json
```

## Gateway Runtime

Run the gateway against the installed-provider state:

```bash
cargo run -p speechmeshd --bin speechmeshd -- \
  --listen 127.0.0.1:8765 \
  --server-name speechmesh-dev \
  --asr-providers-state /etc/speechmesh/providers.state.json
```

At runtime:

- only installed and enabled providers are returned by `discover`
- non-installed providers are invisible to clients
- install metadata such as download notes stays in the state file for operators
- provider bridges are still lazy from the gateway's perspective and are only used when a session starts

## Current Scope

The current lifecycle layer covers explicit registration and exposure control.

What it does not yet do automatically:

- download model weights by itself
- deploy provider bridge services by itself
- hot-reload install state inside an already-running gateway

Those are valid next steps, but the install boundary now exists as a first-class operational concept.
