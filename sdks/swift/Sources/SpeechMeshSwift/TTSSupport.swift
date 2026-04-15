import AVFoundation
import Foundation

public struct SpeechMeshCollectedAudio: Sendable, Equatable {
    public var data: Data
    public var format: AudioFormat?

    public init(data: Data, format: AudioFormat?) {
        self.data = data
        self.format = format
    }
}

public struct SpeechMeshTTSCollector: Sendable {
    private(set) var format: AudioFormat?
    private(set) var data: Data

    public init() {
        self.format = nil
        self.data = Data()
    }

    public mutating func append(_ payload: TtsAudioDeltaPayload) throws {
        if format == nil {
            format = payload.format
        }
        data.append(try payload.decodedAudio())
    }

    public func collectedAudio() -> SpeechMeshCollectedAudio {
        SpeechMeshCollectedAudio(data: data, format: format)
    }
}

public final class SpeechMeshAudioPlayer: NSObject, AVAudioPlayerDelegate {
    private var player: AVAudioPlayer?
    private var completion: (() -> Void)?
    private var tempFileURL: URL?

    public func play(_ audio: SpeechMeshCollectedAudio, completion: (() -> Void)? = nil) throws {
        self.completion = completion
        #if os(iOS)
        let session = AVAudioSession.sharedInstance()
        try session.setCategory(.playback, mode: .default, options: [.defaultToSpeaker])
        try session.setActive(true, options: [])
        #endif
        cleanupTempFile()
        let fileURL = try writeTempFile(for: audio)
        let player = try AVAudioPlayer(contentsOf: fileURL)
        player.delegate = self
        player.prepareToPlay()
        if !player.play() {
            throw NSError(domain: "SpeechMeshAudioPlayer", code: 1, userInfo: [NSLocalizedDescriptionKey: "AVAudioPlayer failed to start playback"])
        }
        self.player = player
    }

    public func stop() {
        player?.stop()
        player = nil
        completion = nil
        cleanupTempFile()
        #if os(iOS)
        try? AVAudioSession.sharedInstance().setActive(false, options: [.notifyOthersOnDeactivation])
        #endif
    }

    public func audioPlayerDidFinishPlaying(_ player: AVAudioPlayer, successfully flag: Bool) {
        completion?()
        completion = nil
        cleanupTempFile()
    }

    private func writeTempFile(for audio: SpeechMeshCollectedAudio) throws -> URL {
        let ext = fileExtension(for: audio.format?.encoding)
        let url = FileManager.default.temporaryDirectory
            .appendingPathComponent(UUID().uuidString)
            .appendingPathExtension(ext)
        try audio.data.write(to: url, options: [.atomic])
        tempFileURL = url
        return url
    }

    private func cleanupTempFile() {
        if let tempFileURL {
            try? FileManager.default.removeItem(at: tempFileURL)
            self.tempFileURL = nil
        }
    }

    private func fileExtension(for encoding: AudioEncoding?) -> String {
        guard let encoding else { return "bin" }
        switch encoding {
        case .mp3:
            return "mp3"
        case .aac:
            return "m4a"
        case .wav, .pcmS16LE, .pcmF32LE:
            return "wav"
        case .flac:
            return "flac"
        case .opus:
            return "opus"
        }
    }
}
