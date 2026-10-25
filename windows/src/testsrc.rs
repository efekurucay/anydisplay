//! A platform without a display: streams an ffmpeg test pattern (`testsrc2`) and logs input.
//! Runs on any OS and proves protocol interop end to end (README "Demo on macOS / Linux").

use crate::annexb::{self, AccessUnitSplitter};
use crate::negotiate::auto_bitrate;
use crate::platform::{
    EncodedFrame, FrameSink, OpenedStream, Platform, StreamRequest, VideoPipeline,
};
use crate::proto::messages::{stream_flags, Codec, InputEvent};
use crate::util::now_us;
use anyhow::{bail, Context};
use std::io::Read;
use std::process::{Child, ChildStdout, Command, Stdio};

pub struct TestSource {
    pub ffmpeg: String,
}

impl Default for TestSource {
    fn default() -> Self {
        TestSource {
            ffmpeg: std::env::var("ANYDISPLAY_FFMPEG").unwrap_or_else(|_| "ffmpeg".into()),
        }
    }
}

impl Platform for TestSource {
    fn host_os(&self) -> String {
        format!("anydisplay-win test source ({})", std::env::consts::OS)
    }

    fn host_caps(&self) -> u32 {
        0
    }

    fn open_stream(
        &mut self,
        req: &StreamRequest<'_>,
        _cursor: FrameSink,
    ) -> anyhow::Result<OpenedStream> {
        let p = req.plan;
        let kbps = if p.bitrate_kbps == 0 {
            auto_bitrate(p.width, p.height, p.fps, p.quality, p.codec)
        } else {
            p.bitrate_kbps
        };
        let pipeline = FfmpegPipeline::start(
            &self.ffmpeg,
            p.width,
            p.height,
            p.fps.min(60),
            p.codec,
            kbps,
        )?;
        Ok(OpenedStream {
            width: p.width,
            height: p.height,
            fps: p.fps.min(60),
            codec: p.codec,
            bitrate_kbps: kbps,
            flags: p.flags & stream_flags::MIRROR,
            logical_w: p.width,
            logical_h: p.height,
            notices: vec![],
            hardware_encoder: false,
            source: format!(
                "ffmpeg testsrc2 ({})",
                if p.codec == Codec::Hevc {
                    "libx265"
                } else {
                    "libx264"
                }
            ),
            pipeline: Box::new(pipeline),
        })
    }

    fn inject(&mut self, event: InputEvent) {
        info!("input {event:?}");
    }
}

/// ffmpeg encoding `testsrc2` in real time with access unit delimiters and no B-frames.
/// A keyframe request restarts ffmpeg, whose first output is an IDR with in-band parameter sets.
pub struct FfmpegPipeline {
    ffmpeg: String,
    args: Vec<String>,
    codec: Codec,
    child: Child,
    stdout: ChildStdout,
    splitter: AccessUnitSplitter,
    frames_since_start: u64,
}

impl FfmpegPipeline {
    pub fn start(
        ffmpeg: &str,
        w: u32,
        h: u32,
        fps: u32,
        codec: Codec,
        kbps: u32,
    ) -> anyhow::Result<Self> {
        let enc: &[&str] = match codec {
            Codec::Hevc => &[
                "-c:v",
                "libx265",
                "-x265-params",
                "aud=1:keyint=120:bframes=0:repeat-headers=1:log-level=error",
            ],
            _ => &[
                "-c:v",
                "libx264",
                "-x264-params",
                "aud=1:keyint=120:bframes=0",
            ],
        };
        let mut args: Vec<String> = ["-loglevel", "error", "-re", "-f", "lavfi", "-i"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        args.push(format!("testsrc2=size={w}x{h}:rate={fps}"));
        args.extend(
            [
                "-pix_fmt",
                "yuv420p",
                "-tune",
                "zerolatency",
                "-preset",
                "ultrafast",
            ]
            .map(String::from),
        );
        args.extend(["-b:v".into(), format!("{kbps}k")]);
        args.extend(enc.iter().map(|s| s.to_string()));
        args.extend(
            [
                "-f",
                if codec == Codec::Hevc { "hevc" } else { "h264" },
                "-",
            ]
            .map(String::from),
        );
        let (child, stdout) = spawn(ffmpeg, &args)?;
        Ok(FfmpegPipeline {
            ffmpeg: ffmpeg.into(),
            args,
            codec,
            child,
            stdout,
            splitter: AccessUnitSplitter::new(codec),
            frames_since_start: 0,
        })
    }

    fn restart(&mut self) -> anyhow::Result<()> {
        let _ = self.child.kill();
        let _ = self.child.wait();
        (self.child, self.stdout) = spawn(&self.ffmpeg, &self.args)?;
        self.splitter = AccessUnitSplitter::new(self.codec);
        self.frames_since_start = 0;
        Ok(())
    }
}

fn spawn(ffmpeg: &str, args: &[String]) -> anyhow::Result<(Child, ChildStdout)> {
    let mut child = Command::new(ffmpeg)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .spawn()
        .with_context(|| format!("starting {ffmpeg} (set ANYDISPLAY_FFMPEG to its path)"))?;
    let stdout = child.stdout.take().expect("piped stdout");
    Ok((child, stdout))
}

impl VideoPipeline for FfmpegPipeline {
    fn next_frame(&mut self, force_keyframe: bool) -> anyhow::Result<Option<EncodedFrame>> {
        if force_keyframe && self.frames_since_start > 0 {
            debug!("restarting ffmpeg for a keyframe");
            self.restart()?;
        }
        let mut buf = [0u8; 64 * 1024];
        loop {
            if let Some(au) = self.splitter.next_au() {
                self.frames_since_start += 1;
                let keyframe = annexb::is_keyframe(&au, self.codec);
                return Ok(Some(EncodedFrame {
                    data: au,
                    keyframe,
                    pts_us: now_us(),
                }));
            }
            let n = self.stdout.read(&mut buf)?;
            if n == 0 {
                bail!("ffmpeg exited: {:?}", self.child.wait()?);
            }
            self.splitter.push(&buf[..n]);
        }
    }
}

impl Drop for FfmpegPipeline {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
