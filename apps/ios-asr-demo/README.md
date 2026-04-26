# SpeechMesh iOS ASR Demo

Minimal iPad app for validating SpeechMesh microphone input against a remote ASR provider such as `minimax.asr`.

## Scope

This app intentionally does only four things:

- capture microphone audio on iPad
- convert it to `pcm_s16le`, `16kHz`, mono
- stream it to SpeechMesh over `/ws`
- display partial/final transcript text

It does not embed provider API keys. The app only talks to the SpeechMesh gateway.

## Generate the Xcode project

This directory uses `XcodeGen` to avoid committing a large `project.pbxproj`.

On a Mac with `xcodegen` installed:

```bash
cd apps/ios-asr-demo
xcodegen generate
open SpeechMeshASRDemo.xcodeproj
```

## Default runtime values

- Gateway URL: `ws://127.0.0.1:8080/ws`
- Provider ID: `minimax.asr`
- Language: `zh-CN`

Edit them in the app UI before recording.

## Usage

1. launch the app on iPad
2. enter the SpeechMesh gateway URL
3. hold the round record button while speaking
4. release to send `asr.commit`
5. read the transcript area for partial/final text

## Notes

- For local testing against a non-TLS websocket endpoint, App Transport Security is relaxed in `Info.plist`.
- If you want production use, tighten ATS and use `wss://`.
