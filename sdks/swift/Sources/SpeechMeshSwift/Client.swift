import Foundation

public struct SpeechMeshClientConfig: Sendable {
    public var url: URL
    public var protocolVersion: String
    public var clientName: String

    public init(url: URL, protocolVersion: String = "v1", clientName: String = "speechmesh-swift-sdk") {
        self.url = url
        self.protocolVersion = protocolVersion
        self.clientName = clientName
    }
}

public enum SpeechMeshClientError: Error, LocalizedError, Sendable {
    case notConnected
    case alreadyConnected
    case handshakeFailed(String)
    case unsupportedServerMessage(String)
    case unexpectedBinaryFrame
    case invalidEnvelope
    case noActiveSession
    case sessionDomainMismatch(expected: CapabilityDomain, actual: CapabilityDomain)
    case invalidBase64Audio
    case server(ErrorInfo, requestID: String?, sessionID: String?)

    public var errorDescription: String? {
        switch self {
        case .notConnected:
            return "speechmesh websocket is not connected"
        case .alreadyConnected:
            return "speechmesh websocket is already connected"
        case .handshakeFailed(let message):
            return "speechmesh handshake failed: \(message)"
        case .unsupportedServerMessage(let type):
            return "unsupported speechmesh server message type: \(type)"
        case .unexpectedBinaryFrame:
            return "unexpected binary frame from speechmesh server"
        case .invalidEnvelope:
            return "invalid speechmesh message envelope"
        case .noActiveSession:
            return "no active speechmesh session"
        case .sessionDomainMismatch(let expected, let actual):
            return "active session domain \(actual.rawValue) does not match expected \(expected.rawValue)"
        case .invalidBase64Audio:
            return "audio payload is not valid base64"
        case .server(let info, let requestID, let sessionID):
            return "speechmesh server error request_id=\(requestID ?? "nil") session_id=\(sessionID ?? "nil") code=\(info.code) message=\(info.message)"
        }
    }
}

public enum SpeechMeshServerMessage: Sendable, Equatable {
    case helloOK(requestID: String?, payload: HelloResponse)
    case discoverResult(requestID: String, payload: DiscoverResult)
    case sessionStarted(requestID: String?, sessionID: String, payload: SessionStartedPayload)
    case asrResult(sessionID: String, sequence: UInt64, payload: AsrResultPayload)
    case ttsVoicesResult(requestID: String, payload: VoiceListResult)
    case ttsAudioDelta(sessionID: String, sequence: UInt64, payload: TtsAudioDeltaPayload)
    case ttsAudioDone(sessionID: String, sequence: UInt64, payload: TtsAudioDonePayload)
    case sessionEnded(sessionID: String, payload: SessionEndedPayload)
    case error(requestID: String?, sessionID: String?, payload: ErrorPayload)
    case pong(requestID: String?)

    public var requestID: String? {
        switch self {
        case .helloOK(let requestID, _):
            return requestID
        case .discoverResult(let requestID, _):
            return requestID
        case .sessionStarted(let requestID, _, _):
            return requestID
        case .ttsVoicesResult(let requestID, _):
            return requestID
        case .error(let requestID, _, _):
            return requestID
        case .pong(let requestID):
            return requestID
        case .asrResult, .ttsAudioDelta, .ttsAudioDone, .sessionEnded:
            return nil
        }
    }

    public var sessionID: String? {
        switch self {
        case .sessionStarted(_, let sessionID, _):
            return sessionID
        case .asrResult(let sessionID, _, _):
            return sessionID
        case .ttsAudioDelta(let sessionID, _, _):
            return sessionID
        case .ttsAudioDone(let sessionID, _, _):
            return sessionID
        case .sessionEnded(let sessionID, _):
            return sessionID
        case .error(_, let sessionID, _):
            return sessionID
        case .helloOK, .discoverResult, .ttsVoicesResult, .pong:
            return nil
        }
    }
}

public actor SpeechMeshClient {
    private let config: SpeechMeshClientConfig
    private let session: URLSession
    private let encoder = JSONEncoder()
    private let decoder = JSONDecoder()
    private var socket: URLSessionWebSocketTask?
    private var nextRequestID: UInt64 = 0
    private var activeSessionID: String?
    private var activeSessionDomain: CapabilityDomain?

    public init(config: SpeechMeshClientConfig, session: URLSession = .shared) {
        self.config = config
        self.session = session
    }

    public func connect() async throws {
        guard socket == nil else {
            throw SpeechMeshClientError.alreadyConnected
        }
        let task = session.webSocketTask(with: config.url)
        task.resume()
        socket = task
        do {
            try await send(type: "hello", requestID: nil, sessionID: nil, payload: HelloRequest(protocolVersion: config.protocolVersion, clientName: config.clientName))
            while true {
                let message = try await receive()
                switch message {
                case .helloOK:
                    return
                case .error(let requestID, let sessionID, let payload):
                    throw SpeechMeshClientError.server(payload.error, requestID: requestID, sessionID: sessionID)
                default:
                    continue
                }
            }
        } catch {
            socket = nil
            task.cancel(with: .protocolError, reason: nil)
            throw error
        }
    }

    public func close() {
        socket?.cancel(with: .normalClosure, reason: nil)
        socket = nil
        activeSessionID = nil
        activeSessionDomain = nil
    }

    public func discover(domains: [CapabilityDomain]) async throws -> DiscoverResult {
        let requestID = nextID()
        try await send(type: "discover", requestID: requestID, sessionID: nil, payload: DiscoverRequest(domains: domains))
        while true {
            let message = try await receive()
            switch message {
            case .discoverResult(let incomingID, let payload) where incomingID == requestID:
                return payload
            case .error(let incomingID, let sessionID, let payload) where incomingID == requestID:
                throw SpeechMeshClientError.server(payload.error, requestID: incomingID, sessionID: sessionID)
            default:
                continue
            }
        }
    }

    public func discoverASR() async throws -> DiscoverResult {
        try await discover(domains: [.asr])
    }

    public func discoverTTS() async throws -> DiscoverResult {
        try await discover(domains: [.tts])
    }

    public func startASR(_ request: AsrStreamRequest) async throws -> (sessionID: String, started: SessionStartedPayload) {
        try await startSession(type: "asr.start", domain: .asr, payload: request)
    }

    public func startTTS(_ request: TtsStreamRequest) async throws -> (sessionID: String, started: SessionStartedPayload) {
        try await startSession(type: "tts.start", domain: .tts, payload: request)
    }

    public func listTTSVoices(_ request: VoiceListRequest) async throws -> VoiceListResult {
        let requestID = nextID()
        try await send(type: "tts.voices", requestID: requestID, sessionID: nil, payload: request)
        while true {
            let message = try await receive()
            switch message {
            case .ttsVoicesResult(let incomingID, let payload) where incomingID == requestID:
                return payload
            case .error(let incomingID, let sessionID, let payload) where incomingID == requestID:
                throw SpeechMeshClientError.server(payload.error, requestID: incomingID, sessionID: sessionID)
            default:
                continue
            }
        }
    }

    public func sendAudio(_ chunk: Data) async throws {
        try ensureActiveSession(domain: .asr)
        guard let socket else {
            throw SpeechMeshClientError.notConnected
        }
        try await socket.send(.data(chunk))
    }

    public func appendTTSInput(_ delta: String) async throws {
        try ensureActiveSession(domain: .tts)
        try await send(type: "tts.input.append", requestID: nil, sessionID: activeSessionID, payload: TtsInputAppendPayload(delta: delta))
    }

    public func commit() async throws {
        guard let domain = activeSessionDomain else {
            throw SpeechMeshClientError.noActiveSession
        }
        let type = domain == .tts ? "tts.commit" : "asr.commit"
        try await send(type: type, requestID: nil, sessionID: activeSessionID, payload: EmptyPayload())
    }

    public func stop() async throws {
        guard activeSessionID != nil else {
            throw SpeechMeshClientError.noActiveSession
        }
        try await send(type: "session.stop", requestID: nil, sessionID: activeSessionID, payload: EmptyPayload())
    }

    public func ping() async throws {
        try await send(type: "ping", requestID: nextID(), sessionID: nil, payload: EmptyPayload())
    }

    public func receive() async throws -> SpeechMeshServerMessage {
        guard let socket else {
            throw SpeechMeshClientError.notConnected
        }
        let raw: RawServerEnvelope
        switch try await socket.receive() {
        case .string(let text):
            guard let data = text.data(using: .utf8) else {
                throw SpeechMeshClientError.invalidEnvelope
            }
            raw = try decoder.decode(RawServerEnvelope.self, from: data)
        case .data:
            throw SpeechMeshClientError.unexpectedBinaryFrame
        @unknown default:
            throw SpeechMeshClientError.invalidEnvelope
        }

        let message = try decodeServerMessage(from: raw)
        if case .sessionStarted(_, let sessionID, let payload) = message {
            activeSessionID = sessionID
            activeSessionDomain = payload.domain
        }
        if case .sessionEnded(let sessionID, _) = message, activeSessionID == sessionID {
            activeSessionID = nil
            activeSessionDomain = nil
        }
        return message
    }

    public func activeSession() -> (sessionID: String, domain: CapabilityDomain)? {
        guard let activeSessionID, let activeSessionDomain else {
            return nil
        }
        return (activeSessionID, activeSessionDomain)
    }

    private func startSession<T: Encodable>(type: String, domain: CapabilityDomain, payload: T) async throws -> (sessionID: String, started: SessionStartedPayload) {
        if activeSessionID != nil {
            throw SpeechMeshClientError.handshakeFailed("speechmesh allows only one active session per connection")
        }
        let requestID = nextID()
        try await send(type: type, requestID: requestID, sessionID: nil, payload: payload)
        while true {
            let message = try await receive()
            switch message {
            case .sessionStarted(let incomingID, let sessionID, let payload) where incomingID == requestID:
                activeSessionID = sessionID
                activeSessionDomain = payload.domain
                return (sessionID, payload)
            case .error(let incomingID, let sessionID, let payload) where incomingID == requestID:
                throw SpeechMeshClientError.server(payload.error, requestID: incomingID, sessionID: sessionID)
            default:
                continue
            }
        }
    }

    private func ensureActiveSession(domain expectedDomain: CapabilityDomain) throws {
        guard let activeSessionID = activeSessionID, let activeSessionDomain = activeSessionDomain else {
            throw SpeechMeshClientError.noActiveSession
        }
        _ = activeSessionID
        guard activeSessionDomain == expectedDomain else {
            throw SpeechMeshClientError.sessionDomainMismatch(expected: expectedDomain, actual: activeSessionDomain)
        }
    }

    private func send<T: Encodable>(type: String, requestID: String?, sessionID: String?, payload: T) async throws {
        guard let socket else {
            throw SpeechMeshClientError.notConnected
        }
        let envelope = RawClientEnvelope(type: type, requestID: requestID, sessionID: sessionID, payload: payload)
        let data = try encoder.encode(envelope)
        guard let text = String(data: data, encoding: .utf8) else {
            throw SpeechMeshClientError.invalidEnvelope
        }
        try await socket.send(.string(text))
    }

    private func decodeServerMessage(from raw: RawServerEnvelope) throws -> SpeechMeshServerMessage {
        switch raw.type {
        case "hello.ok":
            return .helloOK(requestID: raw.requestID, payload: try raw.decodePayload(HelloResponse.self, using: decoder))
        case "discover.result":
            guard let requestID = raw.requestID else { throw SpeechMeshClientError.invalidEnvelope }
            return .discoverResult(requestID: requestID, payload: try raw.decodePayload(DiscoverResult.self, using: decoder))
        case "session.started":
            guard let sessionID = raw.sessionID else { throw SpeechMeshClientError.invalidEnvelope }
            return .sessionStarted(requestID: raw.requestID, sessionID: sessionID, payload: try raw.decodePayload(SessionStartedPayload.self, using: decoder))
        case "asr.result":
            guard let sessionID = raw.sessionID else { throw SpeechMeshClientError.invalidEnvelope }
            return .asrResult(sessionID: sessionID, sequence: raw.sequence ?? 0, payload: try raw.decodePayload(AsrResultPayload.self, using: decoder))
        case "tts.voices.result":
            guard let requestID = raw.requestID else { throw SpeechMeshClientError.invalidEnvelope }
            return .ttsVoicesResult(requestID: requestID, payload: try raw.decodePayload(VoiceListResult.self, using: decoder))
        case "tts.audio.delta":
            guard let sessionID = raw.sessionID else { throw SpeechMeshClientError.invalidEnvelope }
            return .ttsAudioDelta(sessionID: sessionID, sequence: raw.sequence ?? 0, payload: try raw.decodePayload(TtsAudioDeltaPayload.self, using: decoder))
        case "tts.audio.done":
            guard let sessionID = raw.sessionID else { throw SpeechMeshClientError.invalidEnvelope }
            return .ttsAudioDone(sessionID: sessionID, sequence: raw.sequence ?? 0, payload: try raw.decodePayload(TtsAudioDonePayload.self, using: decoder))
        case "session.ended":
            guard let sessionID = raw.sessionID else { throw SpeechMeshClientError.invalidEnvelope }
            return .sessionEnded(sessionID: sessionID, payload: try raw.decodePayload(SessionEndedPayload.self, using: decoder))
        case "error":
            return .error(requestID: raw.requestID, sessionID: raw.sessionID, payload: try raw.decodePayload(ErrorPayload.self, using: decoder))
        case "pong":
            return .pong(requestID: raw.requestID)
        default:
            throw SpeechMeshClientError.unsupportedServerMessage(raw.type)
        }
    }

    private func nextID() -> String {
        nextRequestID += 1
        return "req_\(nextRequestID)"
    }
}

public struct EmptyPayload: Codable, Equatable, Sendable {
    public init() {}
}

private struct RawClientEnvelope<Payload: Encodable>: Encodable {
    let type: String
    let requestID: String?
    let sessionID: String?
    let payload: Payload

    enum CodingKeys: String, CodingKey {
        case type
        case requestID = "request_id"
        case sessionID = "session_id"
        case payload
    }
}

private struct RawServerEnvelope: Decodable {
    let type: String
    let requestID: String?
    let sessionID: String?
    let sequence: UInt64?
    let payload: Data

    enum CodingKeys: String, CodingKey {
        case type
        case requestID = "request_id"
        case sessionID = "session_id"
        case sequence
        case payload
    }

    init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        type = try container.decode(String.self, forKey: .type)
        requestID = try container.decodeIfPresent(String.self, forKey: .requestID)
        sessionID = try container.decodeIfPresent(String.self, forKey: .sessionID)
        sequence = try container.decodeIfPresent(UInt64.self, forKey: .sequence)
        if let nested = try? container.decode(JSONValue.self, forKey: .payload) {
            payload = try JSONEncoder().encode(nested)
        } else {
            payload = Data("null".utf8)
        }
    }

    func decodePayload<T: Decodable>(_ type: T.Type, using decoder: JSONDecoder) throws -> T {
        try decoder.decode(T.self, from: payload)
    }
}
