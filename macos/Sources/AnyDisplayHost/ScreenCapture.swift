import AVFoundation
import CoreGraphics
import CoreMedia
import Foundation
import ScreenCaptureKit

public enum CaptureError: Error, CustomStringConvertible {
    case displayNotFound(CGDirectDisplayID)
    case content(Error)
    case start(Error)

    public var description: String {
        switch self {
        case .displayNotFound(let id): return "display \(id) not available to ScreenCaptureKit"
        case .content(let e): return "SCShareableContent: \(e.localizedDescription)"
        case .start(let e): return "SCStream start: \(e.localizedDescription)"
        }
    }
}

/// Captures one display with ScreenCaptureKit, scaled to the stream size, as 4:2:0 bi-planar YCbCr pixel buffers.
/// Optionally captures system audio as 48 kHz stereo.
public final class ScreenCapturer: NSObject, SCStreamOutput, SCStreamDelegate {
    public struct Settings {
        public var displayID: CGDirectDisplayID
        public var width: Int
        public var height: Int
        public var fps: Int
        public var showsCursor: Bool
        public var captureAudio: Bool
    }

    public let settings: Settings
    private var stream: SCStream?
    private let videoQueue = DispatchQueue(label: "anydisplay.capture.video", qos: .userInteractive)
    private let audioQueue = DispatchQueue(label: "anydisplay.capture.audio", qos: .userInteractive)
    private let onVideo: (CVPixelBuffer, CMTime) -> Void
    private let onAudio: (CMSampleBuffer) -> Void
    private let onStop: (Error) -> Void

    public static let audioSampleRate = 48_000
    public static let audioChannels = 2

    public init(settings: Settings, onVideo: @escaping (CVPixelBuffer, CMTime) -> Void,
                onAudio: @escaping (CMSampleBuffer) -> Void, onStop: @escaping (Error) -> Void) {
        self.settings = settings
        self.onVideo = onVideo
        self.onAudio = onAudio
        self.onStop = onStop
    }

    /// Looks the display up (retrying while a new virtual display registers) and starts capturing.
    public func start(completion: @escaping (Error?) -> Void) {
        find(attempt: 0) { [weak self] result in
            guard let self else { return }
            switch result {
            case .failure(let e): completion(e)
            case .success(let display): self.start(display: display, completion: completion)
            }
        }
    }

    private func find(attempt: Int, completion: @escaping (Result<SCDisplay, Error>) -> Void) {
        SCShareableContent.getExcludingDesktopWindows(false, onScreenWindowsOnly: true) { content, error in
            if let display = content?.displays.first(where: { $0.displayID == self.settings.displayID }) {
                completion(.success(display))
            } else if attempt < 30 {
                DispatchQueue.global().asyncAfter(deadline: .now() + 0.1) { self.find(attempt: attempt + 1, completion: completion) }
            } else if let error {
                completion(.failure(CaptureError.content(error)))
            } else {
                completion(.failure(CaptureError.displayNotFound(self.settings.displayID)))
            }
        }
    }

    private func start(display: SCDisplay, completion: @escaping (Error?) -> Void) {
        let filter = SCContentFilter(display: display, excludingWindows: [])
        let config = SCStreamConfiguration()
        config.width = settings.width
        config.height = settings.height
        config.minimumFrameInterval = CMTime(value: 1, timescale: CMTimeScale(max(settings.fps, 1)))
        config.pixelFormat = kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange
        config.colorMatrix = CGDisplayStream.yCbCrMatrix_ITU_R_709_2
        config.colorSpaceName = CGColorSpace.sRGB
        config.showsCursor = settings.showsCursor
        config.queueDepth = 5
        config.scalesToFit = true
        if settings.captureAudio {
            config.capturesAudio = true
            config.sampleRate = Self.audioSampleRate
            config.channelCount = Self.audioChannels
            config.excludesCurrentProcessAudio = true
        }
        let s = SCStream(filter: filter, configuration: config, delegate: self)
        do {
            try s.addStreamOutput(self, type: .screen, sampleHandlerQueue: videoQueue)
            if settings.captureAudio { try s.addStreamOutput(self, type: .audio, sampleHandlerQueue: audioQueue) }
        } catch {
            completion(CaptureError.start(error))
            return
        }
        stream = s
        s.startCapture { error in completion(error.map { CaptureError.start($0) }) }
    }

    public func stop() {
        guard let s = stream else { return }
        stream = nil
        s.stopCapture { _ in }
    }

    // MARK: SCStreamOutput

    public func stream(_ stream: SCStream, didOutputSampleBuffer sample: CMSampleBuffer, of type: SCStreamOutputType) {
        switch type {
        case .screen:
            guard let attachments = CMSampleBufferGetSampleAttachmentsArray(sample, createIfNecessary: false) as? [[SCStreamFrameInfo: Any]],
                  let raw = attachments.first?[.status] as? Int, SCFrameStatus(rawValue: raw) == .complete,
                  let pb = CMSampleBufferGetImageBuffer(sample) else { return }
            onVideo(pb, CMSampleBufferGetPresentationTimeStamp(sample))
        case .audio:
            onAudio(sample)
        default:
            break
        }
    }

    // MARK: SCStreamDelegate

    public func stream(_ stream: SCStream, didStopWithError error: Error) {
        Log.error("capture stopped: \(error.localizedDescription)")
        onStop(error)
    }
}

/// Converts ScreenCaptureKit audio (float32, usually non-interleaved) to interleaved PCM s16le.
enum AudioConverter {
    static func s16le(_ sample: CMSampleBuffer) -> Data? {
        guard let format = CMSampleBufferGetFormatDescription(sample),
              let asbd = CMAudioFormatDescriptionGetStreamBasicDescription(format)?.pointee,
              asbd.mFormatID == kAudioFormatLinearPCM else { return nil }
        let frames = CMSampleBufferGetNumSamples(sample)
        let channels = Int(asbd.mChannelsPerFrame)
        guard frames > 0, channels > 0 else { return nil }
        let isFloat = asbd.mFormatFlags & kAudioFormatFlagIsFloat != 0
        let nonInterleaved = asbd.mFormatFlags & kAudioFormatFlagIsNonInterleaved != 0
        guard isFloat, asbd.mBitsPerChannel == 32 else { return nil }

        var blockBuffer: CMBlockBuffer?
        let ablSize = MemoryLayout<AudioBufferList>.size + MemoryLayout<AudioBuffer>.size * max(channels - 1, 0)
        let abl = UnsafeMutableRawPointer.allocate(byteCount: ablSize, alignment: MemoryLayout<AudioBufferList>.alignment)
        defer { abl.deallocate() }
        let ablPtr = abl.bindMemory(to: AudioBufferList.self, capacity: 1)
        guard CMSampleBufferGetAudioBufferListWithRetainedBlockBuffer(
            sample, bufferListSizeNeededOut: nil, bufferListOut: ablPtr, bufferListSize: ablSize,
            blockBufferAllocator: nil, blockBufferMemoryAllocator: nil,
            flags: kCMSampleBufferFlag_AudioBufferList_Assure16ByteAlignment, blockBufferOut: &blockBuffer) == noErr else {
            return nil
        }
        let buffers = UnsafeMutableAudioBufferListPointer(ablPtr)
        var out = Data(count: frames * channels * 2)
        out.withUnsafeMutableBytes { raw in
            let dst = raw.bindMemory(to: Int16.self)
            for c in 0..<channels {
                for f in 0..<frames {
                    let v: Float
                    if nonInterleaved {
                        guard c < buffers.count, let p = buffers[c].mData else { continue }
                        v = p.assumingMemoryBound(to: Float.self)[f]
                    } else {
                        guard let p = buffers[0].mData else { continue }
                        v = p.assumingMemoryBound(to: Float.self)[f * channels + c]
                    }
                    dst[f * channels + c] = Int16(max(-1, min(1, v)) * 32767).littleEndian
                }
            }
        }
        return out
    }
}
