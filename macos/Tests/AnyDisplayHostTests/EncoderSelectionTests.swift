import XCTest
import AnyDisplayProtocol
@testable import AnyDisplayHost

final class EncoderSelectionTests: XCTestCase {
    private enum Unavailable: Error { case hevc, h264 }

    private func bitrate(_ codec: Codec) -> Int {
        HostSession.autoBitrate(width: 1920, height: 1080, fps: 60, quality: 80, codec: codec)
    }

    func testAutomaticBitrateMatchesFallbackCodec() throws {
        var attempted: [Int] = []
        let selection = try HostSession.selectEncoder(preferred: .hevc, supportsH264: true, bitrateFor: bitrate) { codec, kbps in
            attempted.append(kbps)
            if codec == .hevc { throw Unavailable.hevc }
            return "H.264 encoder"
        }
        XCTAssertEqual(attempted, [8883, 12690])
        XCTAssertEqual(selection.codec, .h264)
        XCTAssertEqual(selection.bitrateKbps, 12690)
        XCTAssertEqual(selection.encoder, "H.264 encoder")
    }

    func testExplicitBitrateIsPreservedDuringFallback() throws {
        var attempted: [Int] = []
        let selection = try HostSession.selectEncoder(preferred: .hevc, supportsH264: true, bitrateFor: { _ in 7000 }) { codec, kbps in
            attempted.append(kbps)
            if codec == .hevc { throw Unavailable.hevc }
            return codec
        }
        XCTAssertEqual(attempted, [7000, 7000])
        XCTAssertEqual(selection.codec, .h264)
        XCTAssertEqual(selection.bitrateKbps, 7000)
    }

    func testWorkingHEVCDoesNotFallBack() throws {
        let selection = try HostSession.selectEncoder(preferred: .hevc, supportsH264: true, bitrateFor: bitrate) { codec, _ in
            XCTAssertEqual(codec, .hevc)
            return codec
        }
        XCTAssertEqual(selection.codec, .hevc)
        XCTAssertEqual(selection.bitrateKbps, 8883)
    }

    func testUnsupportedH264DoesNotGetRequested() {
        var attempted: [Codec] = []
        XCTAssertThrowsError(try HostSession.selectEncoder(preferred: .hevc, supportsH264: false, bitrateFor: bitrate) { codec, _ -> Int in
            attempted.append(codec)
            throw Unavailable.hevc
        }) { error in
            XCTAssertEqual(error as? Unavailable, .hevc)
        }
        XCTAssertEqual(attempted, [.hevc])
    }

    func testH264FailureIsPropagated() {
        var attempted: [Codec] = []
        XCTAssertThrowsError(try HostSession.selectEncoder(preferred: .h264, supportsH264: true, bitrateFor: bitrate) { codec, _ -> Int in
            attempted.append(codec)
            throw Unavailable.h264
        }) { error in
            XCTAssertEqual(error as? Unavailable, .h264)
        }
        XCTAssertEqual(attempted, [.h264])
    }
}
