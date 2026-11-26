import SwiftUI
import UseThatScreenProtocol

@main
struct UseThatScreenApp: App {
    @StateObject private var session = DisplaySession()

    var body: some Scene {
        WindowGroup {
            ContentView(session: session)
                .tint(Color(red: 0.19, green: 0.40, blue: 0.30))
        }
    }
}

private struct ContentView: View {
    @Environment(\.displayScale) private var displayScale
    @Environment(\.scenePhase) private var scenePhase
    @ObservedObject var session: DisplaySession

    var body: some View {
        GeometryReader { geometry in
            Group {
                if let config = session.config {
                    stream(config: config)
                } else {
                    home
                }
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            .accessibilityHidden(session.comparison != nil)
            .onChange(of: geometry.size, initial: true) { _, size in
                if scenePhase == .active { start(size) }
            }
            .onChange(of: scenePhase, initial: true) { _, phase in
                if phase == .active { start(geometry.size) }
                if phase == .background { session.stop() }
            }
        }
        .overlay {
            if let comparison = session.comparison {
                ZStack {
                    Color.black.opacity(0.25).ignoresSafeArea()
                    pairingCard(comparison)
                }
            }
        }
    }

    private func pairingCard(_ comparison: DisplaySession.Comparison) -> some View {
        VStack(alignment: .leading, spacing: 20) {
            Text("Pair with \(comparison.hostName)")
                .font(.title.weight(.semibold))
            Text("Check that the computer shows the same code.")
                .foregroundStyle(.secondary)
            Text(comparison.code)
                .font(.system(size: 56, weight: .semibold, design: .monospaced))
                .minimumScaleFactor(0.5)
                .accessibilityLabel(comparison.code.map(String.init).joined(separator: " "))
            HStack {
                Button("Cancel", role: .cancel) { session.confirmCode(false) }
                    .buttonStyle(.bordered)
                Button("Codes Match") { session.confirmCode(true) }
                    .buttonStyle(.borderedProminent)
                    .keyboardShortcut(.defaultAction)
            }
            .controlSize(.large)
        }
        .padding(32)
        .frame(maxWidth: 440, alignment: .leading)
        .background(.regularMaterial, in: RoundedRectangle(cornerRadius: 22))
        .padding(24)
    }

    private var home: some View {
        VStack(alignment: .leading, spacing: 20) {
            Image("BrandLogo")
                .resizable()
                .scaledToFit()
                .frame(width: 52, height: 52)
                .accessibilityHidden(true)
            Text("Use That Screen")
                .font(.largeTitle.weight(.bold))
            VStack(alignment: .leading, spacing: 8) {
                Text(session.status)
                    .font(.headline)
                    .accessibilityIdentifier("connection-status")
                Text("On your Mac or Windows PC, open Use That Screen and choose **\(session.deviceName)** under Wi-Fi.")
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
            Picker("Display mode", selection: $session.mode) {
                ForEach(DisplaySession.Mode.allCases) { Text($0.rawValue).tag($0) }
            }
            .pickerStyle(.segmented)
            if let notice = session.notice {
                Text(notice)
                    .font(.callout)
                    .foregroundStyle(.secondary)
            }
            Link("Download the computer app", destination: URL(string: "https://usethatscreen.com/#download")!)
        }
        .frame(maxWidth: 470, alignment: .leading)
        .padding(.horizontal, 28)
        .padding(.vertical, 20)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(Color(uiColor: .systemGroupedBackground))
    }

    private func stream(config: Config) -> some View {
        ZStack {
            Color.black.ignoresSafeArea()
            StreamSurface(renderer: session.renderer)
                .ignoresSafeArea()
                .accessibilityIdentifier("stream-surface")
            if !session.receivedVideo {
                ProgressView("Waiting for video…")
                    .tint(.white)
                    .foregroundStyle(.white)
            }
        }
        .overlay(alignment: .topTrailing) {
            Button("Disconnect", systemImage: "xmark") { session.disconnect() }
                .labelStyle(.iconOnly)
                .buttonStyle(.borderedProminent)
                .tint(.black.opacity(0.7))
                .padding()
                .accessibilityIdentifier("disconnect-stream")
        }
        .overlay(alignment: .topLeading) {
            if HostCaps(rawValue: config.hostCaps).contains(.viewOnly) {
                Text("Shared window")
                    .font(.caption.weight(.medium))
                    .foregroundStyle(.white)
                    .padding(8)
                    .background(.black.opacity(0.7), in: Capsule())
                    .padding()
            }
        }
        .statusBarHidden()
    }

    private func start(_ size: CGSize) {
        session.start(size: CGSize(width: size.width * displayScale, height: size.height * displayScale))
    }
}

private struct StreamSurface: UIViewRepresentable {
    let renderer: StreamRenderer

    func makeUIView(context: Context) -> View {
        View(renderer: renderer)
    }

    func updateUIView(_ view: View, context: Context) {}

    final class View: UIView {
        private let renderer: StreamRenderer

        init(renderer: StreamRenderer) {
            self.renderer = renderer
            super.init(frame: .zero)
            backgroundColor = .black
            layer.addSublayer(renderer.layer)
        }

        required init?(coder: NSCoder) { nil }

        override func layoutSubviews() {
            super.layoutSubviews()
            CATransaction.begin()
            CATransaction.setDisableActions(true)
            renderer.layer.frame = bounds
            CATransaction.commit()
        }
    }
}
