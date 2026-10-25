import Foundation
import os

/// Logs to the unified log (subsystem `dev.anydisplay.host`) and, for the CLI, to stderr.
public enum Log {
    public static var echoToStderr = false
    public static var verbose = false
    private static let logger = Logger(subsystem: "dev.anydisplay.host", category: "host")

    public static func info(_ message: @autoclosure () -> String) {
        let m = message()
        logger.info("\(m, privacy: .public)")
        if echoToStderr { write("[anydisplay] " + m) }
    }

    public static func debug(_ message: @autoclosure () -> String) {
        guard verbose else { return }
        let m = message()
        logger.debug("\(m, privacy: .public)")
        if echoToStderr { write("[anydisplay] " + m) }
    }

    public static func error(_ message: @autoclosure () -> String) {
        let m = message()
        logger.error("\(m, privacy: .public)")
        if echoToStderr { write("[anydisplay] error: " + m) }
    }

    private static func write(_ line: String) {
        FileHandle.standardError.write(Data((line + "\n").utf8))
    }
}
