#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

//! `anydisplay-gui`: Windows system-tray and egui host for AnyDisplay.

#[cfg(not(windows))]
fn main() {
    eprintln!(
        "anydisplay-gui is only supported on Windows; use `anydisplay-win` on other platforms."
    );
    std::process::exit(1);
}

#[cfg(windows)]
fn main() -> eframe::Result<()> {
    app::run()
}

#[cfg(windows)]
mod app {
    use anydisplay::adb::{Adb, UsbDevice};
    use anydisplay::discovery::{NetworkDevice, WifiMonitor};
    use anydisplay::pairing::{PairedDevice, PairingStore};
    use anydisplay::proto::messages::{bye, Codec, DEFAULT_PORT};
    use anydisplay::session::{self, SessionConfig, SessionHandle, SessionState, StreamInfo};
    use anydisplay::win::monitors;
    use anydisplay::{info, warn};
    use eframe::egui;
    use std::collections::{BTreeMap, HashMap};
    use std::net::SocketAddr;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
    use std::sync::mpsc::{self, Receiver, Sender, SyncSender};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};
    use tray_icon::menu::{Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem};
    use tray_icon::{
        Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent,
    };

    const SETUP_URL: &str = "https://efekurucay.com/anydisplay/#setup";

    #[derive(Clone)]
    struct MonitorItem {
        index: usize,
        gdi_name: String,
        friendly_name: String,
        width: i32,
        height: i32,
        primary: bool,
        virtual_display: bool,
    }

    impl MonitorItem {
        fn label(&self) -> String {
            let name = if self.friendly_name.is_empty() {
                self.gdi_name.as_str()
            } else {
                self.friendly_name.as_str()
            };
            let tag = if self.virtual_display {
                " [VDD]"
            } else if self.primary {
                " [Primary]"
            } else {
                ""
            };
            format!(
                "#{}: {} ({}×{}){}",
                self.index, name, self.width, self.height, tag
            )
        }
    }

    struct ActiveSession {
        title: String,
        state: SessionState,
        handle: SessionHandle,
    }

    struct PairingPrompt {
        key: String,
        title: String,
        code: String,
        reply: SyncSender<bool>,
    }

    enum UiEvent {
        SessionState { key: String, state: SessionState },
        SessionEnded { key: String, error: Option<String> },
        PairingPrompt(PairingPrompt),
        ShowWindow,
        QuitRequested,
    }

    #[derive(Default)]
    struct UsbSnapshot {
        scanned: bool,
        adb_path: Option<PathBuf>,
        devices: Vec<UsbDevice>,
        error: Option<String>,
    }

    struct AnyDisplayApp {
        ctx: egui::Context,
        tx: Sender<UiEvent>,
        rx: Receiver<UiEvent>,

        // System tray
        tray: Option<TrayIcon>,
        tray_streaming: bool,
        hwnd: Arc<AtomicIsize>,

        // Discovery & monitors
        wifi_monitor: Option<WifiMonitor>,
        wifi_error: Option<String>,
        usb_snapshot: Arc<Mutex<UsbSnapshot>>,
        usb_stop: Arc<AtomicBool>,
        monitors: Vec<MonitorItem>,
        is_admin: bool,

        // Sessions & pairing
        sessions: BTreeMap<String, ActiveSession>,
        usb_retry_after: HashMap<String, Instant>,
        pairing_prompt: Option<PairingPrompt>,
        paired_devices: BTreeMap<String, PairedDevice>,
        last_error: Option<String>,

        // Settings
        auto_connect_usb: bool,
        selected_monitor: Option<usize>,
        preferred_codec: Option<Codec>,
        stream_audio: bool,
        sync_clipboard: bool,
        verbose_logs: bool,

        // UI inputs & state
        manual_address: String,
        show_logs: bool,
        show_paired: bool,
        window_visible: bool,
        quitting: bool,
        last_auto_connect_check: Instant,
        last_monitor_refresh: Instant,
    }

    pub fn run() -> eframe::Result<()> {
        anydisplay::win::init_process();
        let (rgba, w, h) = make_icon_rgba(false);
        let viewport_icon = egui::IconData {
            rgba,
            width: w,
            height: h,
        };
        let options = eframe::NativeOptions {
            viewport: egui::ViewportBuilder::default()
                .with_title("AnyDisplay")
                .with_inner_size([430.0, 640.0])
                .with_min_inner_size([380.0, 480.0])
                .with_icon(viewport_icon),
            ..Default::default()
        };
        eframe::run_native(
            "AnyDisplay",
            options,
            Box::new(|cc| Ok(Box::new(AnyDisplayApp::new(cc)))),
        )
    }

    impl AnyDisplayApp {
        fn new(cc: &eframe::CreationContext<'_>) -> Self {
            configure_theme(&cc.egui_ctx);
            let (tx, rx) = mpsc::channel();

            // Start Wi-Fi mDNS monitor
            let (wifi_monitor, wifi_error) = match WifiMonitor::start() {
                Ok(m) => (Some(m), None),
                Err(e) => {
                    warn!("mDNS discovery unavailable: {e:#}");
                    (None, Some(format!("{e:#}")))
                }
            };

            // Start USB adb poller
            let usb_snapshot = Arc::new(Mutex::new(UsbSnapshot::default()));
            let usb_stop = Arc::new(AtomicBool::new(false));
            spawn_usb_poller(usb_snapshot.clone(), usb_stop.clone(), cc.egui_ctx.clone());

            // Create system tray icon on UI thread
            let hwnd = Arc::new(AtomicIsize::new(0));
            let (tray, show_id, quit_id) = build_tray_icon();
            spawn_tray_listener(
                tx.clone(),
                cc.egui_ctx.clone(),
                hwnd.clone(),
                show_id,
                quit_id,
            );

            let monitors = query_monitors();
            let paired_devices = load_paired_devices();
            let is_admin = is_elevated();

            info!("AnyDisplay GUI started (admin={is_admin})");

            Self {
                ctx: cc.egui_ctx.clone(),
                tx,
                rx,
                tray,
                tray_streaming: false,
                hwnd,
                wifi_monitor,
                wifi_error,
                usb_snapshot,
                usb_stop,
                monitors,
                is_admin,
                sessions: BTreeMap::new(),
                usb_retry_after: HashMap::new(),
                pairing_prompt: None,
                paired_devices,
                last_error: None,
                auto_connect_usb: true,
                selected_monitor: None,
                preferred_codec: None,
                stream_audio: true,
                sync_clipboard: true,
                verbose_logs: false,
                manual_address: String::new(),
                show_logs: false,
                show_paired: false,
                window_visible: true,
                quitting: false,
                last_auto_connect_check: Instant::now() - Duration::from_secs(5),
                last_monitor_refresh: Instant::now(),
            }
        }

        fn has_vdd(&self) -> bool {
            self.monitors.iter().any(|m| m.virtual_display)
        }

        fn is_streaming(&self) -> bool {
            self.sessions
                .values()
                .any(|s| matches!(s.state, SessionState::Streaming(_)))
        }

        fn refresh_tray_icon(&mut self) {
            let streaming = self.is_streaming();
            if streaming == self.tray_streaming {
                return;
            }
            self.tray_streaming = streaming;
            if let Some(tray) = &self.tray {
                let (rgba, w, h) = make_icon_rgba(streaming);
                if let Ok(icon) = Icon::from_rgba(rgba, w, h) {
                    let _ = tray.set_icon(Some(icon));
                }
                let tip = if streaming {
                    "AnyDisplay — Streaming"
                } else {
                    "AnyDisplay"
                };
                let _ = tray.set_tooltip(Some(tip));
            }
        }

        fn process_events(&mut self) {
            while let Ok(ev) = self.rx.try_recv() {
                match ev {
                    UiEvent::SessionState { key, state } => {
                        if let Some(active) = self.sessions.get_mut(&key) {
                            if let SessionState::Streaming(ref info) = state {
                                if !info.device_name.is_empty() {
                                    active.title = info.device_name.clone();
                                }
                            }
                            active.state = state;
                        }
                    }
                    UiEvent::SessionEnded { key, error } => {
                        self.sessions.remove(&key);
                        if self.pairing_prompt.as_ref().is_some_and(|p| p.key == key) {
                            self.pairing_prompt = None;
                        }
                        if let Some(serial) = key.strip_prefix("usb:") {
                            let backoff = if error.is_some() { 5 } else { 15 };
                            self.usb_retry_after.insert(
                                serial.to_string(),
                                Instant::now() + Duration::from_secs(backoff),
                            );
                        }
                        if let Some(err) = error {
                            self.last_error = Some(err);
                        }
                        self.paired_devices = load_paired_devices();
                    }
                    UiEvent::PairingPrompt(prompt) => {
                        self.pairing_prompt = Some(prompt);
                        self.show_window();
                    }
                    UiEvent::ShowWindow => {
                        self.show_window();
                    }
                    UiEvent::QuitRequested => {
                        self.begin_quit();
                    }
                }
            }
            self.refresh_tray_icon();
        }

        fn show_window(&mut self) {
            self.window_visible = true;
            let raw = self.hwnd.load(Ordering::Relaxed);
            if raw != 0 {
                show_hwnd(raw);
            } else {
                self.ctx
                    .send_viewport_cmd(egui::ViewportCommand::Visible(true));
                self.ctx
                    .send_viewport_cmd(egui::ViewportCommand::Minimized(false));
                self.ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
            }
            self.ctx.request_repaint();
        }

        fn hide_to_tray(&mut self) {
            self.window_visible = false;
            self.ctx
                .send_viewport_cmd(egui::ViewportCommand::CancelClose);
            let raw = self.hwnd.load(Ordering::Relaxed);
            if raw != 0 {
                hide_hwnd(raw);
            } else {
                self.ctx
                    .send_viewport_cmd(egui::ViewportCommand::Visible(false));
            }
        }

        fn begin_quit(&mut self) {
            if self.quitting {
                return;
            }
            self.quitting = true;
            self.usb_stop.store(true, Ordering::Relaxed);
            if let Some(prompt) = self.pairing_prompt.take() {
                let _ = prompt.reply.send(false);
            }
            for s in self.sessions.values() {
                s.handle.stop(bye::SHUTDOWN);
            }
            if !self.sessions.is_empty() {
                std::thread::sleep(Duration::from_millis(300));
            }
            self.ctx
                .send_viewport_cmd(egui::ViewportCommand::Visible(true));
            self.ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }

        fn maybe_auto_connect_usb(&mut self) {
            if !self.auto_connect_usb || self.quitting {
                return;
            }
            let now = Instant::now();
            if now.duration_since(self.last_auto_connect_check) < Duration::from_secs(2) {
                return;
            }
            self.last_auto_connect_check = now;

            let devices = self
                .usb_snapshot
                .lock()
                .map(|s| s.devices.clone())
                .unwrap_or_default();

            for d in devices {
                let key = format!("usb:{}", d.serial);
                if self.sessions.contains_key(&key) {
                    continue;
                }
                if self
                    .usb_retry_after
                    .get(&d.serial)
                    .is_some_and(|until| *until > now)
                {
                    continue;
                }
                self.connect_usb(d);
            }
        }

        fn session_config(&self) -> SessionConfig {
            SessionConfig {
                host_name: host_name(),
                auto_confirm: false,
                codec: self.preferred_codec,
                clipboard: self.sync_clipboard,
                audio: self.stream_audio,
                duration: None,
            }
        }

        fn connect_usb(&mut self, device: UsbDevice) {
            let key = format!("usb:{}", device.serial);
            if self.sessions.contains_key(&key) {
                return;
            }
            self.usb_retry_after.remove(&device.serial);
            self.last_error = None;

            let title = device
                .model
                .clone()
                .unwrap_or_else(|| device.serial.clone());
            let handle = SessionHandle::default();
            self.sessions.insert(
                key.clone(),
                ActiveSession {
                    title: title.clone(),
                    state: SessionState::Connecting,
                    handle: handle.clone(),
                },
            );

            let serial = device.serial.clone();
            let cfg = self.session_config();
            let monitor_idx = self.selected_monitor;
            let tx = self.tx.clone();
            let ctx = self.ctx.clone();

            std::thread::Builder::new()
                .name(format!("anydisplay-usb-{serial}"))
                .spawn(move || {
                    let outcome: anyhow::Result<Option<String>> = (|| {
                        let adb = Adb::locate().ok_or_else(|| {
                            anyhow::anyhow!("adb not found (install Android Platform-Tools)")
                        })?;
                        let port = adb.forward(&serial, DEFAULT_PORT)?;
                        info!("adb forward tcp:{port} -> {serial}:{DEFAULT_PORT}");
                        let addr = SocketAddr::from(([127, 0, 0, 1], port));
                        let res = run_gui_session(
                            addr,
                            key.clone(),
                            title,
                            monitor_idx,
                            cfg,
                            handle,
                            tx.clone(),
                            ctx.clone(),
                        );
                        adb.remove_forward(&serial, port);
                        res
                    })();

                    let error = match outcome {
                        Ok(err) => err,
                        Err(e) => Some(format!("{e:#}")),
                    };
                    let _ = tx.send(UiEvent::SessionEnded { key, error });
                    ctx.request_repaint();
                })
                .ok();
        }

        fn connect_network(&mut self, key: String, title: String, target: String) {
            if self.sessions.contains_key(&key) {
                return;
            }
            self.last_error = None;

            let handle = SessionHandle::default();
            self.sessions.insert(
                key.clone(),
                ActiveSession {
                    title: title.clone(),
                    state: SessionState::Connecting,
                    handle: handle.clone(),
                },
            );

            let cfg = self.session_config();
            let monitor_idx = self.selected_monitor;
            let tx = self.tx.clone();
            let ctx = self.ctx.clone();

            std::thread::Builder::new()
                .name("anydisplay-net".into())
                .spawn(move || {
                    let outcome: anyhow::Result<Option<String>> = (|| {
                        let addr = session::resolve(&target, DEFAULT_PORT)?;
                        run_gui_session(
                            addr,
                            key.clone(),
                            title,
                            monitor_idx,
                            cfg,
                            handle,
                            tx.clone(),
                            ctx.clone(),
                        )
                    })();
                    let error = match outcome {
                        Ok(err) => err,
                        Err(e) => Some(format!("{e:#}")),
                    };
                    let _ = tx.send(UiEvent::SessionEnded { key, error });
                    ctx.request_repaint();
                })
                .ok();
        }

        fn disconnect(&mut self, key: &str) {
            if let Some(active) = self.sessions.get(key) {
                active.handle.stop(bye::USER);
            }
            if let Some(serial) = key.strip_prefix("usb:") {
                self.usb_retry_after
                    .insert(serial.to_string(), Instant::now() + Duration::from_secs(15));
            }
        }
    }

    impl eframe::App for AnyDisplayApp {
        fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
            if self.hwnd.load(Ordering::Relaxed) == 0 {
                if let Some(h) = find_own_window("AnyDisplay") {
                    self.hwnd.store(h, Ordering::Relaxed);
                }
            }
            self.process_events();
            self.maybe_auto_connect_usb();

            if ctx.input(|i| i.viewport().close_requested()) && !self.quitting {
                self.hide_to_tray();
                return;
            }

            // Periodic repaint and monitor refresh while visible.
            if self.window_visible {
                if self.last_monitor_refresh.elapsed() >= Duration::from_secs(3) {
                    self.monitors = query_monitors();
                    self.last_monitor_refresh = Instant::now();
                }
                ctx.request_repaint_after(Duration::from_millis(500));
            }

            // Pairing confirmation modal
            self.render_pairing_modal(ctx);

            // Top bar
            egui::TopBottomPanel::top("header")
                .frame(
                    egui::Frame::NONE
                        .fill(egui::Color32::from_rgb(20, 22, 27))
                        .inner_margin(egui::Margin::symmetric(14, 10)),
                )
                .show(ctx, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new("AnyDisplay")
                                .strong()
                                .size(17.0)
                                .color(egui::Color32::WHITE),
                        );

                        let (pill_text, pill_color) = if self.is_streaming() {
                            ("Streaming", egui::Color32::from_rgb(34, 197, 94))
                        } else if !self.sessions.is_empty() {
                            ("Connecting", egui::Color32::from_rgb(234, 179, 8))
                        } else {
                            ("Idle", egui::Color32::from_rgb(100, 116, 139))
                        };
                        status_badge(ui, pill_text, pill_color);

                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if self.has_vdd() {
                                status_badge(ui, "VDD Ready", egui::Color32::from_rgb(56, 189, 248))
                                    .on_hover_text("Virtual Display Driver detected — extended display enabled.");
                            } else {
                                status_badge(ui, "Mirror Mode", egui::Color32::from_rgb(148, 163, 184))
                                    .on_hover_text("Virtual Display Driver not detected. Install VDD for an extended display.");
                            }
                            if !self.is_admin {
                                status_badge(ui, "Standard", egui::Color32::from_rgb(120, 113, 108))
                                    .on_hover_text("Not running as Administrator. Touch/pen input cannot control elevated (UAC) windows.");
                            }
                        });
                    });
                });

            // Footer
            egui::TopBottomPanel::bottom("footer")
                .frame(
                    egui::Frame::NONE
                        .fill(egui::Color32::from_rgb(20, 22, 27))
                        .inner_margin(egui::Margin::symmetric(14, 8)),
                )
                .show(ctx, |ui| {
                    ui.horizontal(|ui| {
                        if ui.link("Setup & Troubleshooting").clicked() {
                            open_url(SETUP_URL);
                        }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.button("Quit").clicked() {
                                self.begin_quit();
                            }
                            if ui
                                .button("Hide to Tray")
                                .on_hover_text("Keep AnyDisplay running in the system tray")
                                .clicked()
                            {
                                self.hide_to_tray();
                            }
                        });
                    });
                });

            // Main scrollable body
            egui::CentralPanel::default()
                .frame(
                    egui::Frame::NONE
                        .fill(egui::Color32::from_rgb(26, 29, 36))
                        .inner_margin(egui::Margin::symmetric(14, 10)),
                )
                .show(ctx, |ui| {
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        if let Some(err) = self.last_error.clone() {
                            error_banner(ui, &err, || {
                                self.last_error = None;
                            });
                            ui.add_space(8.0);
                        }

                        // 1. Connected sessions
                        if !self.sessions.is_empty() {
                            self.render_connected_section(ui);
                            ui.add_space(10.0);
                        }

                        // 2. USB devices
                        self.render_usb_section(ui);
                        ui.add_space(10.0);

                        // 3. Wi-Fi devices + manual IP
                        self.render_wifi_section(ui);
                        ui.add_space(10.0);

                        // 4. Display & Stream settings
                        self.render_settings_section(ui);
                        ui.add_space(10.0);

                        // 5. Paired devices
                        self.render_paired_section(ui);
                        ui.add_space(6.0);

                        // 6. Logs
                        self.render_logs_section(ui);
                    });
                });
        }
    }

    impl AnyDisplayApp {
        fn render_pairing_modal(&mut self, ctx: &egui::Context) {
            let Some(prompt) = &self.pairing_prompt else {
                return;
            };
            let title = prompt.title.clone();
            let code = prompt.code.clone();
            let grouped = if code.len() == 6 {
                format!("{} {}", &code[..3], &code[3..])
            } else {
                code
            };

            let mut decision: Option<bool> = None;
            egui::Window::new(format!("Pair with {title}?"))
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    ui.vertical_centered(|ui| {
                        ui.add_space(6.0);
                        ui.label(format!("Check that {title} shows this code:"));
                        ui.add_space(10.0);
                        ui.label(
                            egui::RichText::new(grouped)
                                .monospace()
                                .strong()
                                .size(30.0)
                                .color(egui::Color32::from_rgb(96, 165, 250)),
                        );
                        ui.add_space(14.0);
                        ui.horizontal(|ui| {
                            if ui
                                .add(
                                    egui::Button::new(egui::RichText::new("Codes Match").strong())
                                        .fill(egui::Color32::from_rgb(37, 99, 235)),
                                )
                                .clicked()
                            {
                                decision = Some(true);
                            }
                            if ui.button("Cancel").clicked() {
                                decision = Some(false);
                            }
                        });
                        ui.add_space(4.0);
                    });
                });

            if let Some(accepted) = decision {
                if let Some(p) = self.pairing_prompt.take() {
                    let _ = p.reply.send(accepted);
                }
            }
        }

        fn render_connected_section(&mut self, ui: &mut egui::Ui) {
            section_card(ui, "Connected", |ui| {
                let mut to_disconnect: Vec<String> = Vec::new();
                for (key, active) in &self.sessions {
                    ui.horizontal(|ui| {
                        ui.vertical(|ui| {
                            ui.label(egui::RichText::new(&active.title).strong());
                            ui.label(
                                egui::RichText::new(describe_state(&active.state))
                                    .small()
                                    .color(egui::Color32::from_rgb(148, 163, 184)),
                            );
                        });
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.button("Disconnect").clicked() {
                                to_disconnect.push(key.clone());
                            }
                        });
                    });
                    ui.add_space(4.0);
                }
                for key in to_disconnect {
                    self.disconnect(&key);
                }
            });
        }

        fn render_usb_section(&mut self, ui: &mut egui::Ui) {
            let snap = self
                .usb_snapshot
                .lock()
                .map(|s| {
                    (
                        s.scanned,
                        s.adb_path.clone(),
                        s.devices.clone(),
                        s.error.clone(),
                    )
                })
                .unwrap_or((false, None, Vec::new(), None));

            section_card(ui, "USB (ADB)", |ui| {
                if ui
                    .checkbox(
                        &mut self.auto_connect_usb,
                        "Connect USB devices automatically",
                    )
                    .changed()
                    && self.auto_connect_usb
                {
                    self.usb_retry_after.clear();
                    self.last_auto_connect_check = Instant::now() - Duration::from_secs(5);
                }
                ui.add_space(4.0);

                match snap {
                    (false, _, _, _) => {
                        ui.label(
                            egui::RichText::new("Scanning USB devices (adb)…")
                                .small()
                                .color(egui::Color32::from_rgb(148, 163, 184)),
                        );
                    }
                    (true, None, _, _) => {
                        ui.label(
                            egui::RichText::new(
                                "adb not found. Install Android Platform-Tools or set ANYDISPLAY_ADB.",
                            )
                            .small()
                            .color(egui::Color32::from_rgb(148, 163, 184)),
                        );
                    }
                    (true, Some(_), devices, err) if devices.is_empty() => {
                        let msg = err.unwrap_or_else(|| {
                            "No authorized USB devices. Unlock device and enable USB debugging."
                                .into()
                        });
                        ui.label(
                            egui::RichText::new(msg)
                                .small()
                                .color(egui::Color32::from_rgb(148, 163, 184)),
                        );
                    }
                    (true, Some(_), devices, _) => {
                        let mut to_connect: Option<UsbDevice> = None;
                        for d in devices {
                            let key = format!("usb:{}", d.serial);
                            let connected = self.sessions.contains_key(&key);
                            let name = d.model.clone().unwrap_or_else(|| d.serial.clone());
                            ui.horizontal(|ui| {
                                ui.vertical(|ui| {
                                    ui.label(egui::RichText::new(&name).strong());
                                    if d.model.is_some() {
                                        ui.label(
                                            egui::RichText::new(&d.serial)
                                                .small()
                                                .color(egui::Color32::from_rgb(148, 163, 184)),
                                        );
                                    }
                                });
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        if ui
                                            .add_enabled(
                                                !connected,
                                                egui::Button::new(if connected {
                                                    "Connected"
                                                } else {
                                                    "Connect"
                                                }),
                                            )
                                            .clicked()
                                        {
                                            to_connect = Some(d.clone());
                                        }
                                    },
                                );
                            });
                        }
                        if let Some(dev) = to_connect {
                            self.connect_usb(dev);
                        }
                    }
                }
            });
        }

        fn render_wifi_section(&mut self, ui: &mut egui::Ui) {
            let wifi_devices: Vec<NetworkDevice> = self
                .wifi_monitor
                .as_ref()
                .map(|m| m.devices())
                .unwrap_or_default();

            section_card(ui, "Wi-Fi", |ui| {
                if let Some(err) = &self.wifi_error {
                    ui.label(
                        egui::RichText::new(format!("mDNS unavailable: {err}"))
                            .small()
                            .color(egui::Color32::from_rgb(248, 113, 113)),
                    );
                } else if wifi_devices.is_empty() {
                    ui.label(
                        egui::RichText::new(
                            "Searching for devices on the local network (_anydisplay._tcp)…",
                        )
                        .small()
                        .color(egui::Color32::from_rgb(148, 163, 184)),
                    );
                } else {
                    let mut to_connect: Option<(String, String, String)> = None;
                    for d in wifi_devices {
                        let key = format!("net:{}", d.name);
                        let connected = self.sessions.contains_key(&key);
                        let paired = d
                            .device_id
                            .as_deref()
                            .is_some_and(|id| self.paired_devices.contains_key(id));
                        let addr_str = d.addrs.first().map(|a| a.to_string()).unwrap_or_default();

                        ui.horizontal(|ui| {
                            ui.vertical(|ui| {
                                let title = if paired {
                                    d.name.clone()
                                } else {
                                    format!("{} (not paired)", d.name)
                                };
                                ui.label(egui::RichText::new(title).strong());
                                if !addr_str.is_empty() {
                                    ui.label(
                                        egui::RichText::new(&addr_str)
                                            .small()
                                            .color(egui::Color32::from_rgb(148, 163, 184)),
                                    );
                                }
                            });
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    let can_connect = !connected && !addr_str.is_empty();
                                    if ui
                                        .add_enabled(
                                            can_connect,
                                            egui::Button::new(if connected {
                                                "Connected"
                                            } else {
                                                "Connect"
                                            }),
                                        )
                                        .clicked()
                                    {
                                        to_connect = Some((key, d.name.clone(), addr_str));
                                    }
                                },
                            );
                        });
                    }
                    if let Some((key, name, addr)) = to_connect {
                        self.connect_network(key, name, addr);
                    }
                }

                ui.add_space(6.0);
                ui.separator();
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    let resp = ui.add(
                        egui::TextEdit::singleline(&mut self.manual_address)
                            .hint_text("Tablet IP, e.g. 192.168.1.20")
                            .desired_width(ui.available_width() - 80.0),
                    );
                    let enter = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    let clicked = ui
                        .add_enabled(
                            !self.manual_address.trim().is_empty(),
                            egui::Button::new("Connect"),
                        )
                        .clicked();
                    if (enter || clicked) && !self.manual_address.trim().is_empty() {
                        let target = self.manual_address.trim().to_string();
                        let key = format!("ip:{target}");
                        self.connect_network(key, target.clone(), target);
                    }
                });
            });
        }

        fn render_settings_section(&mut self, ui: &mut egui::Ui) {
            let has_vdd = self.has_vdd();
            section_card(ui, "Display & Stream", |ui| {
                // Monitor selector
                ui.horizontal(|ui| {
                    ui.label("Monitor:");
                    let current_label = match self.selected_monitor {
                        None => {
                            if has_vdd {
                                "Auto (Virtual Display)".to_string()
                            } else {
                                "Auto (Primary Monitor)".to_string()
                            }
                        }
                        Some(idx) => self
                            .monitors
                            .iter()
                            .find(|m| m.index == idx)
                            .map(|m| m.label())
                            .unwrap_or_else(|| format!("#{idx}")),
                    };

                    egui::ComboBox::from_id_salt("monitor_select")
                        .selected_text(current_label)
                        .width(230.0)
                        .show_ui(ui, |ui| {
                            ui.selectable_value(
                                &mut self.selected_monitor,
                                None,
                                if has_vdd {
                                    "Auto (Virtual Display)"
                                } else {
                                    "Auto (Primary Monitor)"
                                },
                            );
                            for m in &self.monitors {
                                ui.selectable_value(
                                    &mut self.selected_monitor,
                                    Some(m.index),
                                    m.label(),
                                );
                            }
                        });

                    if ui.button("↻").on_hover_text("Refresh monitors").clicked() {
                        self.monitors = query_monitors();
                    }
                });

                ui.add_space(4.0);

                // Codec selector
                ui.horizontal(|ui| {
                    ui.label("Codec:  ");
                    let codec_label = match self.preferred_codec {
                        None => "Auto",
                        Some(Codec::Hevc) => "HEVC (H.265)",
                        Some(Codec::H264) => "H.264",
                        Some(Codec::Av1) => "AV1",
                    };
                    egui::ComboBox::from_id_salt("codec_select")
                        .selected_text(codec_label)
                        .width(150.0)
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut self.preferred_codec, None, "Auto");
                            ui.selectable_value(
                                &mut self.preferred_codec,
                                Some(Codec::Hevc),
                                "HEVC (H.265)",
                            );
                            ui.selectable_value(
                                &mut self.preferred_codec,
                                Some(Codec::H264),
                                "H.264",
                            );
                        });
                });

                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.checkbox(&mut self.stream_audio, "Stream audio");
                    ui.checkbox(&mut self.sync_clipboard, "Sync clipboard");
                });
            });
        }

        fn render_paired_section(&mut self, ui: &mut egui::Ui) {
            let count = self.paired_devices.len();
            egui::CollapsingHeader::new(format!("Paired Devices ({count})"))
                .default_open(false)
                .open(if self.show_paired { Some(true) } else { None })
                .show(ui, |ui| {
                    if self.paired_devices.is_empty() {
                        ui.label(
                            egui::RichText::new("No paired Wi-Fi devices yet.")
                                .small()
                                .color(egui::Color32::from_rgb(148, 163, 184)),
                        );
                        return;
                    }
                    let mut to_forget: Option<String> = None;
                    for (id, d) in &self.paired_devices {
                        ui.horizontal(|ui| {
                            ui.label(&d.name);
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if ui.small_button("Forget").clicked() {
                                        to_forget = Some(id.clone());
                                    }
                                },
                            );
                        });
                    }
                    if let Some(id) = to_forget {
                        if let Ok(mut store) = PairingStore::open(PairingStore::default_path()) {
                            let _ = store.forget_hex(&id);
                            self.paired_devices = store.devices().clone();
                        }
                    }
                    ui.add_space(4.0);
                    if ui.small_button("Forget All").clicked() {
                        if let Ok(mut store) = PairingStore::open(PairingStore::default_path()) {
                            let _ = store.forget_all();
                            self.paired_devices.clear();
                        }
                    }
                });
        }

        fn render_logs_section(&mut self, ui: &mut egui::Ui) {
            egui::CollapsingHeader::new("Logs")
                .default_open(false)
                .open(if self.show_logs { Some(true) } else { None })
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        if ui.checkbox(&mut self.verbose_logs, "Verbose").changed() {
                            anydisplay::log::set_verbose(self.verbose_logs);
                        }
                        if ui.small_button("Clear").clicked() {
                            anydisplay::log::clear_recent();
                        }
                    });
                    let lines = anydisplay::log::recent();
                    egui::ScrollArea::vertical()
                        .max_height(140.0)
                        .stick_to_bottom(true)
                        .show(ui, |ui| {
                            for line in &lines {
                                ui.label(
                                    egui::RichText::new(line)
                                        .monospace()
                                        .size(11.0)
                                        .color(egui::Color32::from_rgb(203, 213, 225)),
                                );
                            }
                        });
                });
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn run_gui_session(
        addr: SocketAddr,
        key: String,
        title: String,
        monitor_idx: Option<usize>,
        cfg: SessionConfig,
        handle: SessionHandle,
        tx: Sender<UiEvent>,
        ctx: egui::Context,
    ) -> anyhow::Result<Option<String>> {
        let mut store = PairingStore::open(PairingStore::default_path())?;
        let mut platform = anydisplay::win::WinPlatform::new(monitor_idx)?;

        let prompt_tx = tx.clone();
        let prompt_ctx = ctx.clone();
        let prompt_key = key.clone();
        let prompt_title = title.clone();
        let prompt: session::SasPrompt = Arc::new(move |code: &str| {
            let (reply_tx, reply_rx) = mpsc::sync_channel(1);
            let _ = prompt_tx.send(UiEvent::PairingPrompt(PairingPrompt {
                key: prompt_key.clone(),
                title: prompt_title.clone(),
                code: code.to_string(),
                reply: reply_tx,
            }));
            prompt_ctx.request_repaint();
            reply_rx.recv().unwrap_or(false)
        });

        let state_tx = tx.clone();
        let state_ctx = ctx.clone();
        let state_key = key;
        let on_state: session::StateCallback = Arc::new(move |state: SessionState| {
            let _ = state_tx.send(UiEvent::SessionState {
                key: state_key.clone(),
                state,
            });
            state_ctx.request_repaint();
        });

        let summary = session::run_with_state(
            addr,
            &mut store,
            &mut platform,
            &cfg,
            prompt,
            &handle,
            Some(on_state),
        )?;
        Ok(summary.error)
    }

    fn describe_state(state: &SessionState) -> String {
        match state {
            SessionState::Connecting => "Connecting…".into(),
            SessionState::Handshaking => "Authenticating…".into(),
            SessionState::ConfirmingCode(code) => format!("Confirm pairing code {code}"),
            SessionState::Streaming(StreamInfo {
                width,
                height,
                refresh_hz,
                codec,
                mirroring,
                hardware_encoder,
                ..
            }) => format!(
                "{width}×{height} @ {refresh_hz} Hz · {}{}{} ",
                codec.name().to_uppercase(),
                if *hardware_encoder { " (HW)" } else { " (SW)" },
                if *mirroring {
                    " · Mirroring"
                } else {
                    " · Extended"
                },
            )
            .trim_end()
            .to_string(),
            SessionState::Closed(None) => "Disconnected".into(),
            SessionState::Closed(Some(e)) => format!("Error: {e}"),
        }
    }

    fn spawn_usb_poller(
        snapshot: Arc<Mutex<UsbSnapshot>>,
        stop: Arc<AtomicBool>,
        ctx: egui::Context,
    ) {
        std::thread::Builder::new()
            .name("anydisplay-usb-poll".into())
            .spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    let next = match Adb::locate() {
                        Some(adb) => match adb.devices() {
                            Ok(devices) => UsbSnapshot {
                                scanned: true,
                                adb_path: Some(adb.path),
                                devices,
                                error: None,
                            },
                            Err(e) => UsbSnapshot {
                                scanned: true,
                                adb_path: Some(adb.path),
                                devices: Vec::new(),
                                error: Some(e.to_string()),
                            },
                        },
                        None => UsbSnapshot {
                            scanned: true,
                            ..UsbSnapshot::default()
                        },
                    };
                    if let Ok(mut guard) = snapshot.lock() {
                        let changed = guard.scanned != next.scanned
                            || guard.devices != next.devices
                            || guard.adb_path != next.adb_path
                            || guard.error != next.error;
                        *guard = next;
                        if changed {
                            ctx.request_repaint();
                        }
                    }
                    for _ in 0..20 {
                        if stop.load(Ordering::Relaxed) {
                            return;
                        }
                        std::thread::sleep(Duration::from_millis(100));
                    }
                }
            })
            .ok();
    }

    fn build_tray_icon() -> (Option<TrayIcon>, MenuId, MenuId) {
        let menu = Menu::new();
        let show_item = MenuItem::new("Show AnyDisplay", true, None);
        let quit_item = MenuItem::new("Quit AnyDisplay", true, None);
        let show_id = show_item.id().clone();
        let quit_id = quit_item.id().clone();
        let _ = menu.append(&show_item);
        let _ = menu.append(&PredefinedMenuItem::separator());
        let _ = menu.append(&quit_item);

        let (rgba, w, h) = make_icon_rgba(false);
        let tray = Icon::from_rgba(rgba, w, h).ok().and_then(|icon| {
            TrayIconBuilder::new()
                .with_menu(Box::new(menu))
                .with_tooltip("AnyDisplay")
                .with_icon(icon)
                .build()
                .ok()
        });
        (tray, show_id, quit_id)
    }

    fn spawn_tray_listener(
        tx: Sender<UiEvent>,
        ctx: egui::Context,
        hwnd: Arc<AtomicIsize>,
        show_id: MenuId,
        quit_id: MenuId,
    ) {
        let tx_click = tx.clone();
        let ctx_click = ctx.clone();
        let hwnd_click = hwnd.clone();
        std::thread::Builder::new()
            .name("anydisplay-tray-click".into())
            .spawn(move || {
                let rx = TrayIconEvent::receiver();
                while let Ok(ev) = rx.recv() {
                    if matches!(
                        ev,
                        TrayIconEvent::Click {
                            button: MouseButton::Left,
                            button_state: MouseButtonState::Up,
                            ..
                        } | TrayIconEvent::DoubleClick {
                            button: MouseButton::Left,
                            ..
                        }
                    ) {
                        let _ = tx_click.send(UiEvent::ShowWindow);
                        let raw = hwnd_click.load(Ordering::Relaxed);
                        if raw != 0 {
                            show_hwnd(raw);
                        } else {
                            ctx_click.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                            ctx_click.send_viewport_cmd(egui::ViewportCommand::Focus);
                        }
                        ctx_click.request_repaint();
                    }
                }
            })
            .ok();

        std::thread::Builder::new()
            .name("anydisplay-tray-menu".into())
            .spawn(move || {
                let rx = MenuEvent::receiver();
                while let Ok(ev) = rx.recv() {
                    if ev.id == show_id {
                        let _ = tx.send(UiEvent::ShowWindow);
                        let raw = hwnd.load(Ordering::Relaxed);
                        if raw != 0 {
                            show_hwnd(raw);
                        } else {
                            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                        }
                        ctx.request_repaint();
                    } else if ev.id == quit_id {
                        let _ = tx.send(UiEvent::QuitRequested);
                        let raw = hwnd.load(Ordering::Relaxed);
                        if raw != 0 {
                            show_hwnd(raw);
                        } else {
                            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                        }
                        ctx.request_repaint();
                    }
                }
            })
            .ok();
    }

    fn find_own_window(title: &str) -> Option<isize> {
        use windows::core::BOOL;
        use windows::Win32::Foundation::{HWND, LPARAM};
        use windows::Win32::System::Threading::GetCurrentProcessId;
        use windows::Win32::UI::WindowsAndMessaging::{
            EnumWindows, GetWindowTextW, GetWindowThreadProcessId,
        };

        struct Search<'a> {
            pid: u32,
            title: &'a str,
            found: isize,
        }

        unsafe extern "system" fn callback(hwnd: HWND, lparam: LPARAM) -> BOOL {
            let search = &mut *(lparam.0 as *mut Search<'_>);
            let mut pid = 0u32;
            GetWindowThreadProcessId(hwnd, Some(&mut pid));
            if pid != search.pid {
                return BOOL(1);
            }
            let mut buf = [0u16; 256];
            let len = GetWindowTextW(hwnd, &mut buf);
            if len > 0 && String::from_utf16_lossy(&buf[..len as usize]) == search.title {
                search.found = hwnd.0 as isize;
                return BOOL(0);
            }
            BOOL(1)
        }

        let mut search = Search {
            pid: unsafe { GetCurrentProcessId() },
            title,
            found: 0,
        };
        unsafe {
            let _ = EnumWindows(Some(callback), LPARAM(&mut search as *mut _ as isize));
        }
        (search.found != 0).then_some(search.found)
    }

    fn show_hwnd(raw: isize) {
        use windows::Win32::Foundation::HWND;
        use windows::Win32::UI::WindowsAndMessaging::{
            SetForegroundWindow, ShowWindow, SW_RESTORE, SW_SHOW,
        };
        unsafe {
            let h = HWND(raw as *mut _);
            let _ = ShowWindow(h, SW_SHOW);
            let _ = ShowWindow(h, SW_RESTORE);
            let _ = SetForegroundWindow(h);
        }
    }

    fn hide_hwnd(raw: isize) {
        use windows::Win32::Foundation::HWND;
        use windows::Win32::UI::WindowsAndMessaging::{ShowWindow, SW_HIDE};
        unsafe {
            let _ = ShowWindow(HWND(raw as *mut _), SW_HIDE);
        }
    }

    /// Generates a crisp 32×32 overlapping-displays RGBA icon (accented green when streaming).
    fn make_icon_rgba(streaming: bool) -> (Vec<u8>, u32, u32) {
        let (w, h) = (32u32, 32u32);
        let mut buf = vec![0u8; (w * h * 4) as usize];

        let mut set_px = |x: u32, y: u32, rgba: [u8; 4]| {
            if x < w && y < h {
                let i = ((y * w + x) * 4) as usize;
                buf[i..i + 4].copy_from_slice(&rgba);
            }
        };

        let fill_rect = |set_px: &mut dyn FnMut(u32, u32, [u8; 4]),
                         x0: u32,
                         y0: u32,
                         x1: u32,
                         y1: u32,
                         color: [u8; 4]| {
            for y in y0..y1 {
                for x in x0..x1 {
                    set_px(x, y, color);
                }
            }
        };

        let stroke_rect = |set_px: &mut dyn FnMut(u32, u32, [u8; 4]),
                           x0: u32,
                           y0: u32,
                           x1: u32,
                           y1: u32,
                           t: u32,
                           color: [u8; 4]| {
            for y in y0..y1 {
                for x in x0..x1 {
                    if x < x0 + t || x >= x1 - t || y < y0 + t || y >= y1 - t {
                        set_px(x, y, color);
                    }
                }
            }
        };

        let back_border = [148, 163, 184, 255];
        let back_fill = [30, 41, 59, 255];
        let front_border = if streaming {
            [52, 211, 153, 255]
        } else {
            [241, 245, 249, 255]
        };
        let front_fill = if streaming {
            [16, 185, 129, 255]
        } else {
            [59, 130, 246, 255]
        };

        // Back display
        fill_rect(&mut set_px, 3, 4, 22, 19, back_fill);
        stroke_rect(&mut set_px, 3, 4, 22, 19, 2, back_border);

        // Front display (overlapping bottom-right)
        fill_rect(&mut set_px, 10, 12, 29, 27, front_fill);
        stroke_rect(&mut set_px, 10, 12, 29, 27, 2, front_border);

        (buf, w, h)
    }

    fn query_monitors() -> Vec<MonitorItem> {
        monitors::list()
            .unwrap_or_default()
            .into_iter()
            .enumerate()
            .map(|(index, m)| MonitorItem {
                index,
                gdi_name: m.gdi_name,
                friendly_name: m.friendly_name,
                width: m.rect.w,
                height: m.rect.h,
                primary: m.primary,
                virtual_display: m.virtual_display,
            })
            .collect()
    }

    fn load_paired_devices() -> BTreeMap<String, PairedDevice> {
        PairingStore::open(PairingStore::default_path())
            .map(|s| s.devices().clone())
            .unwrap_or_default()
    }

    fn host_name() -> String {
        std::env::var("COMPUTERNAME")
            .ok()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "AnyDisplay host".into())
    }

    fn is_elevated() -> bool {
        unsafe { windows::Win32::UI::Shell::IsUserAnAdmin().as_bool() }
    }

    fn open_url(url: &str) {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let _ = std::process::Command::new("cmd")
            .args(["/C", "start", "", url])
            .creation_flags(CREATE_NO_WINDOW)
            .spawn();
    }

    fn configure_theme(ctx: &egui::Context) {
        let mut visuals = egui::Visuals::dark();
        visuals.window_corner_radius = egui::CornerRadius::same(8);
        visuals.widgets.inactive.corner_radius = egui::CornerRadius::same(6);
        visuals.widgets.hovered.corner_radius = egui::CornerRadius::same(6);
        visuals.widgets.active.corner_radius = egui::CornerRadius::same(6);
        ctx.set_visuals(visuals);
    }

    fn section_card(ui: &mut egui::Ui, title: &str, body: impl FnOnce(&mut egui::Ui)) {
        egui::Frame::NONE
            .fill(egui::Color32::from_rgb(34, 38, 47))
            .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(51, 57, 70)))
            .corner_radius(egui::CornerRadius::same(8))
            .inner_margin(egui::Margin::same(10))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(
                    egui::RichText::new(title)
                        .strong()
                        .size(13.0)
                        .color(egui::Color32::from_rgb(226, 232, 240)),
                );
                ui.add_space(6.0);
                body(ui);
            });
    }

    fn status_badge(ui: &mut egui::Ui, text: &str, color: egui::Color32) -> egui::Response {
        egui::Frame::NONE
            .fill(color.linear_multiply(0.18))
            .stroke(egui::Stroke::new(1.0, color.linear_multiply(0.65)))
            .corner_radius(egui::CornerRadius::same(10))
            .inner_margin(egui::Margin::symmetric(8, 2))
            .show(ui, |ui| {
                ui.label(egui::RichText::new(text).small().strong().color(color));
            })
            .response
    }

    fn error_banner(ui: &mut egui::Ui, msg: &str, on_dismiss: impl FnOnce()) {
        egui::Frame::NONE
            .fill(egui::Color32::from_rgb(69, 26, 26))
            .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(239, 68, 68)))
            .corner_radius(egui::CornerRadius::same(6))
            .inner_margin(egui::Margin::same(8))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new(msg)
                            .small()
                            .color(egui::Color32::from_rgb(254, 202, 202)),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.small_button("×").clicked() {
                            on_dismiss();
                        }
                    });
                });
            });
    }
}
