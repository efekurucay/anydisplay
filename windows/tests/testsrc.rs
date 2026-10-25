//! The ffmpeg test source (needs `ffmpeg` with libx264 on PATH; skipped otherwise).

use anydisplay::platform::VideoPipeline;
use anydisplay::proto::Codec;
use anydisplay::testsrc::FfmpegPipeline;

fn ffmpeg_available() -> bool {
    std::process::Command::new("ffmpeg")
        .arg("-version")
        .output()
        .is_ok_and(|o| o.status.success())
}

#[test]
fn first_frame_is_keyframe_and_requests_produce_keyframes() {
    if !ffmpeg_available() {
        eprintln!("ffmpeg not found; skipping");
        return;
    }
    let mut p = FfmpegPipeline::start("ffmpeg", 320, 240, 30, Codec::H264, 500).unwrap();
    let first = p.next_frame(false).unwrap().unwrap();
    assert!(first.keyframe);
    assert!(anydisplay::annexb::has_parameter_sets(&first.data, Codec::H264));
    for _ in 0..5 {
        assert!(!p.next_frame(false).unwrap().unwrap().keyframe);
    }
    let forced = p.next_frame(true).unwrap().unwrap();
    assert!(forced.keyframe);
    assert!(anydisplay::annexb::has_parameter_sets(&forced.data, Codec::H264));
}
