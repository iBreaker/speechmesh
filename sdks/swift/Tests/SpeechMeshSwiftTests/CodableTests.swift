import Foundation
import Testing
@testable import SpeechMeshSwift

struct CodableTests {
    @Test
    func providerSelectorEncodesSnakeCase() throws {
        let selector = ProviderSelector.provider("apple.asr")
        let data = try JSONEncoder().encode(selector)
        let text = String(decoding: data, as: UTF8.self)

        #expect(text.contains("\"provider_id\":\"apple.asr\""))
        #expect(text.contains("\"mode\":\"provider\""))
    }

    @Test
    func ttsAudioDeltaDecodesAudio() throws {
        let payload = TtsAudioDeltaPayload(
            chunkID: 1,
            audioBase64: Data("hello".utf8).base64EncodedString(),
            isFinal: false,
            format: .pcmS16LE(sampleRateHz: 16_000, channels: 1)
        )

        #expect(try payload.decodedAudio() == Data("hello".utf8))
    }

    @Test
    func ttsCollectorAccumulatesChunks() throws {
        var collector = SpeechMeshTTSCollector()
        try collector.append(
            TtsAudioDeltaPayload(
                chunkID: 1,
                audioBase64: Data([1, 2]).base64EncodedString(),
                isFinal: false,
                format: .pcmS16LE(sampleRateHz: 16_000, channels: 1)
            )
        )
        try collector.append(
            TtsAudioDeltaPayload(
                chunkID: 2,
                audioBase64: Data([3, 4]).base64EncodedString(),
                isFinal: true,
                format: nil
            )
        )

        let audio = collector.collectedAudio()
        #expect(audio.data == Data([1, 2, 3, 4]))
        #expect(audio.format == .pcmS16LE(sampleRateHz: 16_000, channels: 1))
    }
}
