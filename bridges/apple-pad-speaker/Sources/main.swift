import AVFoundation
import Foundation

func log(_ message: String) {
    fputs("speechmesh-pad-speaker: \(message)\n", stderr)
}

func describeCurrentRoute(_ session: AVAudioSession) -> String {
    let outputs = session.currentRoute.outputs.map { output in
        "\(output.portType.rawValue):\(output.portName)"
    }
    if outputs.isEmpty {
        return "(no outputs)"
    }
    return outputs.joined(separator: ", ")
}

func configureAudioSession() throws -> AVAudioSession {
    let session = AVAudioSession.sharedInstance()
    try session.setCategory(
        .playAndRecord,
        mode: .default,
        options: [.defaultToSpeaker, .duckOthers]
    )
    try session.setActive(true, options: [])
    log("audio session ready route=\(describeCurrentRoute(session))")
    return session
}

final class PlaybackCoordinator: NSObject, AVAudioPlayerDelegate {
    private let finish = DispatchSemaphore(value: 0)
    private var successfully = false

    func audioPlayerDidFinishPlaying(_ player: AVAudioPlayer, successfully flag: Bool) {
        successfully = flag
        finish.signal()
    }

    func audioPlayerDecodeErrorDidOccur(_ player: AVAudioPlayer, error: Error?) {
        log("decode error: \(error?.localizedDescription ?? "decode error")")
        successfully = false
        finish.signal()
    }

    func waitForPlayback(timeoutSeconds: TimeInterval) -> Bool {
        let deadline = DispatchTime.now() + timeoutSeconds
        return finish.wait(timeout: deadline) == .success
    }

    func didFinish() -> Bool {
        successfully
    }
}

func readStdinAudio() throws -> Data {
    let input = FileHandle.standardInput.readDataToEndOfFile()
    if input.isEmpty {
        throw NSError(
            domain: "speechmesh-pad-speaker",
            code: 1,
            userInfo: [NSLocalizedDescriptionKey: "no audio payload from stdin"]
        )
    }
    return input
}

func writeTempFile(_ data: Data) throws -> URL {
    let dir = FileManager.default.temporaryDirectory
    let fileURL = dir.appendingPathComponent("speechmesh-pad-player-\(UUID().uuidString)")
    try data.write(to: fileURL, options: .atomic)
    return fileURL
}

func resolvePlayerCommand() -> String {
    let overrides = [
        ProcessInfo.processInfo.environment["SPEECHMESH_PAD_INTERNAL_PLAYER"]
    ]
    for candidate in overrides.compactMap({$0}) where !candidate.isEmpty {
        return candidate
    }

    let candidates = [
        "/var/jb/usr/bin/mpg123",
        "/usr/bin/mpg123",
        "/usr/local/bin/mpg123",
        "/usr/bin/afplay",
        "afplay",
    ]
    for candidate in candidates where FileManager.default.fileExists(atPath: candidate) {
        return candidate
    }
    return "/var/jb/usr/bin/mpg123"
}

func runPlayer(fileURL: URL) -> Result<Void, Error> {
    do {
        let session = try configureAudioSession()
        let player = try AVAudioPlayer(contentsOf: fileURL)
        let coordinator = PlaybackCoordinator()
        player.delegate = coordinator
        player.volume = 1.0
        player.prepareToPlay()

        guard player.play() else {
            return .failure(
                NSError(
                    domain: "speechmesh-pad-speaker",
                    code: 2,
                    userInfo: [NSLocalizedDescriptionKey: "player failed to start"]
                )
            )
        }

        let timeout = max(30, Int(player.duration.rounded(.up)) + 2)
        let deadline = Date().addingTimeInterval(TimeInterval(timeout))
        while Date() < deadline {
            if coordinator.waitForPlayback(timeoutSeconds: 0.1) {
                break
            }
            RunLoop.current.run(mode: .default, before: Date().addingTimeInterval(0.1))
        }

        if !coordinator.didFinish() {
            if Date() >= deadline {
                player.stop()
                return .failure(
                    NSError(
                        domain: "speechmesh-pad-speaker",
                        code: 3,
                        userInfo: [NSLocalizedDescriptionKey: "playback timeout route=\(describeCurrentRoute(session))"]
                    )
                )
            }
            player.stop()
            return .failure(
                NSError(
                    domain: "speechmesh-pad-speaker",
                    code: 4,
                    userInfo: [NSLocalizedDescriptionKey: "playback failed route=\(describeCurrentRoute(session))"]
                )
            )
        }
        try? session.setActive(false, options: [.notifyOthersOnDeactivation])
        return .success(())
    } catch {
        return .failure(error)
    }
}

do {
    let audioData = try readStdinAudio()
    let playerPath = resolvePlayerCommand()
    let _ = playerPath

    let tempFile = try writeTempFile(audioData)
    defer { try? FileManager.default.removeItem(at: tempFile) }

    switch runPlayer(fileURL: tempFile) {
    case .success:
        log("playback finished with player=\(playerPath)")
        exit(0)
    case .failure(let error):
        log("playback failed: \(error.localizedDescription)")
        exit(10)
    }
} catch {
    log("fatal: \(error.localizedDescription)")
    exit(11)
}
