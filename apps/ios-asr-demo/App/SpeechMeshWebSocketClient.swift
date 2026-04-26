import Foundation

struct SessionStartedPayload: Decodable {
    let sessionID: String

    enum CodingKeys: String, CodingKey {
        case sessionID = "session_id"
    }
}

struct AsrResultMessage: Decodable {
    let type: String
    let sessionID: String?
    let payload: AsrResultPayload?

    enum CodingKeys: String, CodingKey {
        case type
        case sessionID = "session_id"
        case payload
    }
}

struct AsrResultPayload: Decodable {
    let text: String
    let revision: Int?
    let isFinal: Bool?
    let speechFinal: Bool?

    enum CodingKeys: String, CodingKey {
        case text
        case revision
        case isFinal = "is_final"
        case speechFinal = "speech_final"
    }
}

struct ErrorMessage: Decodable {
    struct Payload: Decodable {
        struct ErrorBody: Decodable {
            let message: String
        }
        let error: ErrorBody
    }

    let type: String
    let payload: Payload
}

final class SpeechMeshWebSocketClient {
    enum Event {
        case sessionStarted(String)
        case asrResult(AsrResultPayload)
        case sessionEnded
        case failure(String)
    }

    var onEvent: ((Event) -> Void)?

    private let url: URL
    private let providerID: String
    private let language: String
    private let urlSession: URLSession
    private var task: URLSessionWebSocketTask?

    init(url: URL, providerID: String, language: String) {
        self.url = url
        self.providerID = providerID
        self.language = language
        self.urlSession = URLSession(configuration: .default)
    }

    func connectAndStartASR() async throws {
        let task = urlSession.webSocketTask(with: url)
        self.task = task
        task.resume()
        receiveNext()
        try await sendJSON([
            "type": "hello",
            "payload": [
                "protocol_version": "v1",
                "client_name": "speechmesh-ios-asr-demo"
            ]
        ])
        try await sendJSON([
            "type": "asr.start",
            "request_id": UUID().uuidString,
            "payload": [
                "provider": [
                    "mode": "provider",
                    "provider_id": providerID,
                    "required_capabilities": ["streaming-input"],
                    "preferred_capabilities": []
                ],
                "input_format": [
                    "encoding": "pcm_s16le",
                    "sample_rate_hz": 16000,
                    "channels": 1
                ],
                "options": [
                    "language": language,
                    "hints": [],
                    "interim_results": true,
                    "timestamps": false,
                    "punctuation": true,
                    "prefer_on_device": false
                ]
            ]
        ])
    }

    func sendAudio(_ data: Data) async throws {
        try await task?.send(.data(data))
    }

    func commit(sessionID: String) async throws {
        try await sendJSON([
            "type": "asr.commit",
            "session_id": sessionID,
            "payload": [:]
        ])
    }

    func close() {
        task?.cancel(with: .normalClosure, reason: nil)
        task = nil
    }

    private func sendJSON(_ object: [String: Any]) async throws {
        let data = try JSONSerialization.data(withJSONObject: object, options: [])
        guard let text = String(data: data, encoding: .utf8) else {
            throw URLError(.cannotParseResponse)
        }
        try await task?.send(.string(text))
    }

    private func receiveNext() {
        task?.receive { [weak self] result in
            guard let self else { return }
            switch result {
            case .failure(let error):
                self.onEvent?(.failure(error.localizedDescription))
            case .success(let message):
                self.handle(message)
                self.receiveNext()
            }
        }
    }

    private func handle(_ message: URLSessionWebSocketTask.Message) {
        guard case .string(let text) = message else {
            return
        }
        guard let data = text.data(using: .utf8),
              let raw = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
              let type = raw["type"] as? String else {
            return
        }

        switch type {
        case "session.started":
            if let sessionID = raw["session_id"] as? String {
                onEvent?(.sessionStarted(sessionID))
            }
        case "asr.result":
            if let decoded = try? JSONDecoder().decode(AsrResultMessage.self, from: data),
               let payload = decoded.payload {
                onEvent?(.asrResult(payload))
            }
        case "session.ended":
            onEvent?(.sessionEnded)
        case "error":
            if let decoded = try? JSONDecoder().decode(ErrorMessage.self, from: data) {
                onEvent?(.failure(decoded.payload.error.message))
            }
        default:
            break
        }
    }
}
