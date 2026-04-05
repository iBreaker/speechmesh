import AVFoundation
import Foundation
@preconcurrency import Speech

enum BridgeError: Error, LocalizedError {
    case invalidMessage(String)
    case unsupportedMessageType(String)
    case invalidAudioFormat(String)
    case invalidAudioData(String)
    case sessionAlreadyExists(String)
    case sessionNotFound(String)
    case recognizerUnavailable(String)
    case authorizationDenied(String)

    var errorDescription: String? {
        switch self {
        case .invalidMessage(let message):
            return message
        case .unsupportedMessageType(let message):
            return message
        case .invalidAudioFormat(let message):
            return message
        case .invalidAudioData(let message):
            return message
        case .sessionAlreadyExists(let message):
            return message
        case .sessionNotFound(let message):
            return message
        case .recognizerUnavailable(let message):
            return message
        case .authorizationDenied(let message):
            return message
        }
    }
}

struct EmptyPayload: Codable {}

struct InputFormat: Codable {
    let encoding: String
    let sampleRateHz: Int
    let channels: Int

    enum CodingKeys: String, CodingKey {
        case encoding
        case sampleRateHz = "sample_rate_hz"
        case channels
    }
}

struct StartPayload: Codable {
    let locale: String?
    let shouldReportPartials: Bool?
    let requiresOnDevice: Bool?
    let inputFormat: InputFormat?

    enum CodingKeys: String, CodingKey {
        case locale
        case shouldReportPartials = "should_report_partials"
        case requiresOnDevice = "requires_on_device"
        case inputFormat = "input_format"
    }
}

struct AudioPayload: Codable {
    let dataBase64: String

    enum CodingKeys: String, CodingKey {
        case dataBase64 = "data_base64"
    }
}

struct TextCommand<T: Codable>: Codable {
    let type: String
    let requestId: String?
    let sessionId: String?
    let payload: T

    enum CodingKeys: String, CodingKey {
        case type
        case requestId = "request_id"
        case sessionId = "session_id"
        case payload
    }
}

struct Event<T: Encodable>: Encodable {
    let type: String
    let requestId: String?
    let sessionId: String?
    let payload: T

    enum CodingKeys: String, CodingKey {
        case type
        case requestId = "request_id"
        case sessionId = "session_id"
        case payload
    }
}

struct ErrorPayload: Codable {
    let code: String
    let message: String
    let details: [String: String]
}

struct StartedPayload: Codable {
    let providerId: String
    let locale: String
    let inputFormat: InputFormat

    enum CodingKeys: String, CodingKey {
        case providerId = "provider_id"
        case locale
        case inputFormat = "input_format"
    }
}

struct PartialPayload: Codable {
    let text: String
    let sequence: UInt64
    let isFinal: Bool

    enum CodingKeys: String, CodingKey {
        case text
        case sequence
        case isFinal = "is_final"
    }
}

struct FinalPayload: Codable {
    let text: String
    let sequence: UInt64
    let isFinal: Bool
    let segments: [TranscriptSegment]

    enum CodingKeys: String, CodingKey {
        case text
        case sequence
        case isFinal = "is_final"
        case segments
    }
}

struct TranscriptSegment: Codable {
    let substring: String
    let timestampS: Double
    let durationS: Double
    let confidence: Float

    enum CodingKeys: String, CodingKey {
        case substring
        case timestampS = "timestamp_s"
        case durationS = "duration_s"
        case confidence
    }
}

struct EndedPayload: Codable {
    let reason: String
}

struct AuthResultPayload: Codable {
    let status: String
    let authorized: Bool
}

func requestSpeechAuthorizationStatus() async -> SFSpeechRecognizerAuthorizationStatus {
    await withCheckedContinuation { continuation in
        SFSpeechRecognizer.requestAuthorization { status in
            continuation.resume(returning: status)
        }
    }
}

actor LineWriter {
    private let encoder = JSONEncoder()
    private let stdout = FileHandle.standardOutput

    func send<T: Encodable>(_ value: T) {
        do {
            let data = try encoder.encode(AnyEncodable(value))
            stdout.write(data)
            stdout.write(Data([0x0A]))
        } catch {
            let fallback = "{\"type\":\"error\",\"payload\":{\"code\":\"internal_error\",\"message\":\"failed to encode output\",\"details\":{}}}\n"
            stdout.write(fallback.data(using: .utf8)!)
        }
    }
}

struct AnyEncodable: Encodable {
    private let encodeImpl: (Encoder) throws -> Void

    init<T: Encodable>(_ value: T) {
        self.encodeImpl = value.encode
    }

    func encode(to encoder: Encoder) throws {
        try encodeImpl(encoder)
    }
}

@MainActor
final class RecognitionSession {
    let sessionId: String
    let locale: String
    let inputFormat: InputFormat

    private let writer: LineWriter
    private let request: SFSpeechAudioBufferRecognitionRequest
    private let recognizer: SFSpeechRecognizer
    private var recognitionTask: SFSpeechRecognitionTask?
    private var sequence: UInt64 = 0
    private var ended = false
    private let onEnd: @MainActor (String) -> Void

    init(
        sessionId: String,
        locale: String,
        inputFormat: InputFormat,
        recognizer: SFSpeechRecognizer,
        request: SFSpeechAudioBufferRecognitionRequest,
        writer: LineWriter,
        onEnd: @escaping @MainActor (String) -> Void
    ) {
        self.sessionId = sessionId
        self.locale = locale
        self.inputFormat = inputFormat
        self.recognizer = recognizer
        self.request = request
        self.writer = writer
        self.onEnd = onEnd
    }

    func start(shouldReportPartials: Bool, requiresOnDevice: Bool) {
        request.shouldReportPartialResults = shouldReportPartials
        request.requiresOnDeviceRecognition = requiresOnDevice

        recognitionTask = recognizer.recognitionTask(with: request) { [weak self] result, error in
            guard let self else { return }
            Task { @MainActor in
                await self.handleRecognition(result: result, error: error)
            }
        }
    }

    func appendAudioChunk(_ data: Data) throws {
        let buffer = try Self.makePCMBuffer(data: data, format: inputFormat)
        request.append(buffer)
    }

    func commit() {
        request.endAudio()
    }

    func cancel(reason: String) async {
        recognitionTask?.cancel()
        await end(reason: reason)
    }

    private func handleRecognition(result: SFSpeechRecognitionResult?, error: Error?) async {
        if let error {
            await writer.send(
                Event(
                    type: "error",
                    requestId: nil,
                    sessionId: sessionId,
                    payload: ErrorPayload(
                        code: "provider_error",
                        message: error.localizedDescription,
                        details: [:]
                    )
                )
            )
            await end(reason: "error")
            return
        }

        guard let result else { return }

        sequence += 1
        if result.isFinal {
            await writer.send(
                Event(
                    type: "asr.final",
                    requestId: nil,
                    sessionId: sessionId,
                    payload: FinalPayload(
                        text: result.bestTranscription.formattedString,
                        sequence: sequence,
                        isFinal: true,
                        segments: result.bestTranscription.segments.map {
                            TranscriptSegment(
                                substring: $0.substring,
                                timestampS: $0.timestamp,
                                durationS: $0.duration,
                                confidence: $0.confidence
                            )
                        }
                    )
                )
            )
            await end(reason: "final")
            return
        }

        await writer.send(
            Event(
                type: "asr.partial",
                requestId: nil,
                sessionId: sessionId,
                payload: PartialPayload(
                    text: result.bestTranscription.formattedString,
                    sequence: sequence,
                    isFinal: false
                )
            )
        )
    }

    private func end(reason: String) async {
        if ended {
            return
        }
        ended = true
        recognitionTask?.cancel()
        await writer.send(
            Event(
                type: "asr.ended",
                requestId: nil,
                sessionId: sessionId,
                payload: EndedPayload(reason: reason)
            )
        )
        onEnd(sessionId)
    }

    private static func makePCMBuffer(data: Data, format: InputFormat) throws -> AVAudioPCMBuffer {
        guard format.encoding == "pcm_s16le" else {
            throw BridgeError.invalidAudioFormat("unsupported encoding: \(format.encoding)")
        }
        guard format.channels == 1 else {
            throw BridgeError.invalidAudioFormat("only mono audio is supported in v1")
        }
        guard format.sampleRateHz > 0 else {
            throw BridgeError.invalidAudioFormat("invalid sample_rate_hz")
        }
        guard data.count % 2 == 0 else {
            throw BridgeError.invalidAudioData("pcm_s16le chunk must be aligned to 2-byte frames")
        }

        guard let audioFormat = AVAudioFormat(
            commonFormat: .pcmFormatInt16,
            sampleRate: Double(format.sampleRateHz),
            channels: AVAudioChannelCount(format.channels),
            interleaved: true
        ) else {
            throw BridgeError.invalidAudioFormat("failed to create AVAudioFormat")
        }

        let frameCount = AVAudioFrameCount(data.count / 2)
        guard let buffer = AVAudioPCMBuffer(pcmFormat: audioFormat, frameCapacity: frameCount) else {
            throw BridgeError.invalidAudioData("failed to allocate AVAudioPCMBuffer")
        }
        buffer.frameLength = frameCount

        guard let channelData = buffer.int16ChannelData else {
            throw BridgeError.invalidAudioData("failed to access channel data")
        }

        data.withUnsafeBytes { rawBuffer in
            guard let src = rawBuffer.baseAddress else { return }
            memcpy(channelData[0], src, data.count)
        }
        return buffer
    }
}

private struct CommandEnvelope: Decodable {
    let type: String
}

@MainActor
final class BridgeController {
    private let writer = LineWriter()
    private var sessions: [String: RecognitionSession] = [:]
    private var shouldShutdown = false

    func run() async {
        await writer.send(
            Event(
                type: "bridge.ready",
                requestId: nil,
                sessionId: nil,
                payload: ["name": "apple-asr-bridge", "version": "0.1.0", "protocol": "ndjson-v1"]
            )
        )

        for await line in Self.stdinLines() {
            if shouldShutdown {
                break
            }
            do {
                try await handle(line: line)
            } catch {
                await writer.send(
                    Event(
                        type: "error",
                        requestId: nil,
                        sessionId: nil,
                        payload: ErrorPayload(
                            code: "invalid_request",
                            message: error.localizedDescription,
                            details: [:]
                        )
                    )
                )
            }
        }
    }

    private func handle(line: String) async throws {
        guard let data = line.data(using: .utf8) else {
            throw BridgeError.invalidMessage("stdin line is not utf-8")
        }

        let envelope = try JSONDecoder().decode(CommandEnvelope.self, from: data)
        switch envelope.type {
        case "hello":
            let command = try JSONDecoder().decode(TextCommand<EmptyPayload>.self, from: data)
            await hello(command)
        case "auth.request":
            let command = try JSONDecoder().decode(TextCommand<EmptyPayload>.self, from: data)
            await authRequest(command)
        case "asr.start":
            let command = try JSONDecoder().decode(TextCommand<StartPayload>.self, from: data)
            try await asrStart(command)
        case "asr.audio":
            let command = try JSONDecoder().decode(TextCommand<AudioPayload>.self, from: data)
            try await asrAudio(command)
        case "asr.commit":
            let command = try JSONDecoder().decode(TextCommand<EmptyPayload>.self, from: data)
            try await asrCommit(command)
        case "asr.stop":
            let command = try JSONDecoder().decode(TextCommand<EmptyPayload>.self, from: data)
            try await asrStop(command)
        case "ping":
            let command = try JSONDecoder().decode(TextCommand<EmptyPayload>.self, from: data)
            await pong(command)
        case "shutdown":
            let command = try JSONDecoder().decode(TextCommand<EmptyPayload>.self, from: data)
            await shutdown(command)
        default:
            throw BridgeError.unsupportedMessageType("unsupported type: \(envelope.type)")
        }
    }

    private func hello(_ command: TextCommand<EmptyPayload>) async {
        await writer.send(
            Event(
                type: "hello.ok",
                requestId: command.requestId,
                sessionId: nil,
                payload: [
                    "name": "apple-asr-bridge",
                    "version": "0.1.0",
                    "provider_id": "apple.asr",
                    "protocol": "ndjson-v1",
                    "audio_input": "base64 pcm_s16le",
                ]
            )
        )
    }

    private func authRequest(_ command: TextCommand<EmptyPayload>) async {
        let status = await requestSpeechAuthorizationStatus()
        await writer.send(
            Event(
                type: "auth.result",
                requestId: command.requestId,
                sessionId: nil,
                payload: AuthResultPayload(
                    status: statusName(status),
                    authorized: status == .authorized
                )
            )
        )
    }

    private func asrStart(_ command: TextCommand<StartPayload>) async throws {
        let status = await requestSpeechAuthorizationStatus()
        guard status == .authorized else {
            throw BridgeError.authorizationDenied("speech authorization status: \(statusName(status))")
        }

        guard let sessionId = command.sessionId, !sessionId.isEmpty else {
            throw BridgeError.invalidMessage("asr.start requires non-empty session_id")
        }
        if sessions[sessionId] != nil {
            throw BridgeError.sessionAlreadyExists("session already exists: \(sessionId)")
        }

        let locale = command.payload.locale ?? "en-US"
        guard let recognizer = SFSpeechRecognizer(locale: Locale(identifier: locale)), recognizer.isAvailable else {
            throw BridgeError.recognizerUnavailable("speech recognizer is unavailable for locale: \(locale)")
        }

        let format = command.payload.inputFormat ?? InputFormat(encoding: "pcm_s16le", sampleRateHz: 16_000, channels: 1)
        let request = SFSpeechAudioBufferRecognitionRequest()
        let shouldReportPartials = command.payload.shouldReportPartials ?? true
        let requiresOnDevice = command.payload.requiresOnDevice ?? false

        let session = RecognitionSession(
            sessionId: sessionId,
            locale: locale,
            inputFormat: format,
            recognizer: recognizer,
            request: request,
            writer: writer,
            onEnd: { [weak self] endedId in
                self?.sessions.removeValue(forKey: endedId)
            }
        )
        sessions[sessionId] = session
        session.start(shouldReportPartials: shouldReportPartials, requiresOnDevice: requiresOnDevice)

        await writer.send(
            Event(
                type: "asr.started",
                requestId: command.requestId,
                sessionId: sessionId,
                payload: StartedPayload(
                    providerId: "apple.asr",
                    locale: locale,
                    inputFormat: format
                )
            )
        )
    }

    private func asrAudio(_ command: TextCommand<AudioPayload>) async throws {
        guard let sessionId = command.sessionId else {
            throw BridgeError.invalidMessage("asr.audio requires session_id")
        }
        guard let session = sessions[sessionId] else {
            throw BridgeError.sessionNotFound("session not found: \(sessionId)")
        }
        guard let data = Data(base64Encoded: command.payload.dataBase64) else {
            throw BridgeError.invalidAudioData("payload.data_base64 is not valid base64")
        }

        try await Task { @MainActor in
            try session.appendAudioChunk(data)
        }.value
    }

    private func asrCommit(_ command: TextCommand<EmptyPayload>) async throws {
        guard let sessionId = command.sessionId else {
            throw BridgeError.invalidMessage("asr.commit requires session_id")
        }
        guard let session = sessions[sessionId] else {
            throw BridgeError.sessionNotFound("session not found: \(sessionId)")
        }
        session.commit()
        await writer.send(
            Event(
                type: "asr.committed",
                requestId: command.requestId,
                sessionId: sessionId,
                payload: EmptyPayload()
            )
        )
    }

    private func asrStop(_ command: TextCommand<EmptyPayload>) async throws {
        guard let sessionId = command.sessionId else {
            throw BridgeError.invalidMessage("asr.stop requires session_id")
        }
        guard let session = sessions[sessionId] else {
            throw BridgeError.sessionNotFound("session not found: \(sessionId)")
        }
        await session.cancel(reason: "stopped")
        sessions.removeValue(forKey: sessionId)
    }

    private func pong(_ command: TextCommand<EmptyPayload>) async {
        await writer.send(
            Event(
                type: "pong",
                requestId: command.requestId,
                sessionId: command.sessionId,
                payload: EmptyPayload()
            )
        )
    }

    private func shutdown(_ command: TextCommand<EmptyPayload>) async {
        shouldShutdown = true
        let runningSessions = Array(sessions.values)
        for session in runningSessions {
            await session.cancel(reason: "shutdown")
        }
        sessions.removeAll()
        await writer.send(
            Event(
                type: "shutdown.ok",
                requestId: command.requestId,
                sessionId: nil,
                payload: EmptyPayload()
            )
        )
    }

    private func statusName(_ status: SFSpeechRecognizerAuthorizationStatus) -> String {
        switch status {
        case .notDetermined:
            return "not_determined"
        case .denied:
            return "denied"
        case .restricted:
            return "restricted"
        case .authorized:
            return "authorized"
        @unknown default:
            return "unknown"
        }
    }

    nonisolated private static func stdinLines() -> AsyncStream<String> {
        AsyncStream { continuation in
            DispatchQueue.global(qos: .userInitiated).async {
                while let line = readLine(strippingNewline: true) {
                    let trimmed = line.trimmingCharacters(in: .whitespacesAndNewlines)
                    if trimmed.isEmpty {
                        continue
                    }
                    continuation.yield(trimmed)
                }
                continuation.finish()
            }
        }
    }
}

@main
struct AppleASRBridgeMain {
    static func main() async {
        let controller = await MainActor.run { BridgeController() }
        await controller.run()
    }
}
