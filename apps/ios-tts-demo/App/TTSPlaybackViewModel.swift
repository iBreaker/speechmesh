import AVFoundation
import Foundation
import SpeechMeshSwift
import SwiftUI

@MainActor
final class TTSPlaybackViewModel: ObservableObject {
    @Published var gatewayURL = "wss://speechmesh.svc.lan.breaker.host/ws"
    @Published var providerID = "minimax.tts"
    @Published var voiceID = "female-shaonv"
    @Published var text = "你好，我是 SpeechMesh iPhone TTS 测试。"
    @Published var voices: [VoiceDescriptor] = []
    @Published var status = "Ready"
    @Published var lastError: String?
    @Published var isPlaying = false
    @Published var isLoadingVoices = false
    @Published var audioRouteDescription = "Unknown"
    @Published var lastAudioBytes = 0
    @Published var debugLines: [String] = []

    private let player = SpeechMeshAudioPlayer()
    private let localSynth = AVSpeechSynthesizer()
    private var currentTask: Task<Void, Never>?
    private var hasAutoplayed = false

    init() {
        writeConsoleLine("[SpeechMeshTTSDemo] init viewModel")
    }

    var isBusy: Bool {
        isPlaying || isLoadingVoices
    }

    func autoplayIfNeeded() {
        guard !hasAutoplayed else { return }
        hasAutoplayed = true
        log("autoplayIfNeeded triggered")
        refreshAudioRoute()
        playSample()
    }

    func refreshAudioRoute() {
        #if os(iOS)
        let session = AVAudioSession.sharedInstance()
        let outputs = session.currentRoute.outputs.map { "\($0.portType.rawValue):\($0.portName)" }
        let route = outputs.isEmpty ? "no-output" : outputs.joined(separator: ", ")
        audioRouteDescription = "route=\(route) volume=\(session.outputVolume)"
        #else
        audioRouteDescription = "not available"
        #endif
        log("audio route \(audioRouteDescription)")
    }

    func runLocalSpeakerTest() {
        do {
            #if os(iOS)
            let session = AVAudioSession.sharedInstance()
            try session.setCategory(.playback, mode: .default, options: [.defaultToSpeaker])
            try session.setActive(true, options: [])
            #endif
            let utterance = AVSpeechUtterance(string: "你好，这是 iPhone 本地扬声器自检。")
            utterance.voice = AVSpeechSynthesisVoice(language: "zh-CN")
            utterance.rate = 0.45
            status = "Running local speaker test..."
            log("local speaker test start")
            localSynth.speak(utterance)
        } catch {
            lastError = error.localizedDescription
            status = "Local speaker test failed"
            log("local speaker test failed: \(error.localizedDescription)")
        }
    }

    func loadVoices() {
        guard !isBusy else { return }
        currentTask?.cancel()
        isLoadingVoices = true
        status = "Loading voices from gateway..."
        log("load voices start gateway=\(gatewayURL) provider=\(providerID)")
        lastError = nil
        let providerID = self.providerID

        currentTask = Task {
            defer {
                Task { @MainActor in
                    self.isLoadingVoices = false
                }
            }
            do {
                let client = try makeClient()
                try await withTimeout(seconds: 10, label: "connect") {
                    try await client.connect()
                }
                log("connected")
                defer { Task { await client.close() } }
                let result = try await withTimeout(seconds: 10, label: "tts.voices") {
                    try await client.listTTSVoices(.init(provider: .provider(providerID)))
                }
                self.voices = result.voices
                if self.voiceID.isEmpty, let first = result.voices.first {
                    self.voiceID = first.id
                }
                self.status = "Loaded \(result.voices.count) voice(s)."
                log("voices loaded count=\(result.voices.count)")
            } catch {
                self.lastError = error.localizedDescription
                self.status = "Load voices failed"
                log("load voices failed: \(error.localizedDescription)")
            }
        }
    }

    func playSample() {
        guard !isBusy else { return }
        currentTask?.cancel()
        isPlaying = true
        status = "Connecting to gateway..."
        refreshAudioRoute()
        lastAudioBytes = 0
        log("play start gateway=\(gatewayURL) provider=\(providerID) voice=\(voiceID)")
        lastError = nil
        let providerID = self.providerID
        let voiceID = self.voiceID
        let text = self.text

        currentTask = Task {
            defer {
                Task { @MainActor in
                    if !Task.isCancelled {
                        self.isPlaying = false
                    }
                }
            }
            do {
                let client = try makeClient()
                try await withTimeout(seconds: 10, label: "connect") {
                    try await client.connect()
                }
                log("connected")
                defer { Task { await client.close() } }

                self.status = "Starting TTS session..."
                let request = TtsStreamRequest(
                    provider: .provider(providerID),
                    inputKind: .text,
                    outputFormat: nil,
                    options: SynthesisOptions(
                        language: "zh-CN",
                        voice: voiceID.isEmpty ? nil : voiceID,
                        stream: true,
                        rate: nil,
                        pitch: nil,
                        volume: nil,
                        providerOptions: nil
                    )
                )
                _ = try await withTimeout(seconds: 10, label: "tts.start") {
                    try await client.startTTS(request)
                }
                log("session started")
                try await withTimeout(seconds: 10, label: "tts.input.append") {
                    try await client.appendTTSInput(text)
                }
                log("text appended chars=\(text.count)")
                try await withTimeout(seconds: 10, label: "tts.commit") {
                    try await client.commit()
                }
                log("commit sent")

                self.status = "Receiving audio..."
                var collector = SpeechMeshTTSCollector()
                while !Task.isCancelled {
                    let event = try await withTimeout(seconds: 15, label: "receive event") {
                        try await client.receive()
                    }
                    switch event {
                    case .ttsAudioDelta(_, _, let payload):
                        try collector.append(payload)
                        let bytes = try payload.decodedAudio().count
                        lastAudioBytes += bytes
                        log("audio chunk id=\(payload.chunkID) bytes=\(bytes) total=\(lastAudioBytes)")
                    case .ttsAudioDone:
                        let audio = collector.collectedAudio()
                        lastAudioBytes = audio.data.count
                        log("audio done totalBytes=\(audio.data.count) format=\(audio.format?.encoding.rawValue ?? "unknown")")
                        self.status = "Playing on iPhone speaker..."
                        try self.player.play(audio) { [weak self] in
                            Task { @MainActor in
                                self?.log("playback finished callback")
                                self?.status = "Playback finished"
                            }
                        }
                        log("player started")
                    case .sessionEnded:
                        log("session ended")
                        if collector.collectedAudio().data.isEmpty {
                            self.status = "Session ended without audio"
                        }
                        return
                    case .error(let requestID, let sessionID, let payload):
                        throw SpeechMeshClientError.server(payload.error, requestID: requestID, sessionID: sessionID)
                    default:
                        continue
                    }
                }
            } catch {
                self.lastError = error.localizedDescription
                self.status = "Playback failed"
                log("playback failed: \(error.localizedDescription)")
            }
        }
    }

    func stopPlayback() {
        currentTask?.cancel()
        player.stop()
        isPlaying = false
        status = "Stopped"
    }

    private func makeClient() throws -> SpeechMeshClient {
        guard let url = URL(string: gatewayURL) else {
            throw URLError(.badURL)
        }
        return SpeechMeshClient(config: SpeechMeshClientConfig(url: url, clientName: "speechmesh-ios-demo"))
    }

    private func log(_ message: String) {
        let stamp = DateFormatter.logTime.string(from: .now)
        let line = "\(stamp) \(message)"
        debugLines.append(line)
        if debugLines.count > 40 {
            debugLines.removeFirst(debugLines.count - 40)
        }
        NSLog("[SpeechMeshTTSDemo] %@", line)
        writeConsoleLine("[SpeechMeshTTSDemo] \(line)")
    }

    private func withTimeout<T: Sendable>(
        seconds: Double,
        label: String,
        operation: @escaping @Sendable () async throws -> T
    ) async throws -> T {
        try await withThrowingTaskGroup(of: T.self) { group in
            group.addTask {
                try await operation()
            }
            group.addTask {
                let duration = UInt64(seconds * 1_000_000_000)
                try await Task.sleep(nanoseconds: duration)
                throw NSError(
                    domain: "SpeechMeshTTSDemo.Timeout",
                    code: 1,
                    userInfo: [NSLocalizedDescriptionKey: "\(label) timed out after \(Int(seconds))s"]
                )
            }
            let result = try await group.next()!
            group.cancelAll()
            return result
        }
    }
}

private func writeConsoleLine(_ message: String) {
    fputs("\(message)\n", stderr)
    fflush(stderr)
}

private extension DateFormatter {
    static let logTime: DateFormatter = {
        let formatter = DateFormatter()
        formatter.dateFormat = "HH:mm:ss"
        return formatter
    }()
}
