import Foundation
import CoreVideo
import CoreMedia
import AnyDisplayProtocol

@main struct Bench {
    static func main() throws {
        let a = CommandLine.arguments
        let input = try Data(contentsOf: URL(fileURLWithPath: a[1]), options: .mappedIfSafe)
        let width = Int(a[2])!, height = Int(a[3])!, fps = Int(a[4])!, kbps = Int(a[5])!
        let codec: Codec = a[6] == "hevc" ? .hevc : .h264
        let output = a[7]
        let count = input.count / (width * height * 3 / 2)
        _ = FileManager.default.createFile(atPath: output, contents: nil)
        let file = try FileHandle(forWritingTo: URL(fileURLWithPath: output))
        let lock = NSLock(), group = DispatchGroup()
        var submitted = [UInt64](repeating: 0, count: count)
        var timings = [Double](), sizes = [Int](), keys = [Bool](), drops = 0
        let encoder = try VideoEncoder(codec: codec, width: width, height: height, fps: fps, bitrateKbps: kbps,
            onOutput: { out in
                let now = DispatchTime.now().uptimeNanoseconds
                let index = Int(CMTimeGetSeconds(out.pts) * Double(fps) + 0.5)
                lock.lock()
                if index >= 30 { timings.append(Double(now - submitted[index]) / 1_000_000) }
                sizes.append(out.accessUnit.count)
                keys.append(out.keyframe)
                file.write(out.accessUnit)
                lock.unlock()
                group.leave()
            }, onDrop: {
                lock.lock(); drops += 1; lock.unlock()
                group.leave()
            })
        let start = DispatchTime.now().uptimeNanoseconds
        for index in 0..<count {
            try autoreleasepool {
                var pixel: CVPixelBuffer?
                let status = CVPixelBufferCreate(nil, width, height, kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange,
                    [kCVPixelBufferIOSurfacePropertiesKey: [:]] as CFDictionary, &pixel)
                guard status == kCVReturnSuccess, let pixel else { throw NSError(domain: "pixel", code: Int(status)) }
                CVPixelBufferLockBaseAddress(pixel, [])
                input.withUnsafeBytes { bytes in
                    let source = bytes.baseAddress!.advanced(by: index * width * height * 3 / 2)
                    for plane in 0...1 {
                        let dest = CVPixelBufferGetBaseAddressOfPlane(pixel, plane)!
                        let stride = CVPixelBufferGetBytesPerRowOfPlane(pixel, plane)
                        let rows = plane == 0 ? height : height / 2
                        let offset = plane == 0 ? 0 : width * height
                        for row in 0..<rows {
                            memcpy(dest.advanced(by: row * stride), source.advanced(by: offset + row * width), width)
                        }
                    }
                }
                CVPixelBufferUnlockBaseAddress(pixel, [])
                CVBufferSetAttachment(pixel, kCVImageBufferYCbCrMatrixKey, kCVImageBufferYCbCrMatrix_ITU_R_709_2, .shouldPropagate)
                CVBufferSetAttachment(pixel, kCVImageBufferColorPrimariesKey, kCVImageBufferColorPrimaries_ITU_R_709_2, .shouldPropagate)
                CVBufferSetAttachment(pixel, kCVImageBufferTransferFunctionKey, kCVImageBufferTransferFunction_ITU_R_709_2, .shouldPropagate)
                let deadline = start + UInt64(index) * 1_000_000_000 / UInt64(fps)
                let now = DispatchTime.now().uptimeNanoseconds
                if deadline > now { usleep(useconds_t((deadline - now) / 1000)) }
                group.enter()
                lock.lock(); submitted[index] = DispatchTime.now().uptimeNanoseconds; lock.unlock()
                encoder.encode(pixel, pts: CMTime(value: Int64(index), timescale: Int32(fps)), forceKeyframe: index == 0)
            }
        }
        guard group.wait(timeout: .now() + 10) == .success else { fatalError("encoder timeout") }
        try file.close()
        guard drops == 0 && sizes.count == count else { fatalError("Incomplete encode: \(sizes.count)/\(count), drops=\(drops)") }
        timings.sort()
        let total = sizes.reduce(0, +)
        let result: [String: Any] = ["codec": a[6], "target_kbps": kbps, "width": width, "height": height,
            "fps": fps, "input_frames": count, "output_frames": sizes.count, "dropped": drops,
            "hardware": encoder.isHardware, "actual_kbps": Double(total * 8 * fps) / Double(count * 1000),
            "encode_median_ms": timings[timings.count / 2], "encode_p95_ms": timings[Int(Double(timings.count - 1) * 0.95)],
            "frame_sizes": sizes, "keyframes": keys]
        try JSONSerialization.data(withJSONObject: result, options: [.prettyPrinted, .sortedKeys]).write(to: URL(fileURLWithPath: output + ".json"))
        print("\(a[6]) \(kbps)kbps hw=\(encoder.isHardware) frames=\(sizes.count)/\(count) p50=\(timings[timings.count / 2])ms p95=\(timings[Int(Double(timings.count - 1) * 0.95)])ms actual=\(Double(total * 8 * fps) / Double(count * 1000))kbps")
        encoder.invalidate()
    }
}
