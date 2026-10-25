import AppKit
import AnyDisplayHost
import AnyDisplayProtocol
import Foundation
import Network

let usage = """
usage:
  anydisplay                              run the menu bar app
  anydisplay connect HOST[:PORT] [opts]   connect to a client over the network (default port 47820)
  anydisplay usb [SERIAL] [opts]          connect to a USB client through adb forward
  anydisplay discover [--seconds N]       list clients on Wi-Fi (Bonjour) and USB (adb)
  anydisplay paired                       list paired devices
  anydisplay forget DEVICE_ID | --all     remove pairings

options:
  -y, --yes          accept the pairing code without asking
  --seconds N        end the session N seconds after streaming starts
  --no-clipboard     do not share the clipboard
  -v, --verbose      debug logging
"""

struct Options {
    var positional: [String] = []
    var yes = false
    var seconds: Double?
    var clipboard = true
    var verbose = false
}

func parse(_ args: [String]) -> Options {
    var o = Options()
    var i = 0
    while i < args.count {
        switch args[i] {
        case "-y", "--yes": o.yes = true
        case "-v", "--verbose": o.verbose = true
        case "--no-clipboard": o.clipboard = false
        case "--seconds":
            i += 1
            guard i < args.count, let s = Double(args[i]) else { fail("--seconds needs a number") }
            o.seconds = s
        default: o.positional.append(args[i])
        }
        i += 1
    }
    return o
}

func fail(_ message: String) -> Never {
    FileHandle.standardError.write(Data("anydisplay: \(message)\n".utf8))
    exit(2)
}

func endpoint(_ target: String) -> NWEndpoint {
    var host = target
    var port = anyDisplayPort
    if target.hasPrefix("["), let close = target.firstIndex(of: "]") {
        host = String(target[target.index(after: target.startIndex)..<close])
        let rest = target[target.index(after: close)...]
        if rest.hasPrefix(":"), let p = UInt16(rest.dropFirst()) { port = p }
    } else if target.filter({ $0 == ":" }).count == 1, let idx = target.lastIndex(of: ":") {
        host = String(target[..<idx])
        guard let p = UInt16(target[target.index(after: idx)...]) else { fail("bad port in \(target)") }
        port = p
    }
    return .hostPort(host: NWEndpoint.Host(host), port: NWEndpoint.Port(rawValue: port)!)
}

/// Runs one session from the command line and exits when it ends.
final class CLISession {
    let session: HostSession
    let options: Options
    var onClosed: (() -> Void)?
    private var stopTimer: Timer?
    private var signalSources: [DispatchSourceSignal] = []

    init(endpoint: NWEndpoint, options: Options) {
        self.options = options
        session = HostSession(endpoint: endpoint, store: PairingStore(),
                              options: SessionOptions(autoConfirmSAS: options.yes, clipboard: options.clipboard))
    }

    func run() {
        session.onConfirmCode = { code, reply in
            FileHandle.standardError.write(Data("Does the device show \(code)? [y/N] ".utf8))
            DispatchQueue.global().async {
                let answer = readLine()?.trimmingCharacters(in: .whitespaces).lowercased()
                reply(answer == "y" || answer == "yes")
            }
        }
        session.onStateChange = { [weak self] state in
            guard let self else { return }
            switch state {
            case .streaming(let info):
                Log.info("streaming \(info.width)x\(info.height)@\(info.refreshHz) to \(info.deviceName)\(info.mirroring ? " (mirroring)" : "")")
                if let s = self.options.seconds, self.stopTimer == nil {
                    self.stopTimer = Timer.scheduledTimer(withTimeInterval: s, repeats: false) { _ in self.session.stop() }
                }
            case .closed(let error):
                self.onClosed?()
                // Let the BYE drain before exiting.
                DispatchQueue.main.asyncAfter(deadline: .now() + 0.3) { exit(error == nil ? 0 : 1) }
            default:
                break
            }
        }
        for sig in [SIGINT, SIGTERM] {
            signal(sig, SIG_IGN)
            let src = DispatchSource.makeSignalSource(signal: sig, queue: .main)
            src.setEventHandler { [weak self] in self?.session.stop(reason: .shutdown) }
            src.resume()
            signalSources.append(src)
        }
        session.start()
    }
}

// MARK: - Entry

let app = NSApplication.shared
// Virtual displays only come online while an AppKit event loop runs, so every mode runs one.
app.setActivationPolicy(.accessory)

let argv = Array(CommandLine.arguments.dropFirst())
let command = argv.first ?? "app"
let opts = parse(Array(argv.dropFirst()))
Log.verbose = opts.verbose
var cli: CLISession?
var appController: AppController?
var discovery: (BonjourBrowser, UsbMonitor?)?

switch command {
case "connect":
    guard let target = opts.positional.first else { fail("connect needs HOST[:PORT]\n\n\(usage)") }
    Log.echoToStderr = true
    cli = CLISession(endpoint: endpoint(target), options: opts)
    cli?.run()

case "usb":
    Log.echoToStderr = true
    guard let adb = Adb() else { fail("adb not found (PATH, ~/Library/Android/sdk/platform-tools, /opt/homebrew/bin; or set ANYDISPLAY_ADB)") }
    let devices = adb.devices()
    let serial: String
    if let s = opts.positional.first {
        serial = s
    } else if devices.count == 1 {
        serial = devices[0].serial
    } else {
        fail(devices.isEmpty ? "no USB device with debugging authorized" :
             "several devices, pick one: \(devices.map(\.serial).joined(separator: " "))")
    }
    guard let port = adb.forward(serial: serial) else { fail("adb forward failed for \(serial)") }
    Log.info("adb forward tcp:\(port) -> \(serial):\(anyDisplayPort)")
    cli = CLISession(endpoint: .hostPort(host: "127.0.0.1", port: NWEndpoint.Port(rawValue: port)!), options: opts)
    cli?.onClosed = { adb.removeForward(serial: serial, localPort: port) }
    cli?.run()

case "discover":
    let browser = BonjourBrowser()
    browser.onChange = { devices in
        print("wifi: " + (devices.isEmpty ? "(none)" : devices.map { "\($0.name) id=\($0.deviceID ?? "?")" }.joined(separator: ", ")))
    }
    browser.start()
    var monitor: UsbMonitor?
    if let adb = Adb() {
        monitor = UsbMonitor(adb: adb)
        monitor?.onChange = { devices in
            print("usb (\(adb.path)): " + (devices.isEmpty ? "(none)" : devices.map { "\($0.serial) \($0.model ?? "")" }.joined(separator: ", ")))
        }
        monitor?.start()
    } else {
        print("usb: adb not found")
    }
    discovery = (browser, monitor)
    DispatchQueue.main.asyncAfter(deadline: .now() + (opts.seconds ?? 5)) { exit(0) }

case "paired":
    let store = PairingStore()
    print("host_id \(store.hostID.hex)  (\(store.url.path))")
    for (id, d) in store.devices.sorted(by: { $0.key < $1.key }) { print("\(id)  \(d.name)  paired \(d.pairedAt)") }
    exit(0)

case "forget":
    let store = PairingStore()
    if opts.positional.first == "--all" {
        store.forgetAll()
    } else if let id = opts.positional.first, let data = Data(hex: id), data.count == 16 {
        store.forget(deviceID: data)
    } else {
        fail("forget needs a 32-hex-char DEVICE_ID or --all")
    }
    exit(0)

case "app":
    appController = AppController()
    appController?.start()

case "help", "-h", "--help":
    print(usage)
    exit(0)

default:
    fail("unknown command \(command)\n\n\(usage)")
}

app.run()
