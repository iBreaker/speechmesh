import AVFoundation
import Foundation

@MainActor
final class ASRViewModel: ObservableObject {
    @Published var gatewayURL = "ws://127.0.0.1:8080/ws"
    @Published var providerID = "minimax.asr"
    @Published var language = "zh-CN"
    @Published var status = "Idle"
    @Published var transcript = ""
    @Published var isRecording = false
    @Published var lastError: String?

    private let audioEngine = AVAudioEngine()
    private var audioConverter: AVAudioConverter?
    private var websocket: SpeechMeshWebSocketClient?
    private var sessionID: String?
    private var didInstallTap = false

    func beginPress() {
        Task {
            await startRecording()
        }
    }

    func endPress() {
        Task {
            await stopRecording()
        }
    }

    private func startRecording() async {
        guard !isRecording else { return }
        lastError = nil
        transcript = ""

        do {
            try configureAudioSession()
            try await setupWebSocket()
            try installTapIfNeeded()
            audioEngine.prepare()
            try audioEngine.start()
            isRecording = true
            status = sessionID == nil ? "Connecting" : "Recording"
        } catch {
            lastError = error.localizedDescription
            status = "Failed"
            teardownAudio()
            websocket?.close()
            websocket = nil
        }
    }

    private func stopRecording() async {
        guard isRecording else { return }
        isRecording = false
        teardownAudio()
        status = "Committing"
        if let websocket, let sessionID {
            do {
                try await websocket.commit(sessionID: sessionID)
            } catch {
                lastError = error.localizedDescription
                status = "Commit failed"
                websocket.close()
                self.websocket = nil
            }
        }
    }

    private func configureAudioSession() throws {
        let session = AVAudioSession.sharedInstance()
        try session.setCategory(.record, mode: .measurement, options: [.duckOthers])
        try session.setPreferredSampleRate(48_000)
        try session.setActive(true)
    }

    private func setupWebSocket() async throws {
        guard let url = URL(string: gatewayURL) else {
            throw URLError(.badURL)
        }
        let client = SpeechMeshWebSocketClient(url: url, providerID: providerID, language: language)
        client.onEvent = { [weak self] event in
            guard let self else { return }
            Task { @MainActor in
                self.handleEvent(event)
            }
        }
        websocket = client
        try await client.connectAndStartASR()
    }

    private func installTapIfNeeded() throws {
        guard !didInstallTap else { return }

        let inputNode = audioEngine.inputNode
        let inputFormat = inputNode.inputFormat(forBus: 0)
        guard let outputFormat = AVAudioFormat(
            commonFormat: .pcmFormatInt16,
            sampleRate: 16_000,
            channels: 1,
            interleaved: true
        ) else {
            throw NSError(domain: "SpeechMeshASRDemo", code: -1, userInfo: [NSLocalizedDescriptionKey: "failed to create target audio format"])
        }
        guard let converter = AVAudioConverter(from: inputFormat, to: outputFormat) else {
            throw NSError(domain: "SpeechMeshASRDemo", code: -1, userInfo: [NSLocalizedDescriptionKey: "failed to create audio converter"])
        }
        self.audioConverter = converter

        inputNode.installTap(onBus: 0, bufferSize: 2048, format: inputFormat) { [weak self] buffer, _ in
            guard let self,
                  self.isRecording,
                  let data = Self.convertToPCM16Mono(buffer: buffer, converter: converter) else {
                return
            }
            Task {
                do {
                    try await self.websocket?.sendAudio(data)
                } catch {
                    await MainActor.run {
                        self.lastError = error.localizedDescription
                        self.status = "Streaming failed"
                    }
                }
            }
        }
        didInstallTap = true
    }

    private func teardownAudio() {
        audioEngine.stop()
        if didInstallTap {
            audioEngine.inputNode.removeTap(onBus: 0)
            didInstallTap = false
        }
        audioConverter = nil
    }

    private func handleEvent(_ event: SpeechMeshWebSocketClient.Event) {
        switch event {
        case .sessionStarted(let id):
            sessionID = id
            status = isRecording ? "Recording" : "Ready"
        case .asrResult(let payload):
            transcript = payload.text
            if payload.isFinal == true && payload.speechFinal == true {
                status = "Final"
                websocket?.close()
                websocket = nil
                sessionID = nil
            } else {
                status = "Partial"
            }
        case .sessionEnded:
            status = "Ended"
            websocket?.close()
            websocket = nil
            sessionID = nil
        case .failure(let message):
            lastError = message
            status = "Error"
            websocket?.close()
            websocket = nil
            sessionID = nil
        }
    }

    private static func convertToPCM16Mono(buffer: AVAudioPCMBuffer, converter: AVAudioConverter) -> Data? {
        let ratio = converter.outputFormat.sampleRate / buffer.format.sampleRate
        let frameCapacity = AVAudioFrameCount((Double(buffer.frameLength) * ratio).rounded(.up) + 32)
        guard let outputBuffer = AVAudioPCMBuffer(pcmFormat: converter.outputFormat, frameCapacity: frameCapacity) else {
            return nil
        }

        var didProvideInput = false
        var conversionError: NSError?
        let status = converter.convert(to: outputBuffer, error: &conversionError) { _, outStatus in
            if didProvideInput {
                outStatus.pointee = .endOfStream
                return nil
            }
            didProvideInput = true
            outStatus.pointee = .haveData
            return buffer
        }

        guard conversionError == nil, status != .error, outputBuffer.frameLength > 0 else {
            return nil
        }

        let audioBuffer = outputBuffer.audioBufferList.pointee.mBuffers
        guard let mData = audioBuffer.mData else { return nil }
        return Data(bytes: mData, count: Int(audioBuffer.mDataByteSize))
    }
}
