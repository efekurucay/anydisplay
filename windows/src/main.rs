//! `anydisplay-win`: command-line AnyDisplay host.

use anyhow::{anyhow, bail};
use anydisplay::adb::Adb;
use anydisplay::pairing::PairingStore;
use anydisplay::platform::Platform;
use anydisplay::proto::messages::{bye, Codec, DEFAULT_PORT};
use anydisplay::session::{self, SessionConfig, SessionHandle};
use anydisplay::{info, warn};
use std::io::Write;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

const USAGE: &str = "\
usage:
  anydisplay-win list [--seconds N]              monitors, USB (adb) and Wi-Fi (mDNS) clients, paired devices
  anydisplay-win usb [SERIAL] [options]          connect to a USB client through adb forward
  anydisplay-win connect HOST[:PORT] [options]   connect to a client over the network (default port 47820)

options:
  --monitor N        capture monitor N from `list` (default: the virtual display if installed,
                     else the primary monitor)
  --codec h264|hevc  preferred codec (used when the client can decode it)
  -y, --yes          accept the pairing code without asking
  --seconds N        end the session N seconds after streaming starts
  --no-clipboard     do not share the clipboard
  --no-audio         never stream audio
  --test-source      stream an ffmpeg test pattern instead of the desktop (any OS)
  -v, --verbose      debug logging

environment: ANYDISPLAY_STORE (pairing store path), ANYDISPLAY_ADB (adb path), ANYDISPLAY_FFMPEG (ffmpeg path)";

#[derive(Default)]
struct Opts {
    positional: Vec<String>,
    monitor: Option<usize>,
    codec: Option<Codec>,
    yes: bool,
    seconds: Option<f64>,
    clipboard: bool,
    audio: bool,
    test_source: bool,
}

fn parse(args: &[String]) -> anyhow::Result<Opts> {
    let mut o = Opts {
        clipboard: true,
        audio: true,
        ..Default::default()
    };
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut value = |name: &str| {
            it.next()
                .cloned()
                .ok_or_else(|| anyhow!("{name} needs a value"))
        };
        match a.as_str() {
            "-y" | "--yes" => o.yes = true,
            "-v" | "--verbose" => anydisplay::log::set_verbose(true),
            "--no-clipboard" => o.clipboard = false,
            "--no-audio" => o.audio = false,
            "--test-source" => o.test_source = true,
            "--monitor" => o.monitor = Some(value("--monitor")?.parse()?),
            "--seconds" => o.seconds = Some(value("--seconds")?.parse()?),
            "--codec" => {
                o.codec = Some(match value("--codec")?.to_ascii_lowercase().as_str() {
                    "h264" | "avc" => Codec::H264,
                    "hevc" | "h265" => Codec::Hevc,
                    other => bail!("unknown codec {other} (h264 or hevc)"),
                })
            }
            s if s.starts_with('-') => bail!("unknown option {s}"),
            _ => o.positional.push(a.clone()),
        }
    }
    Ok(o)
}

fn host_name() -> String {
    if let Ok(n) = std::env::var("COMPUTERNAME") {
        return n;
    }
    std::process::Command::new("hostname")
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "AnyDisplay host".into())
}

fn make_platform(o: &Opts) -> anyhow::Result<Box<dyn Platform>> {
    if o.test_source || cfg!(not(windows)) {
        if cfg!(not(windows)) && !o.test_source {
            info!("not on Windows: streaming the ffmpeg test pattern (--test-source)");
        }
        return Ok(Box::new(anydisplay::testsrc::TestSource::default()));
    }
    #[cfg(windows)]
    return Ok(Box::new(anydisplay::win::WinPlatform::new(o.monitor)?));
    #[cfg(not(windows))]
    unreachable!()
}

fn prompt() -> session::SasPrompt {
    Arc::new(|code: &str| {
        eprint!("Pairing code {code}. Does the device show the same code? [y/N] ");
        let _ = std::io::stderr().flush();
        let mut line = String::new();
        let _ = std::io::stdin().read_line(&mut line);
        matches!(line.trim().to_ascii_lowercase().as_str(), "y" | "yes")
    })
}

fn run_session(addr: std::net::SocketAddr, o: &Opts) -> anyhow::Result<bool> {
    let mut store = PairingStore::open(PairingStore::default_path())?;
    let mut platform = make_platform(o)?;
    let cfg = SessionConfig {
        host_name: host_name(),
        auto_confirm: o.yes,
        codec: o.codec,
        clipboard: o.clipboard,
        audio: o.audio,
        duration: o.seconds.map(Duration::from_secs_f64),
    };
    let handle = SessionHandle::default();
    let h = handle.clone();
    let presses = AtomicU32::new(0);
    ctrlc::set_handler(move || {
        if presses.fetch_add(1, Ordering::SeqCst) > 0 {
            std::process::exit(130);
        }
        h.stop(bye::SHUTDOWN);
    })?;
    let summary = session::run(addr, &mut store, platform.as_mut(), &cfg, prompt(), &handle)?;
    Ok(summary.error.is_none())
}

fn list(o: &Opts) -> anyhow::Result<()> {
    #[cfg(windows)]
    {
        println!("monitors:");
        for (i, m) in anydisplay::win::monitors::list()?.iter().enumerate() {
            println!("  {i}: {m}");
        }
    }
    match Adb::locate() {
        Some(adb) => {
            let devices = adb.devices()?;
            println!(
                "usb ({}):{}",
                adb.path.display(),
                if devices.is_empty() { " (none)" } else { "" }
            );
            for d in devices {
                println!("  {}  {}", d.serial, d.model.unwrap_or_default());
            }
        }
        None => println!("usb: adb not found (PATH, Android SDK, or ANYDISPLAY_ADB)"),
    }
    let seconds = o.seconds.unwrap_or(3.0);
    let found = anydisplay::discovery::browse(Duration::from_secs_f64(seconds))?;
    println!(
        "wifi (_anydisplay._tcp, {seconds} s):{}",
        if found.is_empty() { " (none)" } else { "" }
    );
    for d in found {
        let addrs: Vec<String> = d.addrs.iter().map(|a| a.to_string()).collect();
        println!(
            "  {}  id={}  {}",
            d.name,
            d.device_id.as_deref().unwrap_or("?"),
            addrs.join(" ")
        );
    }
    let store = PairingStore::open(PairingStore::default_path())?;
    println!(
        "paired ({}):{}",
        store.path().display(),
        if store.devices().is_empty() {
            " (none)"
        } else {
            ""
        }
    );
    for (id, d) in store.devices() {
        println!("  {id}  {}  paired {}", d.name, d.paired_at);
    }
    Ok(())
}

fn main() {
    #[cfg(windows)]
    anydisplay::win::init_process();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let command = args.first().cloned().unwrap_or_default();
    let result = parse(args.get(1..).unwrap_or_default()).and_then(|o| match command.as_str() {
        "list" => list(&o).map(|_| true),
        "connect" => {
            let target = o
                .positional
                .first()
                .ok_or_else(|| anyhow!("connect needs HOST[:PORT]\n\n{USAGE}"))?;
            run_session(session::resolve(target, DEFAULT_PORT)?, &o)
        }
        "usb" => {
            let adb = Adb::locate()
                .ok_or_else(|| anyhow!("adb not found (PATH, Android SDK, or set ANYDISPLAY_ADB)"))?;
            let serial = match o.positional.first() {
                Some(s) => s.clone(),
                None => {
                    let devices = adb.devices()?;
                    match devices.as_slice() {
                        [one] => one.serial.clone(),
                        [] => bail!("no USB device with debugging authorized"),
                        many => bail!(
                            "several devices, pick one: {}",
                            many.iter()
                                .map(|d| d.serial.as_str())
                                .collect::<Vec<_>>()
                                .join(" ")
                        ),
                    }
                }
            };
            let port = adb.forward(&serial, DEFAULT_PORT)?;
            info!("adb forward tcp:{port} -> {serial}:{DEFAULT_PORT}");
            let r = run_session(std::net::SocketAddr::from(([127, 0, 0, 1], port)), &o);
            adb.remove_forward(&serial, port);
            r
        }
        "" | "help" | "-h" | "--help" => {
            println!("{USAGE}");
            Ok(true)
        }
        other => bail!("unknown command {other}\n\n{USAGE}"),
    });
    match result {
        Ok(true) => {}
        Ok(false) => std::process::exit(1),
        Err(e) => {
            warn!("{e:#}");
            std::process::exit(2);
        }
    }
}
