import SwiftUI

struct ContentView: View {
    @StateObject private var viewModel = TTSPlaybackViewModel()

    var body: some View {
        NavigationStack {
            Form {
                Section("Build") {
                    Text("debug-ui-v2")
                        .font(.footnote.monospaced())
                }

                Section("Gateway") {
                    TextField("WebSocket URL", text: $viewModel.gatewayURL)
                        .textInputAutocapitalization(.never)
                        .autocorrectionDisabled()
                        .keyboardType(.URL)
                }

                Section("Provider") {
                    TextField("Provider ID", text: $viewModel.providerID)
                        .textInputAutocapitalization(.never)
                        .autocorrectionDisabled()
                    TextField("Voice ID", text: $viewModel.voiceID)
                        .textInputAutocapitalization(.never)
                        .autocorrectionDisabled()
                    Button(viewModel.isLoadingVoices ? "Loading..." : "Load Voices") {
                        viewModel.loadVoices()
                    }
                    .disabled(viewModel.isBusy)
                    if !viewModel.voices.isEmpty {
                        Picker("Voice", selection: $viewModel.voiceID) {
                            ForEach(viewModel.voices, id: \.id) { voice in
                                Text(voice.displayName.isEmpty ? voice.id : "\(voice.displayName) (\(voice.id))")
                                    .tag(voice.id)
                            }
                        }
                    }
                }

                Section("Text") {
                    TextEditor(text: $viewModel.text)
                        .frame(minHeight: 140)
                }

                Section("Playback") {
                    Button(viewModel.isPlaying ? "Playing..." : "Play On iPhone") {
                        viewModel.playSample()
                    }
                    .disabled(viewModel.isBusy)

                    if viewModel.isPlaying {
                        Button("Stop") {
                            viewModel.stopPlayback()
                        }
                    }
                }

                Section("Speaker") {
                    Button("Local Speaker Test") {
                        viewModel.runLocalSpeakerTest()
                    }
                    .disabled(viewModel.isBusy)

                    Button("Refresh Audio Route") {
                        viewModel.refreshAudioRoute()
                    }
                    .disabled(viewModel.isBusy)

                    Text(viewModel.audioRouteDescription)
                        .font(.footnote)
                }

                Section("Status") {
                    Text(viewModel.status)
                        .font(.footnote)
                    Text("Last audio bytes: \(viewModel.lastAudioBytes)")
                        .font(.footnote)
                    if let error = viewModel.lastError {
                        Text(error)
                            .font(.footnote)
                            .foregroundStyle(.red)
                    }
                }

                if !viewModel.debugLines.isEmpty {
                    Section("Debug Log") {
                        ForEach(Array(viewModel.debugLines.enumerated()), id: \.offset) { _, line in
                            Text(line)
                                .font(.caption2.monospaced())
                                .textSelection(.enabled)
                        }
                    }
                }
            }
            .navigationTitle("SpeechMesh TTS")
            .task {
                fputs("[SpeechMeshTTSDemo] ContentView.task start\n", stderr)
                fflush(stderr)
                viewModel.autoplayIfNeeded()
            }
        }
    }
}
