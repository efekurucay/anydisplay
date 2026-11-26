import AVFoundation
import CoreMedia
import UIKit
import UseThatScreenProtocol

@MainActor
final class StreamRenderer {
    let layer = AVSampleBufferDisplayLayer()
    private var format: CMVideoFormatDescription?

    init() {
        layer.videoGravity = .resizeAspect
    }

    func reset() {
        format = nil
        layer.sampleBufferRenderer.flush(removingDisplayedImage: true, completionHandler: nil)
    }

    /// The host sends Annex B; AVFoundation expects a four-byte length before each NAL.
    func enqueue(_ frame: VideoFrame) -> Bool {
        let renderer = layer.sampleBufferRenderer
        if renderer.status == .failed || renderer.requiresFlushToResumeDecoding {
            reset()
        }
        guard renderer.isReadyForMoreMediaData else {
            format = nil
            return false
        }
        let nals = Self.splitNALs(frame.accessUnit)
        if frame.isKeyframe {
            guard let sps = nals.first(where: { Self.nalType($0) == 7 }),
                  let pps = nals.first(where: { Self.nalType($0) == 8 }),
                  let newFormat = Self.makeFormat(sps: sps, pps: pps) else { return false }
            format = newFormat
        }
        guard let format else { return false }
        var avcc = Data()
        var hasPicture = false
        for nal in nals {
            let type = Self.nalType(nal)
            if type == 7 || type == 8 || type == 9 { continue }
            if (1...5).contains(type) { hasPicture = true }
            withUnsafeBytes(of: UInt32(nal.count).bigEndian) { avcc.append(contentsOf: $0) }
            avcc.append(nal)
        }
        guard hasPicture, let sample = Self.makeSample(avcc, format: format, frame: frame) else { return false }
        renderer.enqueue(sample)
        return true
    }

    static func splitNALs(_ data: Data) -> [Data] {
        let bytes = [UInt8](data)
        var nals: [Data] = []
        var start: Int?
        var zeros = 0
        for (index, byte) in bytes.enumerated() {
            if byte == 0 {
                zeros += 1
                continue
            }
            if byte == 1 && zeros >= 2 {
                let end = index - zeros
                if let start, start < end { nals.append(Data(bytes[start..<end])) }
                start = index + 1
            }
            zeros = 0
        }
        let end = bytes.count - zeros
        if let start, start < end { nals.append(Data(bytes[start..<end])) }
        return nals
    }

    private static func nalType(_ nal: Data) -> UInt8 { (nal.first ?? 0) & 0x1f }

    private static func makeFormat(sps: Data, pps: Data) -> CMVideoFormatDescription? {
        var format: CMVideoFormatDescription?
        let status = sps.withUnsafeBytes { spsBytes in
            pps.withUnsafeBytes { ppsBytes in
                let pointers = [spsBytes.bindMemory(to: UInt8.self).baseAddress!,
                                ppsBytes.bindMemory(to: UInt8.self).baseAddress!]
                return CMVideoFormatDescriptionCreateFromH264ParameterSets(
                    allocator: kCFAllocatorDefault, parameterSetCount: 2, parameterSetPointers: pointers,
                    parameterSetSizes: [sps.count, pps.count], nalUnitHeaderLength: 4, formatDescriptionOut: &format)
            }
        }
        return status == noErr ? format : nil
    }

    private static func makeSample(_ data: Data, format: CMVideoFormatDescription, frame: VideoFrame) -> CMSampleBuffer? {
        var storage: CMBlockBuffer?
        guard CMBlockBufferCreateWithMemoryBlock(allocator: kCFAllocatorDefault, memoryBlock: nil,
                   blockLength: data.count, blockAllocator: kCFAllocatorDefault, customBlockSource: nil, offsetToData: 0,
                   dataLength: data.count, flags: 0, blockBufferOut: &storage) == noErr, let storage else { return nil }
        let copied = data.withUnsafeBytes { buffer in
            CMBlockBufferReplaceDataBytes(with: buffer.baseAddress!, blockBuffer: storage,
                                          offsetIntoDestination: 0, dataLength: data.count)
        }
        guard copied == noErr else { return nil }

        var timing = CMSampleTimingInfo(duration: .invalid,
                                       presentationTimeStamp: CMTime(value: Int64(clamping: frame.ptsUs), timescale: 1_000_000),
                                       decodeTimeStamp: .invalid)
        var size = data.count
        var sample: CMSampleBuffer?
        guard CMSampleBufferCreateReady(allocator: kCFAllocatorDefault, dataBuffer: storage, formatDescription: format,
                   sampleCount: 1, sampleTimingEntryCount: 1, sampleTimingArray: &timing,
                   sampleSizeEntryCount: 1, sampleSizeArray: &size, sampleBufferOut: &sample) == noErr,
              let sample else { return nil }
        if let attachments = CMSampleBufferGetSampleAttachmentsArray(sample, createIfNecessary: true) {
            let dictionary = unsafeBitCast(CFArrayGetValueAtIndex(attachments, 0), to: CFMutableDictionary.self)
            CFDictionarySetValue(dictionary, Unmanaged.passUnretained(kCMSampleAttachmentKey_DisplayImmediately).toOpaque(),
                                 Unmanaged.passUnretained(kCFBooleanTrue).toOpaque())
        }
        return sample
    }
}
