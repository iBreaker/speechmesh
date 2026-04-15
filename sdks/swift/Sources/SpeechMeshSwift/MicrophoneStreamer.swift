@preconcurrency import AVFoundation
import Foundation

public enum SpeechMeshMicrophoneError: Error, LocalizedError {
    case recordPermissionDenied
    case inputUnavailable
    case converterCreationFailed

    public var errorDescription: String? {
        switch self {
        case .recordPermissionDenied:
            return "microphone permission denied"
        case .inputUnavailable:
            return "microphone input node is unavailable"
        case .converterCreationFailed:
            return "failed to create audio converter for speechmesh microphone stream"
        }
    }
}

public final class SpeechMeshMicrophoneStreamer {
    public struct Configuration: Sendable {
        public var chunkFrameCount: AVAudioFrameCount
        public var outputFormat: AudioFormat

        public init(chunkFrameCount: AVAudioFrameCount = 1_024, outputFormat: AudioFormat = .pcmS16LE(sampleRateHz: 16_000, channels: 1)) {
            self.chunkFrameCount = chunkFrameCount
            self.outputFormat = outputFormat
        }
    }

    private let engine = AVAudioEngine()
    private let configuration: Configuration
    private var converter: AVAudioConverter?

    public init(configuration: Configuration = Configuration()) {
        self.configuration = configuration
    }

    public func start(onChunk: @escaping @Sendable (Data) -> Void) async throws {
        try await requestPermissionIfNeeded()
        #if os(iOS)
        let session = AVAudioSession.sharedInstance()
        try session.setCategory(.playAndRecord, mode: .measurement, options: [.defaultToSpeaker, .allowBluetooth])
        try session.setPreferredSampleRate(Double(configuration.outputFormat.sampleRateHz))
        try session.setActive(true, options: [])
        #endif

        let inputNode = engine.inputNode
        let inputFormat = inputNode.inputFormat(forBus: 0)
        guard inputFormat.channelCount > 0 else {
            throw SpeechMeshMicrophoneError.inputUnavailable
        }

        guard let targetFormat = AVAudioFormat(
            commonFormat: .pcmFormatInt16,
            sampleRate: Double(configuration.outputFormat.sampleRateHz),
            channels: AVAudioChannelCount(configuration.outputFormat.channels),
            interleaved: false
        ) else {
            throw SpeechMeshMicrophoneError.converterCreationFailed
        }

        let converter = AVAudioConverter(from: inputFormat, to: targetFormat)
        guard let converter else {
            throw SpeechMeshMicrophoneError.converterCreationFailed
        }
        self.converter = converter

        inputNode.removeTap(onBus: 0)
        inputNode.installTap(onBus: 0, bufferSize: configuration.chunkFrameCount, format: inputFormat) { [weak self] buffer, _ in
            guard let self else { return }
            guard let data = self.convert(buffer: buffer, targetFormat: targetFormat) else {
                return
            }
            onChunk(data)
        }

        engine.prepare()
        try engine.start()
    }

    public func stop() {
        engine.inputNode.removeTap(onBus: 0)
        engine.stop()
        #if os(iOS)
        try? AVAudioSession.sharedInstance().setActive(false, options: [.notifyOthersOnDeactivation])
        #endif
    }

    private func convert(buffer: AVAudioPCMBuffer, targetFormat: AVAudioFormat) -> Data? {
        guard let converter else {
            return nil
        }

        let ratio = targetFormat.sampleRate / buffer.format.sampleRate
        let targetFrameCapacity = max(AVAudioFrameCount(Double(buffer.frameLength) * ratio) + 32, 32)
        guard let converted = AVAudioPCMBuffer(pcmFormat: targetFormat, frameCapacity: targetFrameCapacity) else {
            return nil
        }

        final class InputState: @unchecked Sendable {
            let buffer: AVAudioPCMBuffer
            var didProvideInput = false

            init(buffer: AVAudioPCMBuffer) {
                self.buffer = buffer
            }
        }

        let state = InputState(buffer: buffer)
        var error: NSError?
        let status = converter.convert(to: converted, error: &error) { _, outStatus in
            if state.didProvideInput {
                outStatus.pointee = .noDataNow
                return nil
            }
            state.didProvideInput = true
            outStatus.pointee = .haveData
            return state.buffer
        }

        guard error == nil else {
            return nil
        }
        guard status == .haveData || status == .inputRanDry else {
            return nil
        }
        guard let channelData = converted.int16ChannelData else {
            return nil
        }

        let samples = Int(converted.frameLength)
        let byteCount = samples * MemoryLayout<Int16>.size
        return Data(bytes: channelData[0], count: byteCount)
    }

    private func requestPermissionIfNeeded() async throws {
        #if os(iOS)
        let granted = await withCheckedContinuation { continuation in
            AVAudioSession.sharedInstance().requestRecordPermission { granted in
                continuation.resume(returning: granted)
            }
        }
        guard granted else {
            throw SpeechMeshMicrophoneError.recordPermissionDenied
        }
        #endif
    }
}
