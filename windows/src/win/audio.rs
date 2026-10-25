//! WASAPI loopback capture of the default render device → PCM s16le 48 kHz stereo AUDIO_DATA.

use super::ComGuard;
use crate::audio_convert::{AudioConverter, InputFormat, SampleFormat, CHANNELS, SAMPLE_RATE};
use crate::platform::{AudioStream, FrameSink};
use crate::proto::messages::{AudioConfig, AudioData, Message};
use crate::util::now_us;
use anyhow::{anyhow, Context};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::thread::JoinHandle;
use std::time::Duration;
use windows::Win32::Media::Audio::*;
use windows::Win32::Media::KernelStreaming::WAVE_FORMAT_EXTENSIBLE;
use windows::Win32::Media::Multimedia::{KSDATAFORMAT_SUBTYPE_IEEE_FLOAT, WAVE_FORMAT_IEEE_FLOAT};
use windows::Win32::System::Com::{CoCreateInstance, CoTaskMemFree, CLSCTX_ALL};

struct Capture {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Drop for Capture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

pub fn start(sink: FrameSink) -> anyhow::Result<AudioStream> {
    let stop = Arc::new(AtomicBool::new(false));
    let (ready_tx, ready_rx) = mpsc::channel::<Result<(), String>>();
    let s = stop.clone();
    let thread = std::thread::Builder::new()
        .name("anydisplay-audio".into())
        .spawn(move || {
            let _com = ComGuard::new();
            match open() {
                Ok((client, capture, format)) => {
                    let _ = ready_tx.send(Ok(()));
                    run(client, capture, format, sink, s);
                }
                Err(e) => {
                    let _ = ready_tx.send(Err(format!("{e:#}")));
                }
            }
        })?;
    ready_rx
        .recv()
        .map_err(|_| anyhow!("audio thread exited"))?
        .map_err(|e| anyhow!(e))?;
    Ok(AudioStream {
        config: AudioConfig {
            sample_rate: SAMPLE_RATE,
            channels: CHANNELS as u8,
            codec: 0,
        },
        handle: Box::new(Capture {
            stop,
            thread: Some(thread),
        }),
    })
}

fn open() -> anyhow::Result<(IAudioClient, IAudioCaptureClient, InputFormat)> {
    unsafe {
        let enumerator: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
        let device = enumerator
            .GetDefaultAudioEndpoint(eRender, eConsole)
            .context("no default audio output")?;
        let client: IAudioClient = device.Activate(CLSCTX_ALL, None)?;
        let mix = client.GetMixFormat()?;
        let format = parse_format(mix);
        // 100 ms buffer; loopback runs at the device rate in the mix format, converted here.
        let r = client.Initialize(
            AUDCLNT_SHAREMODE_SHARED,
            AUDCLNT_STREAMFLAGS_LOOPBACK,
            1_000_000,
            0,
            mix,
            None,
        );
        CoTaskMemFree(Some(mix as *const _));
        r.context("IAudioClient::Initialize (loopback)")?;
        let format = format?;
        let capture: IAudioCaptureClient = client.GetService()?;
        client.Start()?;
        crate::info!(
            "audio loopback: {} Hz, {} ch, {:?}",
            format.rate,
            format.channels,
            format.format
        );
        Ok((client, capture, format))
    }
}

unsafe fn parse_format(f: *const WAVEFORMATEX) -> anyhow::Result<InputFormat> {
    let w = unsafe { std::ptr::read_unaligned(f) };
    let float = match w.wFormatTag as u32 {
        WAVE_FORMAT_EXTENSIBLE => {
            let ext = unsafe { std::ptr::read_unaligned(f as *const WAVEFORMATEXTENSIBLE) };
            let sub = ext.SubFormat;
            sub == KSDATAFORMAT_SUBTYPE_IEEE_FLOAT
        }
        t => t == WAVE_FORMAT_IEEE_FLOAT,
    };
    let format = match (float, w.wBitsPerSample) {
        (true, 32) => SampleFormat::F32,
        (false, 16) => SampleFormat::S16,
        (false, 32) => SampleFormat::S32,
        (fl, bits) => return Err(anyhow!("unsupported mix format: float={fl} bits={bits}")),
    };
    Ok(InputFormat {
        format,
        channels: w.nChannels,
        rate: w.nSamplesPerSec,
    })
}

fn run(
    client: IAudioClient,
    capture: IAudioCaptureClient,
    format: InputFormat,
    sink: FrameSink,
    stop: Arc<AtomicBool>,
) {
    let mut conv = AudioConverter::new(format);
    let bytes_per_frame = match format.format {
        SampleFormat::S16 => 2,
        _ => 4,
    } * format.channels.max(1) as usize;
    while !stop.load(Ordering::Relaxed) {
        std::thread::sleep(Duration::from_millis(10));
        loop {
            let packet = unsafe { capture.GetNextPacketSize() };
            match packet {
                Ok(0) => break,
                Ok(_) => {}
                Err(e) => {
                    crate::warn!("audio capture stopped: {e}");
                    return;
                }
            }
            let (mut data, mut frames, mut flags) = (std::ptr::null_mut(), 0u32, 0u32);
            if unsafe { capture.GetBuffer(&mut data, &mut frames, &mut flags, None, None) }.is_err()
            {
                break;
            }
            let len = frames as usize * bytes_per_frame;
            let pcm = if flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0 || data.is_null() {
                conv.convert(&vec![0u8; len])
            } else {
                conv.convert(unsafe { std::slice::from_raw_parts(data, len) })
            };
            unsafe {
                let _ = capture.ReleaseBuffer(frames);
            }
            if !pcm.is_empty() {
                sink(
                    AudioData {
                        pts_us: now_us(),
                        samples: pcm,
                    }
                    .frame(),
                );
            }
        }
    }
    unsafe {
        let _ = client.Stop();
    }
}
