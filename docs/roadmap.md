# Roadmap

## Current State

Completed or already working in the repository:

- core domain and transport boundaries
- WebSocket v1 transport contract
- `speechmeshd` gateway runtime
- mock, stdio, tcp, and agent ASR bridge modes
- generic WebSocket TTS session lifecycle
- first MeloTTS-backed TTS provider path
- split deployment for Linux gateway + macOS Apple Speech execution
- first-party Rust and Go client SDKs
- end-to-end validation clients and helper scripts
- Kubernetes and macOS service assets for the current deployment path

## Near-Term Priorities

- stabilize the public SDK surfaces
- harden deployment and operational documentation
- improve auth layering in front of the gateway
- add release automation and packaging hygiene

## Next Capability Milestones

- expand TTS beyond domain modeling into transport-backed runtime paths
- add at least one non-Apple ASR provider for contrast
- define transport support beyond WebSocket where justified
- improve observability and admin tooling

## Longer-Term Direction

- broader provider matrix
- stronger authn and authz story
- more language SDKs
- richer production operations story for multi-node deployments
