import Foundation

public enum CapabilityDomain: String, Codable, Sendable {
    case asr
    case tts
    case transport
}

public struct Capability: Codable, Equatable, Sendable {
    public var key: String
    public var enabled: Bool

    public init(key: String, enabled: Bool) {
        self.key = key
        self.enabled = enabled
    }
}

public enum RuntimeMode: String, Codable, Sendable {
    case inProcess = "in_process"
    case localDaemon = "local_daemon"
    case remoteGateway = "remote_gateway"
}

public enum ProviderSelectionMode: String, Codable, Sendable {
    case auto
    case provider
}

public struct ProviderSelector: Codable, Equatable, Sendable {
    public var mode: ProviderSelectionMode
    public var providerID: String?
    public var requiredCapabilities: [String]
    public var preferredCapabilities: [String]

    enum CodingKeys: String, CodingKey {
        case mode
        case providerID = "provider_id"
        case requiredCapabilities = "required_capabilities"
        case preferredCapabilities = "preferred_capabilities"
    }

    public init(
        mode: ProviderSelectionMode = .auto,
        providerID: String? = nil,
        requiredCapabilities: [String] = [],
        preferredCapabilities: [String] = []
    ) {
        self.mode = mode
        self.providerID = providerID
        self.requiredCapabilities = requiredCapabilities
        self.preferredCapabilities = preferredCapabilities
    }

    public static func provider(_ providerID: String) -> ProviderSelector {
        ProviderSelector(mode: .provider, providerID: providerID)
    }
}

public enum AudioEncoding: String, Codable, Sendable {
    case pcmS16LE = "pcm_s16le"
    case pcmF32LE = "pcm_f32le"
    case opus
    case mp3
    case aac
    case flac
    case wav
}

public struct AudioFormat: Codable, Equatable, Sendable {
    public var encoding: AudioEncoding
    public var sampleRateHz: UInt32
    public var channels: UInt16

    enum CodingKeys: String, CodingKey {
        case encoding
        case sampleRateHz = "sample_rate_hz"
        case channels
    }

    public init(encoding: AudioEncoding, sampleRateHz: UInt32, channels: UInt16) {
        self.encoding = encoding
        self.sampleRateHz = sampleRateHz
        self.channels = channels
    }

    public static func pcmS16LE(sampleRateHz: UInt32, channels: UInt16) -> AudioFormat {
        AudioFormat(encoding: .pcmS16LE, sampleRateHz: sampleRateHz, channels: channels)
    }
}

public struct ProviderDescriptor: Codable, Equatable, Sendable {
    public var id: String
    public var name: String
    public var domain: CapabilityDomain
    public var runtime: RuntimeMode
    public var capabilities: [Capability]

    public init(id: String, name: String, domain: CapabilityDomain, runtime: RuntimeMode, capabilities: [Capability] = []) {
        self.id = id
        self.name = name
        self.domain = domain
        self.runtime = runtime
        self.capabilities = capabilities
    }
}

public struct ErrorInfo: Codable, Equatable, Sendable {
    public var code: String
    public var message: String
    public var retryable: Bool
    public var details: JSONValue

    public init(code: String, message: String, retryable: Bool = false, details: JSONValue = .null) {
        self.code = code
        self.message = message
        self.retryable = retryable
        self.details = details
    }
}

public struct HelloRequest: Codable, Equatable, Sendable {
    public var protocolVersion: String
    public var clientName: String?

    enum CodingKeys: String, CodingKey {
        case protocolVersion = "protocol_version"
        case clientName = "client_name"
    }

    public init(protocolVersion: String, clientName: String?) {
        self.protocolVersion = protocolVersion
        self.clientName = clientName
    }
}

public struct HelloResponse: Codable, Equatable, Sendable {
    public var protocolVersion: String
    public var serverName: String
    public var oneSessionPerConnection: Bool

    enum CodingKeys: String, CodingKey {
        case protocolVersion = "protocol_version"
        case serverName = "server_name"
        case oneSessionPerConnection = "one_session_per_connection"
    }

    public init(protocolVersion: String, serverName: String, oneSessionPerConnection: Bool) {
        self.protocolVersion = protocolVersion
        self.serverName = serverName
        self.oneSessionPerConnection = oneSessionPerConnection
    }
}

public struct DiscoverRequest: Codable, Equatable, Sendable {
    public var domains: [CapabilityDomain]

    public init(domains: [CapabilityDomain]) {
        self.domains = domains
    }
}

public struct DiscoverResult: Codable, Equatable, Sendable {
    public var providers: [ProviderDescriptor]

    public init(providers: [ProviderDescriptor]) {
        self.providers = providers
    }
}

public enum StreamMode: String, Codable, Sendable {
    case buffered
    case streaming
}

public struct SessionStartedPayload: Codable, Equatable, Sendable {
    public var domain: CapabilityDomain
    public var providerID: String
    public var acceptedInputFormat: AudioFormat?
    public var acceptedOutputFormat: AudioFormat?
    public var inputMode: StreamMode?
    public var outputMode: StreamMode?

    enum CodingKeys: String, CodingKey {
        case domain
        case providerID = "provider_id"
        case acceptedInputFormat = "accepted_input_format"
        case acceptedOutputFormat = "accepted_output_format"
        case inputMode = "input_mode"
        case outputMode = "output_mode"
    }

    public init(
        domain: CapabilityDomain,
        providerID: String,
        acceptedInputFormat: AudioFormat? = nil,
        acceptedOutputFormat: AudioFormat? = nil,
        inputMode: StreamMode? = nil,
        outputMode: StreamMode? = nil
    ) {
        self.domain = domain
        self.providerID = providerID
        self.acceptedInputFormat = acceptedInputFormat
        self.acceptedOutputFormat = acceptedOutputFormat
        self.inputMode = inputMode
        self.outputMode = outputMode
    }
}

public struct AsrWordPayload: Codable, Equatable, Sendable {
    public var text: String
    public var startMS: UInt64?
    public var endMS: UInt64?
    public var isFinal: Bool

    enum CodingKeys: String, CodingKey {
        case text
        case startMS = "start_ms"
        case endMS = "end_ms"
        case isFinal = "is_final"
    }

    public init(text: String, startMS: UInt64? = nil, endMS: UInt64? = nil, isFinal: Bool) {
        self.text = text
        self.startMS = startMS
        self.endMS = endMS
        self.isFinal = isFinal
    }
}

public struct AsrResultPayload: Codable, Equatable, Sendable {
    public var segmentID: UInt64
    public var revision: UInt64
    public var text: String
    public var delta: String?
    public var isFinal: Bool
    public var speechFinal: Bool
    public var beginTimeMS: UInt64?
    public var endTimeMS: UInt64?
    public var words: [AsrWordPayload]

    enum CodingKeys: String, CodingKey {
        case segmentID = "segment_id"
        case revision
        case text
        case delta
        case isFinal = "is_final"
        case speechFinal = "speech_final"
        case beginTimeMS = "begin_time_ms"
        case endTimeMS = "end_time_ms"
        case words
    }

    public init(
        segmentID: UInt64,
        revision: UInt64,
        text: String,
        delta: String? = nil,
        isFinal: Bool,
        speechFinal: Bool,
        beginTimeMS: UInt64? = nil,
        endTimeMS: UInt64? = nil,
        words: [AsrWordPayload] = []
    ) {
        self.segmentID = segmentID
        self.revision = revision
        self.text = text
        self.delta = delta
        self.isFinal = isFinal
        self.speechFinal = speechFinal
        self.beginTimeMS = beginTimeMS
        self.endTimeMS = endTimeMS
        self.words = words
    }
}

public struct RecognitionOptions: Codable, Equatable, Sendable {
    public var language: String?
    public var hints: [String]
    public var interimResults: Bool
    public var timestamps: Bool
    public var punctuation: Bool
    public var preferOnDevice: Bool
    public var providerOptions: JSONValue?

    enum CodingKeys: String, CodingKey {
        case language
        case hints
        case interimResults = "interim_results"
        case timestamps
        case punctuation
        case preferOnDevice = "prefer_on_device"
        case providerOptions = "provider_options"
    }

    public init(
        language: String? = nil,
        hints: [String] = [],
        interimResults: Bool = false,
        timestamps: Bool = false,
        punctuation: Bool = false,
        preferOnDevice: Bool = false,
        providerOptions: JSONValue? = nil
    ) {
        self.language = language
        self.hints = hints
        self.interimResults = interimResults
        self.timestamps = timestamps
        self.punctuation = punctuation
        self.preferOnDevice = preferOnDevice
        self.providerOptions = providerOptions
    }
}

public struct AsrStreamRequest: Codable, Equatable, Sendable {
    public var provider: ProviderSelector
    public var inputFormat: AudioFormat
    public var options: RecognitionOptions

    enum CodingKeys: String, CodingKey {
        case provider
        case inputFormat = "input_format"
        case options
    }

    public init(provider: ProviderSelector = ProviderSelector(), inputFormat: AudioFormat, options: RecognitionOptions = RecognitionOptions()) {
        self.provider = provider
        self.inputFormat = inputFormat
        self.options = options
    }
}

public struct SessionEndedPayload: Codable, Equatable, Sendable {
    public var reason: String?

    public init(reason: String? = nil) {
        self.reason = reason
    }
}

public struct VoiceListRequest: Codable, Equatable, Sendable {
    public var provider: ProviderSelector
    public var language: String?

    public init(provider: ProviderSelector = ProviderSelector(), language: String? = nil) {
        self.provider = provider
        self.language = language
    }
}

public struct VoiceDescriptor: Codable, Equatable, Sendable {
    public var id: String
    public var language: String
    public var displayName: String
    public var gender: String?
    public var capabilities: [String]

    enum CodingKeys: String, CodingKey {
        case id
        case language
        case displayName = "display_name"
        case gender
        case capabilities
    }

    public init(id: String, language: String, displayName: String, gender: String? = nil, capabilities: [String] = []) {
        self.id = id
        self.language = language
        self.displayName = displayName
        self.gender = gender
        self.capabilities = capabilities
    }
}

public struct VoiceListResult: Codable, Equatable, Sendable {
    public var voices: [VoiceDescriptor]

    public init(voices: [VoiceDescriptor]) {
        self.voices = voices
    }
}

public enum SynthesisInputKind: String, Codable, Sendable {
    case text
    case ssml
}

public struct SynthesisOptions: Codable, Equatable, Sendable {
    public var language: String?
    public var voice: String?
    public var stream: Bool
    public var rate: Float?
    public var pitch: Float?
    public var volume: Float?
    public var providerOptions: JSONValue?

    enum CodingKeys: String, CodingKey {
        case language
        case voice
        case stream
        case rate
        case pitch
        case volume
        case providerOptions = "provider_options"
    }

    public init(
        language: String? = nil,
        voice: String? = nil,
        stream: Bool = false,
        rate: Float? = nil,
        pitch: Float? = nil,
        volume: Float? = nil,
        providerOptions: JSONValue? = nil
    ) {
        self.language = language
        self.voice = voice
        self.stream = stream
        self.rate = rate
        self.pitch = pitch
        self.volume = volume
        self.providerOptions = providerOptions
    }
}

public struct TtsStreamRequest: Codable, Equatable, Sendable {
    public var provider: ProviderSelector
    public var inputKind: SynthesisInputKind
    public var outputFormat: AudioFormat?
    public var options: SynthesisOptions

    enum CodingKeys: String, CodingKey {
        case provider
        case inputKind = "input_kind"
        case outputFormat = "output_format"
        case options
    }

    public init(
        provider: ProviderSelector = ProviderSelector(),
        inputKind: SynthesisInputKind = .text,
        outputFormat: AudioFormat? = nil,
        options: SynthesisOptions = SynthesisOptions()
    ) {
        self.provider = provider
        self.inputKind = inputKind
        self.outputFormat = outputFormat
        self.options = options
    }
}

public struct TtsInputAppendPayload: Codable, Equatable, Sendable {
    public var delta: String

    public init(delta: String) {
        self.delta = delta
    }
}

public struct TtsAudioDeltaPayload: Codable, Equatable, Sendable {
    public var chunkID: UInt64
    public var audioBase64: String
    public var isFinal: Bool
    public var format: AudioFormat?

    enum CodingKeys: String, CodingKey {
        case chunkID = "chunk_id"
        case audioBase64 = "audio_base64"
        case isFinal = "is_final"
        case format
    }

    public init(chunkID: UInt64, audioBase64: String, isFinal: Bool, format: AudioFormat? = nil) {
        self.chunkID = chunkID
        self.audioBase64 = audioBase64
        self.isFinal = isFinal
        self.format = format
    }

    public func decodedAudio() throws -> Data {
        guard let data = Data(base64Encoded: audioBase64) else {
            throw SpeechMeshClientError.invalidBase64Audio
        }
        return data
    }
}

public struct TtsAudioDonePayload: Codable, Equatable, Sendable {
    public var inputKind: SynthesisInputKind
    public var totalChunks: UInt64
    public var totalBytes: UInt64

    enum CodingKeys: String, CodingKey {
        case inputKind = "input_kind"
        case totalChunks = "total_chunks"
        case totalBytes = "total_bytes"
    }

    public init(inputKind: SynthesisInputKind, totalChunks: UInt64, totalBytes: UInt64) {
        self.inputKind = inputKind
        self.totalChunks = totalChunks
        self.totalBytes = totalBytes
    }
}

public struct ErrorPayload: Codable, Equatable, Sendable {
    public var error: ErrorInfo

    public init(error: ErrorInfo) {
        self.error = error
    }
}
