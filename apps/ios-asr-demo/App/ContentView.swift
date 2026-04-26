import SwiftUI

struct ContentView: View {
    @StateObject private var viewModel = ASRViewModel()

    var body: some View {
        NavigationStack {
            ScrollView {
                VStack(alignment: .leading, spacing: 20) {
                    Group {
                        LabeledField(title: "Gateway URL", text: $viewModel.gatewayURL)
                        LabeledField(title: "Provider ID", text: $viewModel.providerID)
                        LabeledField(title: "Language", text: $viewModel.language)
                    }

                    VStack(alignment: .leading, spacing: 8) {
                        Text("Status")
                            .font(.headline)
                        Text(viewModel.status)
                            .font(.body.monospaced())
                            .foregroundStyle(.secondary)
                    }

                    VStack(alignment: .leading, spacing: 8) {
                        Text("Transcript")
                            .font(.headline)
                        Text(viewModel.transcript.isEmpty ? "Press and hold to talk" : viewModel.transcript)
                            .frame(maxWidth: .infinity, minHeight: 180, alignment: .topLeading)
                            .padding()
                            .background(Color(uiColor: .secondarySystemBackground))
                            .clipShape(RoundedRectangle(cornerRadius: 16, style: .continuous))
                    }

                    ZStack {
                        Circle()
                            .fill(viewModel.isRecording ? Color.red : Color.accentColor)
                            .frame(width: 140, height: 140)
                        Text(viewModel.isRecording ? "Release" : "Hold")
                            .font(.title2.weight(.semibold))
                            .foregroundStyle(.white)
                    }
                    .frame(maxWidth: .infinity)
                    .contentShape(Circle())
                    .gesture(
                        DragGesture(minimumDistance: 0)
                            .onChanged { _ in
                                if !viewModel.isRecording {
                                    viewModel.beginPress()
                                }
                            }
                            .onEnded { _ in
                                viewModel.endPress()
                            }
                    )

                    if let lastError = viewModel.lastError, !lastError.isEmpty {
                        VStack(alignment: .leading, spacing: 8) {
                            Text("Error")
                                .font(.headline)
                            Text(lastError)
                                .foregroundStyle(.red)
                        }
                    }
                }
                .padding(24)
            }
            .navigationTitle("SpeechMesh ASR")
        }
    }
}

private struct LabeledField: View {
    let title: String
    @Binding var text: String

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text(title)
                .font(.headline)
            TextField(title, text: $text)
                .textInputAutocapitalization(.never)
                .autocorrectionDisabled()
                .padding(12)
                .background(Color(uiColor: .secondarySystemBackground))
                .clipShape(RoundedRectangle(cornerRadius: 12, style: .continuous))
        }
    }
}
