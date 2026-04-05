# Security Policy

## Supported Versions

SpeechMesh is currently pre-1.0.

Security fixes should be assumed to land on the latest `main` branch state first. If you maintain an internal fork, plan to rebase onto the latest upstream fixes.

## Reporting a Vulnerability

Please do not open a public issue for security-sensitive reports.

Report vulnerabilities to the project maintainers through an existing private channel. Include:

- affected component or path
- reproduction steps
- impact assessment
- any suggested mitigation if available

## Security Boundaries To Keep In Mind

Current areas that deserve extra care:

- ingress and reverse-proxy handling for long-lived WebSocket connections
- exposure of `/agent` versus `/ws`
- agent shared-secret management
- any bridge process reachable over TCP
- macOS hosts that execute Apple-native bridge binaries

## Immediate Hardening Recommendations

- run public traffic over TLS
- use a non-default agent shared secret
- keep bridge listeners off the public Internet
- scope gateway access through network policy, ingress policy, or an auth proxy
- rotate any credentials used by deployment automation

## Current Limitations

SpeechMesh does not yet provide a complete built-in authn/authz stack for public multi-tenant exposure. If you need that today, place a hardened auth layer in front of the gateway.
