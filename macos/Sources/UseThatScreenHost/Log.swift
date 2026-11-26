import Foundation
import os

/// Logs to the unified log (subsystem `com.usethatscreen.host`) and, for the CLI, to stderr.
public enum Log {
    public static var echoToStderr = false
    public static var verbose = false
    private static let logger = Logger(subsystem: "com.usethatscreen.host", category: "host")

    public static func info(_ message: @autoclosure () -> String) {
        let m = message()
        logger.info("\(m, privacy: .public)")
        if echoToStderr { write("[usethatscreen] " + m) }
    }

    public static func debug(_ message: @autoclosure () -> String) {
        guard verbose else { return }
        let m = message()
        logger.debug("\(m, privacy: .public)")
        if echoToStderr { write("[usethatscreen] " + m) }
    }

    public static func error(_ message: @autoclosure () -> String) {
        let m = message()
        logger.error("\(m, privacy: .public)")
        if echoToStderr { write("[usethatscreen] error: " + m) }
    }

    private static func write(_ line: String) {
        FileHandle.standardError.write(Data((line + "\n").utf8))
    }
}
