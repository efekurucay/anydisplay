import XCTest
@testable import AnyDisplayHost

final class DiscoveryTests: XCTestCase {
    func testDeviceListWithMissingSerialAndWindowsLineEndings() {
        let output = "List of devices attached\r\n"
            + "(no serial number) device product:9469X_RU model:9469X transport_id:1\r\n"
            + "(no serial number) unauthorized transport_id:2\r\n"
            + "R58N12345 device model:SM_T870 transport_id:3\r\n"
            + "emulator-5554 offline\r\n0123 device\r\n\r\n"
        let devices = Adb.parseDevices(output)
        XCTAssertEqual(devices.map(\.serial), ["(no serial number)", "R58N12345", "0123"])
        XCTAssertEqual(devices.map(\.model), ["9469X", "SM_T870", nil])
    }

    func testIncompleteLinesAreIgnored() {
        for output in ["", "\n\n", "List of devices attached", "serial", "(no serial number)", "(no serial number) offline"] {
            XCTAssertTrue(Adb.parseDevices(output).isEmpty, output)
        }
    }

    func testForwardAndCleanupUseExplicitDeviceSelection() {
        for command in [["forward", "tcp:0", "tcp:47820"], ["forward", "--remove", "tcp:12345"]] {
            XCTAssertEqual(Adb.targeted(serial: Adb.noSerial, arguments: command), ["-d"] + command)
            XCTAssertEqual(Adb.targeted(serial: "R58N12345", arguments: command), ["-s", "R58N12345"] + command)
        }
    }
}
