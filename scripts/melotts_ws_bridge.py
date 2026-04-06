#!/usr/bin/env python3
from __future__ import annotations

import argparse
import asyncio
import base64
import json
import time
import uuid
import urllib.error
import urllib.request
from dataclasses import dataclass, field
from typing import Any, Optional

import uvicorn
from fastapi import FastAPI, WebSocket, WebSocketDisconnect
from fastapi.responses import JSONResponse


@dataclass
class BridgeConfig:
    listen_host: str
    listen_port: int
    provider_id: str
    melotts_base_url: str
    request_timeout_secs: float
    default_speed: float
    default_chunk_bytes: int


@dataclass
class SessionState:
    session_id: str
    speed: float
    chunk_bytes: int
    text_parts: list[str] = field(default_factory=list)

    def append_text(self, value: str) -> None:
        text = value.strip()
        if text:
            self.text_parts.append(text)

    def full_text(self) -> str:
        return " ".join(self.text_parts).strip()


@dataclass
class MeloTtsResult:
    audio_wav: bytes
    render_millis: Optional[int]
    sample_rate_hz: Optional[int]
    speaker: Optional[str]


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description=(
            "SpeechMesh first-pass MeloTTS bridge: "
            "WebSocket TTS facade backed by Melo local HTTP server."
        )
    )
    parser.add_argument("--listen-host", default="127.0.0.1")
    parser.add_argument("--listen-port", type=int, default=8797)
    parser.add_argument("--provider-id", default="melotts.local")
    parser.add_argument("--melotts-base-url", default="http://127.0.0.1:7797")
    parser.add_argument("--request-timeout-secs", type=float, default=30.0)
    parser.add_argument("--default-speed", type=float, default=1.0)
    parser.add_argument("--default-chunk-bytes", type=int, default=16384)
    parser.add_argument("--log-level", default="info")
    return parser.parse_args()


def create_app(config: BridgeConfig) -> FastAPI:
    app = FastAPI(title="SpeechMesh MeloTTS Bridge", version="0.1.0")

    @app.get("/healthz")
    async def healthz() -> JSONResponse:
        ok, detail = await asyncio.to_thread(check_melotts_health, config)
        return JSONResponse({"ok": ok, "provider_id": config.provider_id, "upstream": detail})

    @app.websocket("/ws/tts")
    async def websocket_tts(websocket: WebSocket) -> None:
        await websocket.accept()
        sessions: dict[str, SessionState] = {}

        try:
            while True:
                raw = await websocket.receive_text()
                request_id, message_type, payload = decode_client_message(raw)
                try:
                    if message_type == "hello":
                        await send_message(
                            websocket,
                            {
                                "type": "hello.ok",
                                "request_id": request_id,
                                "payload": {
                                    "protocol_version": "v1alpha1",
                                    "server_name": "speechmesh-melotts-bridge",
                                    "provider_id": config.provider_id,
                                    "mode": "tts",
                                },
                            },
                        )
                        continue

                    if message_type == "tts.voices":
                        await send_message(
                            websocket,
                            {
                                "type": "tts.voices.result",
                                "request_id": request_id,
                                "payload": {
                                    "voices": [
                                        {
                                            "id": "default",
                                            "display_name": "MeloTTS Default",
                                            "language": "auto",
                                            "capabilities": ["text", "wav"],
                                        }
                                    ]
                                },
                            },
                        )
                        continue

                    if message_type == "tts.start":
                        session = create_session(payload, config)
                        sessions[session.session_id] = session
                        await send_message(
                            websocket,
                            {
                                "type": "session.started",
                                "request_id": request_id,
                                "session_id": session.session_id,
                                "payload": {
                                    "domain": "tts",
                                    "provider_id": config.provider_id,
                                    "output_format": {
                                        "encoding": "wav",
                                    },
                                },
                            },
                        )
                        continue

                    if message_type in {"tts.input_text", "tts.append"}:
                        session = resolve_session(payload, sessions)
                        session.append_text(extract_text(payload))
                        await send_message(
                            websocket,
                            {
                                "type": "tts.input.accepted",
                                "session_id": session.session_id,
                                "payload": {
                                    "buffered_chars": len(session.full_text()),
                                },
                            },
                        )
                        continue

                    if message_type in {"tts.commit", "tts.flush"}:
                        session = resolve_session(payload, sessions)
                        session.append_text(extract_text(payload))
                        text = session.full_text()
                        if not text:
                            raise ValueError("tts text buffer is empty")

                        started = time.perf_counter()
                        result = await asyncio.to_thread(
                            synthesize_with_melotts, config, text, session.speed
                        )
                        chunk_count = 0
                        for chunk_count, chunk in enumerate(
                            chunk_audio(result.audio_wav, session.chunk_bytes),
                            start=1,
                        ):
                            is_final = chunk_count * session.chunk_bytes >= len(result.audio_wav)
                            await send_message(
                                websocket,
                                {
                                    "type": "tts.audio.chunk",
                                    "session_id": session.session_id,
                                    "sequence": chunk_count,
                                    "payload": {
                                        "data_base64": base64.b64encode(chunk).decode("ascii"),
                                        "mime_type": "audio/wav",
                                        "sample_rate_hz": result.sample_rate_hz,
                                        "is_final": is_final,
                                        "render_millis": result.render_millis,
                                        "speaker": result.speaker,
                                    },
                                },
                            )
                        bridge_elapsed_ms = int((time.perf_counter() - started) * 1000)
                        await send_message(
                            websocket,
                            {
                                "type": "session.ended",
                                "session_id": session.session_id,
                                "payload": {
                                    "reason": "completed",
                                    "chars": len(text),
                                    "chunks": chunk_count,
                                    "bridge_elapsed_millis": bridge_elapsed_ms,
                                },
                            },
                        )
                        sessions.pop(session.session_id, None)
                        continue

                    if message_type in {"session.stop", "tts.cancel"}:
                        session = resolve_session(payload, sessions)
                        sessions.pop(session.session_id, None)
                        await send_message(
                            websocket,
                            {
                                "type": "session.ended",
                                "session_id": session.session_id,
                                "payload": {"reason": "stopped"},
                            },
                        )
                        continue

                    raise ValueError(f"unsupported message type: {message_type}")
                except Exception as error:  # noqa: BLE001
                    await send_error(
                        websocket,
                        request_id=request_id,
                        session_id=payload.get("session_id") if isinstance(payload, dict) else None,
                        code="bad_request",
                        message=str(error),
                    )
        except WebSocketDisconnect:
            return

    return app


def decode_client_message(raw: str) -> tuple[Optional[str], str, dict[str, Any]]:
    try:
        parsed = json.loads(raw)
    except json.JSONDecodeError as error:
        raise ValueError(f"invalid json frame: {error}") from error
    if not isinstance(parsed, dict):
        raise ValueError("message must be an object")
    message_type = parsed.get("type")
    if not isinstance(message_type, str) or not message_type:
        raise ValueError("message type is required")
    payload = parsed.get("payload", {})
    if payload is None:
        payload = {}
    if not isinstance(payload, dict):
        raise ValueError("payload must be an object")
    request_id = parsed.get("request_id")
    if request_id is not None and not isinstance(request_id, str):
        raise ValueError("request_id must be a string")
    return request_id, message_type, payload


def create_session(payload: dict[str, Any], config: BridgeConfig) -> SessionState:
    session = SessionState(
        session_id=str(uuid.uuid4()),
        speed=extract_speed(payload, config.default_speed),
        chunk_bytes=extract_chunk_bytes(payload, config.default_chunk_bytes),
    )
    session.append_text(extract_text(payload))
    return session


def resolve_session(payload: dict[str, Any], sessions: dict[str, SessionState]) -> SessionState:
    session_id = payload.get("session_id")
    if not isinstance(session_id, str) or not session_id:
        raise ValueError("session_id is required")
    session = sessions.get(session_id)
    if session is None:
        raise ValueError(f"unknown session_id: {session_id}")
    return session


def extract_text(payload: dict[str, Any]) -> str:
    if "text" in payload and isinstance(payload["text"], str):
        return payload["text"]
    input_payload = payload.get("input")
    if isinstance(input_payload, str):
        return input_payload
    if isinstance(input_payload, dict):
        if isinstance(input_payload.get("text"), str):
            return input_payload["text"]
        if isinstance(input_payload.get("ssml"), str):
            return input_payload["ssml"]
    return ""


def extract_speed(payload: dict[str, Any], default_speed: float) -> float:
    options = payload.get("options")
    raw = None
    if isinstance(options, dict):
        raw = options.get("rate", options.get("speed"))
    if raw is None:
        raw = payload.get("speed")
    if isinstance(raw, (int, float)):
        return max(0.5, min(2.0, float(raw)))
    return default_speed


def extract_chunk_bytes(payload: dict[str, Any], default_size: int) -> int:
    options = payload.get("options")
    raw = None
    if isinstance(options, dict):
        raw = options.get("chunk_bytes")
    if isinstance(raw, int) and raw > 0:
        return min(raw, 1024 * 1024)
    return default_size


def check_melotts_health(config: BridgeConfig) -> tuple[bool, dict[str, Any]]:
    url = f"{config.melotts_base_url.rstrip('/')}/healthz"
    request = urllib.request.Request(url=url, method="GET")
    try:
        with urllib.request.urlopen(request, timeout=config.request_timeout_secs) as response:
            body = response.read().decode("utf-8")
            detail = json.loads(body)
            return True, detail
    except Exception as error:  # noqa: BLE001
        return False, {"error": str(error), "url": url}


def synthesize_with_melotts(config: BridgeConfig, text: str, speed: float) -> MeloTtsResult:
    url = f"{config.melotts_base_url.rstrip('/')}/v1/tts"
    payload = json.dumps({"text": text, "speed": speed}).encode("utf-8")
    request = urllib.request.Request(
        url=url,
        data=payload,
        headers={"Content-Type": "application/json"},
        method="POST",
    )
    try:
        with urllib.request.urlopen(request, timeout=config.request_timeout_secs) as response:
            audio_wav = response.read()
            render = parse_optional_int(response.headers.get("X-Render-Millis"))
            sample_rate = parse_optional_int(response.headers.get("X-Sample-Rate"))
            speaker = response.headers.get("X-Speaker")
            return MeloTtsResult(
                audio_wav=audio_wav,
                render_millis=render,
                sample_rate_hz=sample_rate,
                speaker=speaker,
            )
    except urllib.error.HTTPError as error:
        detail = error.read().decode("utf-8", errors="ignore")
        raise RuntimeError(f"MeloTTS HTTP {error.code}: {detail}") from error
    except urllib.error.URLError as error:
        raise RuntimeError(f"MeloTTS upstream unavailable: {error.reason}") from error


def parse_optional_int(value: Optional[str]) -> Optional[int]:
    if value is None:
        return None
    try:
        return int(value)
    except ValueError:
        return None


def chunk_audio(value: bytes, chunk_bytes: int) -> list[bytes]:
    if chunk_bytes <= 0:
        return [value]
    return [value[i : i + chunk_bytes] for i in range(0, len(value), chunk_bytes)]


async def send_message(websocket: WebSocket, message: dict[str, Any]) -> None:
    await websocket.send_text(json.dumps(message, ensure_ascii=True))


async def send_error(
    websocket: WebSocket,
    request_id: Optional[str],
    session_id: Optional[str],
    code: str,
    message: str,
) -> None:
    frame = {
        "type": "error",
        "request_id": request_id,
        "session_id": session_id,
        "payload": {
            "error": {
                "code": code,
                "message": message,
                "retryable": False,
            }
        },
    }
    await send_message(websocket, frame)


def main() -> None:
    args = parse_args()
    config = BridgeConfig(
        listen_host=args.listen_host,
        listen_port=args.listen_port,
        provider_id=args.provider_id,
        melotts_base_url=args.melotts_base_url,
        request_timeout_secs=args.request_timeout_secs,
        default_speed=args.default_speed,
        default_chunk_bytes=args.default_chunk_bytes,
    )
    app = create_app(config)
    uvicorn.run(
        app,
        host=config.listen_host,
        port=config.listen_port,
        log_level=args.log_level,
    )


if __name__ == "__main__":
    main()
