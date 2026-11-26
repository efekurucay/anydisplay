#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

//! `usethatscreen-gui`: Windows system-tray and egui host for Use That Screen.

#[cfg(not(windows))]
fn main() {
    eprintln!(
        "usethatscreen-gui is only supported on Windows; use `usethatscreen-win` on other platforms."
    );
    std::process::exit(1);
}

#[cfg(windows)]
fn main() -> eframe::Result<()> {
    app::run()
}

#[cfg(windows)]
mod app {
    use usethatscreen::adb::{Adb, UsbDevice};
    use usethatscreen::discovery::{NetworkDevice, WifiMonitor};
    use usethatscreen::pairing::{PairedDevice, PairingStore, SavedConnection};
    use usethatscreen::proto::messages::{bye, Codec, DEFAULT_PORT};
    use usethatscreen::session::{self, SessionConfig, SessionHandle, SessionState, StreamInfo};
    use usethatscreen::win::monitors;
    use usethatscreen::{info, warn};
    use eframe::egui::{self, Color32, CornerRadius, Margin, RichText, Stroke};
    use std::collections::{BTreeMap, HashMap};
    use std::net::{IpAddr, SocketAddr, TcpListener, TcpStream};
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
    use std::sync::mpsc::{self, Receiver, Sender, SyncSender};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};
    use tray_icon::menu::{Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem};
    use tray_icon::{
        Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent,
    };

    const APP_NAME: &str = "Use That Screen";
    const SETUP_URL: &str = "https://usethatscreen.com/#setup";
    const VDD_URL: &str = "https://github.com/VirtualDrivers/Virtual-Display-Driver/releases";

    enum Endpoint {
        Dial(SocketAddr),
        Incoming(TcpStream),
    }

    /// Design tokens aligned with the Use That Screen website (warm cream / organic ink / burnt orange
    /// / soft lime in light mode, plus a matching dark variant).
    #[derive(Clone, Copy)]
    struct Palette {
        dark: bool,
        canvas: Color32,
        header_bg: Color32,
        card_bg: Color32,
        card_highlight: Color32,
        card_highlight_border: Color32,
        input_bg: Color32,
        hairline: Color32,
        ink: Color32,
        body: Color32,
        muted: Color32,
        primary: Color32,
        on_primary: Color32,
        accent: Color32,
        accent_hover: Color32,
        forest: Color32,
        sage: Color32,
        error_bg: Color32,
        error_border: Color32,
        error_text: Color32,
    }

    impl Palette {
        fn light() -> Self {
            Self {
                dark: false,
                canvas: Color32::from_rgb(0xF5, 0xF3, 0xEC),
                header_bg: Color32::from_rgb(0xEB, 0xEC, 0xE2),
                card_bg: Color32::from_rgb(0xEB, 0xEC, 0xE2),
                card_highlight: Color32::from_rgb(0xD9, 0xED, 0x9D),
                card_highlight_border: Color32::from_rgb(0xCA, 0xD6, 0xB6),
                input_bg: Color32::from_rgb(0xF5, 0xF3, 0xEC),
                hairline: Color32::from_rgb(0xD7, 0xD8, 0xCB),
                ink: Color32::from_rgb(0x24, 0x28, 0x20),
                body: Color32::from_rgb(0x55, 0x5F, 0x4C),
                muted: Color32::from_rgb(0x5D, 0x66, 0x56),
                primary: Color32::from_rgb(0x24, 0x28, 0x20),
                on_primary: Color32::WHITE,
                accent: Color32::from_rgb(0xC7, 0x4B, 0x29),
                accent_hover: Color32::from_rgb(0xAC, 0x3F, 0x22),
                forest: Color32::from_rgb(0x25, 0x3E, 0x34),
                sage: Color32::from_rgb(0x69, 0x7A, 0x67),
                error_bg: Color32::from_rgb(0xF8, 0xE6, 0xE0),
                error_border: Color32::from_rgb(0xC7, 0x4B, 0x29),
                error_text: Color32::from_rgb(0x8C, 0x2D, 0x14),
            }
        }

        fn dark() -> Self {
            Self {
                dark: true,
                canvas: Color32::from_rgb(0x0F, 0x0F, 0x0F),
                header_bg: Color32::from_rgb(0x16, 0x16, 0x16),
                card_bg: Color32::from_rgb(0x1D, 0x1D, 0x1D),
                card_highlight: Color32::from_rgb(0x25, 0x3E, 0x34),
                card_highlight_border: Color32::from_rgb(0x40, 0x63, 0x54),
                input_bg: Color32::from_rgb(0x14, 0x14, 0x14),
                hairline: Color32::from_rgb(0x35, 0x35, 0x35),
                ink: Color32::from_rgb(0xF6, 0xF6, 0xF6),
                body: Color32::from_rgb(0xC5, 0xC9, 0xBF),
                muted: Color32::from_rgb(0x9E, 0x9E, 0x9E),
                primary: Color32::from_rgb(0xF6, 0xF6, 0xF6),
                on_primary: Color32::from_rgb(0x0B, 0x0B, 0x0C),
                accent: Color32::from_rgb(0xC7, 0x4B, 0x29),
                accent_hover: Color32::from_rgb(0xAC, 0x3F, 0x22),
                forest: Color32::from_rgb(0xD9, 0xED, 0x9D),
                sage: Color32::from_rgb(0x76, 0x83, 0x69),
                error_bg: Color32::from_rgb(0x33, 0x1B, 0x15),
                error_border: Color32::from_rgb(0xC7, 0x4B, 0x29),
                error_text: Color32::from_rgb(0xF5, 0xB8, 0xA8),
            }
        }
    }

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

    struct UseThatScreenApp {
        ctx: egui::Context,
        tx: Sender<UiEvent>,
        rx: Receiver<UiEvent>,

        // Theme
        dark_mode: bool,

        // System tray
        tray: Option<TrayIcon>,
        tray_streaming: bool,
        hwnd: Arc<AtomicIsize>,

        // Discovery & monitors
        wifi_monitor: Option<WifiMonitor>,
        wifi_error: Option<String>,
        incoming: Option<(Arc<()>, Receiver<TcpStream>)>,
        usb_snapshot: Arc<Mutex<UsbSnapshot>>,
        usb_stop: Arc<AtomicBool>,
        monitors: Vec<MonitorItem>,

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
        saved_connections: Vec<SavedConnection>,
        editing_connection: Option<SavedConnection>,
        show_logs: bool,
        window_visible: bool,
        quitting: bool,
        last_auto_connect_check: Instant,
        last_monitor_refresh: Instant,
    }

    pub fn run() -> eframe::Result<()> {
        usethatscreen::win::init_process();
        let (rgba, w, h) = make_icon_rgba(false);
        let viewport_icon = egui::IconData {
            rgba,
            width: w,
            height: h,
        };
        let options = eframe::NativeOptions {
            viewport: egui::ViewportBuilder::default()
                .with_title(APP_NAME)
                .with_inner_size([480.0, 740.0])
                .with_min_inner_size([430.0, 540.0])
                .with_icon(viewport_icon),
            ..Default::default()
        };
        eframe::run_native(
            "UseThatScreen",
            options,
            Box::new(|cc| Ok(Box::new(UseThatScreenApp::new(cc)))),
        )
    }

    impl UseThatScreenApp {
        fn new(cc: &eframe::CreationContext<'_>) -> Self {
            install_fonts(&cc.egui_ctx);
            let dark_mode = match cc.egui_ctx.system_theme() {
                Some(egui::Theme::Dark) => true,
                Some(egui::Theme::Light) => false,
                None => cc.egui_ctx.style().visuals.dark_mode,
            };
            apply_theme(
                &cc.egui_ctx,
                if dark_mode {
                    Palette::dark()
                } else {
                    Palette::light()
                },
            );
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
            let store = PairingStore::open(PairingStore::default_path()).ok();
            let paired_devices = store
                .as_ref()
                .map(|store| store.devices().clone())
                .unwrap_or_default();
            let saved_connections = store
                .map(|store| store.connections().to_vec())
                .unwrap_or_default();

            info!("Use That Screen GUI started");

            Self {
                ctx: cc.egui_ctx.clone(),
                tx,
                rx,
                dark_mode,
                tray,
                tray_streaming: false,
                hwnd,
                wifi_monitor,
                wifi_error,
                usb_snapshot,
                usb_stop,
                monitors,
                sessions: BTreeMap::new(),
                usb_retry_after: HashMap::new(),
                pairing_prompt: None,
                paired_devices,
                incoming: None,
                last_error: None,
                auto_connect_usb: true,
                selected_monitor: None,
                preferred_codec: None,
                stream_audio: true,
                sync_clipboard: true,
                verbose_logs: false,
                manual_address: String::new(),
                saved_connections,
                editing_connection: None,
                show_logs: false,
                window_visible: true,
                quitting: false,
                last_auto_connect_check: Instant::now() - Duration::from_secs(5),
                last_monitor_refresh: Instant::now(),
            }
        }

        fn palette(&self) -> Palette {
            if self.dark_mode {
                Palette::dark()
            } else {
                Palette::light()
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
                    "Use That Screen — Streaming"
                } else {
                    APP_NAME
                };
                let _ = tray.set_tooltip(Some(tip));
            }
        }

        fn process_events(&mut self) {
            while let Ok(ev) = self.rx.try_recv() {
                match ev {
                    UiEvent::SessionState { key, state } => {
                        let mut new_connection = None;
                        if let Some(active) = self.sessions.get_mut(&key) {
                            if let SessionState::Streaming(ref info) = state {
                                if !matches!(active.state, SessionState::Streaming(_)) {
                                    new_connection = key
                                        .strip_prefix("ip:")
                                        .map(|address| (address, info.device_name.clone()));
                                }
                                if !info.device_name.is_empty() {
                                    active.title = info.device_name.clone();
                                }
                            }
                            active.state = state;
                        }
                        if let Some((address, name)) = new_connection {
                            self.update_saved_connections(|store| {
                                store.remember_connection(address, &name)
                            });
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
                .name(format!("usethatscreen-usb-{serial}"))
                .spawn(move || {
                    let outcome: anyhow::Result<Option<String>> = (|| {
                        let adb = Adb::locate().ok_or_else(|| {
                            anyhow::anyhow!("adb not found (install Android Platform-Tools)")
                        })?;
                        let port = adb.forward(&serial, DEFAULT_PORT)?;
                        info!("adb forward tcp:{port} -> {serial}:{DEFAULT_PORT}");
                        let addr = SocketAddr::from(([127, 0, 0, 1], port));
                        let res = run_gui_session(
                            Endpoint::Dial(addr),
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
            self.start_network_session(key, title, move || {
                Ok(Endpoint::Dial(session::resolve(&target, DEFAULT_PORT)?))
            });
        }

        fn update_saved_connections(
            &mut self,
            update: impl FnOnce(&mut PairingStore) -> std::io::Result<()>,
        ) {
            let result = PairingStore::open(PairingStore::default_path()).and_then(|mut store| {
                update(&mut store)?;
                Ok(store.connections().to_vec())
            });
            match result {
                Ok(connections) => self.saved_connections = connections,
                Err(err) => {
                    self.last_error = Some(format!("Could not update saved connections: {err}"))
                }
            }
        }

        fn start_network_session(
            &mut self,
            key: String,
            title: String,
            endpoint: impl FnOnce() -> anyhow::Result<Endpoint> + Send + 'static,
        ) {
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
                .name("usethatscreen-net".into())
                .spawn(move || {
                    let outcome: anyhow::Result<Option<String>> = (|| {
                        run_gui_session(
                            endpoint()?,
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

        fn listen_for_android(&mut self) -> std::io::Result<()> {
            let listener = TcpListener::bind(("0.0.0.0", DEFAULT_PORT))?;
            listener.set_nonblocking(true)?;
            let lifetime = Arc::new(());
            let worker = Arc::downgrade(&lifetime);
            let (tx, rx) = mpsc::sync_channel(1);
            let ctx = self.ctx.clone();
            std::thread::Builder::new()
                .name("usethatscreen-incoming".into())
                .spawn(move || {
                    while worker.strong_count() > 0 {
                        if let Ok((stream, SocketAddr::V4(peer))) = listener.accept() {
                            if session::is_android_peer(*peer.ip()) && tx.try_send(stream).is_ok() {
                                ctx.request_repaint();
                            }
                        }
                        std::thread::sleep(Duration::from_millis(100));
                    }
                })?;
            self.incoming = Some((lifetime, rx));
            Ok(())
        }

        fn accept_android_connection(&mut self) {
            let Some((_, rx)) = &self.incoming else {
                return;
            };
            if let Ok(stream) = rx.try_recv() {
                if !self.quitting && self.sessions.is_empty() {
                    self.start_network_session("incoming".into(), "Android".into(), || {
                        Ok(Endpoint::Incoming(stream))
                    });
                }
            }
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

    impl eframe::App for UseThatScreenApp {
        fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
            if self.hwnd.load(Ordering::Relaxed) == 0 {
                if let Some(h) = find_own_window(APP_NAME) {
                    self.hwnd.store(h, Ordering::Relaxed);
                }
            }
            self.process_events();
            self.accept_android_connection();
            self.maybe_auto_connect_usb();

            if ctx.input(|i| i.viewport().close_requested()) && !self.quitting {
                self.hide_to_tray();
                return;
            }

            if self.window_visible {
                if self.last_monitor_refresh.elapsed() >= Duration::from_secs(3) {
                    self.monitors = query_monitors();
                    self.last_monitor_refresh = Instant::now();
                }
                ctx.request_repaint_after(Duration::from_millis(500));
            }

            let p = self.palette();

            if self.pairing_prompt.is_some() {
                self.render_pairing_modal(ctx, p);
            } else {
                self.render_connection_editor(ctx);
            }

            // Top bar
            egui::TopBottomPanel::top("header")
                .frame(
                    egui::Frame::NONE
                        .fill(p.header_bg)
                        .stroke(Stroke::new(1.0, p.hairline))
                        .inner_margin(Margin::symmetric(16, 12)),
                )
                .show(ctx, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new(APP_NAME)
                                .family(display_font())
                                .size(19.5)
                                .color(p.ink),
                        );
                        ui.add_space(4.0);

                        if self.is_streaming() {
                            pill_filled(
                                ui,
                                "STREAMING",
                                p.card_highlight,
                                p.ink,
                                p.card_highlight_border,
                            );
                        } else if !self.sessions.is_empty() {
                            pill_filled(ui, "CONNECTING", p.accent, Color32::WHITE, p.accent);
                        } else {
                            pill_outline(ui, "IDLE", p);
                        }

                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let tip = if self.dark_mode {
                                "Switch to Light theme"
                            } else {
                                "Switch to Dark theme"
                            };
                            if theme_icon_button(ui, self.dark_mode, p)
                                .on_hover_text(tip)
                                .clicked()
                            {
                                self.dark_mode = !self.dark_mode;
                                apply_theme(ctx, self.palette());
                            }
                        });
                    });
                });

            // Footer
            egui::TopBottomPanel::bottom("footer")
                .frame(
                    egui::Frame::NONE
                        .fill(p.header_bg)
                        .stroke(Stroke::new(1.0, p.hairline))
                        .inner_margin(Margin::symmetric(16, 10)),
                )
                .show(ctx, |ui| {
                    ui.horizontal(|ui| {
                        let link = ui.add(
                            egui::Label::new(
                                RichText::new("Setup & Troubleshooting ↗")
                                    .size(12.5)
                                    .strong()
                                    .color(p.body),
                            )
                            .sense(egui::Sense::click()),
                        );
                        if link.hovered() {
                            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                        }
                        if link.clicked() {
                            open_url(SETUP_URL);
                        }

                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if primary_button_sm(ui, "Quit", p).clicked() {
                                self.begin_quit();
                            }
                            if outline_button_sm(ui, "Hide to Tray", p)
                                .on_hover_text("Keep Use That Screen running in the system tray")
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
                        .fill(p.canvas)
                        .inner_margin(Margin::symmetric(16, 14)),
                )
                .show(ctx, |ui| {
                    egui::ScrollArea::vertical()
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            if let Some(err) = self.last_error.clone() {
                                error_banner(ui, &err, p, || {
                                    self.last_error = None;
                                });
                                ui.add_space(12.0);
                            }

                            // 1. Connected sessions (signature lime highlight card)
                            if !self.sessions.is_empty() {
                                self.render_connected_section(ui, p);
                                ui.add_space(10.0);
                            }

                            // 2. USB devices
                            self.render_usb_section(ui, p);
                            ui.add_space(10.0);

                            // 3. Wi-Fi devices + manual IP
                            self.render_wifi_section(ui, p);
                            ui.add_space(10.0);

                            // 4. Display & Stream settings
                            self.render_settings_section(ui, p);
                            ui.add_space(10.0);

                            // 5. Logs
                            self.render_logs_section(ui, p);
                        });
                });
        }
    }

    impl UseThatScreenApp {
        fn render_pairing_modal(&mut self, ctx: &egui::Context, p: Palette) {
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
                .frame(
                    egui::Frame::NONE
                        .fill(p.canvas)
                        .stroke(Stroke::new(1.0, p.hairline))
                        .corner_radius(CornerRadius::same(6))
                        .inner_margin(Margin::same(18)),
                )
                .show(ctx, |ui| {
                    ui.vertical_centered(|ui| {
                        ui.label(
                            RichText::new(format!("Check that {title} shows this code:"))
                                .color(p.body),
                        );
                        ui.add_space(12.0);
                        egui::Frame::NONE
                            .fill(p.card_highlight)
                            .stroke(Stroke::new(1.0, p.card_highlight_border))
                            .corner_radius(CornerRadius::same(6))
                            .inner_margin(Margin::symmetric(20, 10))
                            .show(ui, |ui| {
                                ui.label(
                                    RichText::new(grouped)
                                        .monospace()
                                        .strong()
                                        .size(30.0)
                                        .color(p.ink),
                                );
                            });
                        ui.add_space(16.0);
                        ui.horizontal(|ui| {
                            if accent_button(ui, "Codes Match", p).clicked() {
                                decision = Some(true);
                            }
                            if outline_button_sm(ui, "Cancel", p).clicked() {
                                decision = Some(false);
                            }
                        });
                    });
                });

            if let Some(accepted) = decision {
                if let Some(pr) = self.pairing_prompt.take() {
                    let _ = pr.reply.send(accepted);
                }
            }
        }

        fn render_connected_section(&mut self, ui: &mut egui::Ui, p: Palette) {
            highlight_card(ui, "CONNECTED", p, |ui| {
                let mut to_disconnect: Vec<String> = Vec::new();
                for (key, active) in &self.sessions {
                    ui.horizontal(|ui| {
                        ui.vertical(|ui| {
                            ui.label(
                                RichText::new(&active.title)
                                    .family(display_font())
                                    .size(15.5)
                                    .color(p.ink),
                            );
                            ui.label(
                                RichText::new(describe_state(&active.state))
                                    .size(12.0)
                                    .color(if p.dark { p.body } else { p.forest }),
                            );
                        });
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if primary_button_sm(ui, "Disconnect", p).clicked() {
                                to_disconnect.push(key.clone());
                            }
                        });
                    });
                    ui.add_space(2.0);
                }
                for key in to_disconnect {
                    self.disconnect(&key);
                }
            });
        }

        fn render_usb_section(&mut self, ui: &mut egui::Ui, p: Palette) {
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

            section_card(ui, "USB · ADB", p, |ui| {
                if ui
                    .checkbox(
                        &mut self.auto_connect_usb,
                        RichText::new("Connect USB devices automatically").color(p.ink),
                    )
                    .changed()
                    && self.auto_connect_usb
                {
                    self.usb_retry_after.clear();
                    self.last_auto_connect_check = Instant::now() - Duration::from_secs(5);
                }
                ui.add_space(6.0);

                match snap {
                    (false, _, _, _) => {
                        ui.label(
                            RichText::new("Scanning USB devices (adb)…")
                                .size(12.0)
                                .color(p.muted),
                        );
                    }
                    (true, None, _, _) => {
                        ui.label(
                            RichText::new(
                                "adb not found. Install Android Platform-Tools or set USETHATSCREEN_ADB.",
                            )
                            .size(12.0)
                            .color(p.muted),
                        );
                    }
                    (true, Some(_), devices, err) if devices.is_empty() => {
                        let msg = err.unwrap_or_else(|| {
                            "No authorized USB devices. Unlock device and enable USB debugging."
                                .into()
                        });
                        ui.label(RichText::new(msg).size(12.0).color(p.muted));
                    }
                    (true, Some(_), devices, _) => {
                        let mut to_connect: Option<UsbDevice> = None;
                        for d in devices {
                            let key = format!("usb:{}", d.serial);
                            let connected = self.sessions.contains_key(&key);
                            let name = d.model.clone().unwrap_or_else(|| d.serial.clone());
                            ui.horizontal(|ui| {
                                ui.vertical(|ui| {
                                    ui.label(RichText::new(&name).strong().size(13.5).color(p.ink));
                                    if d.model.is_some() {
                                        ui.label(
                                            RichText::new(&d.serial)
                                                .monospace()
                                                .size(11.0)
                                                .color(p.muted),
                                        );
                                    }
                                });
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        if connected {
                                            pill_outline_colored(ui, "CONNECTED", p.forest, p);
                                        } else if accent_button(ui, "Connect", p).clicked() {
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

        fn render_wifi_section(&mut self, ui: &mut egui::Ui, p: Palette) {
            let wifi_devices: Vec<NetworkDevice> = self
                .wifi_monitor
                .as_ref()
                .map(|m| m.devices())
                .unwrap_or_default();

            let mut to_connect: Option<(String, String, String)> = None;
            let mut to_forget: Option<String> = None;

            let online_paired_ids: std::collections::HashSet<&str> = wifi_devices
                .iter()
                .filter_map(|d| d.device_id.as_deref())
                .collect();
            let offline_paired: Vec<(String, String)> = self
                .paired_devices
                .iter()
                .filter(|(id, _)| !online_paired_ids.contains(id.as_str()))
                .map(|(id, d)| (id.clone(), d.name.clone()))
                .collect();

            section_card(ui, "WI-FI", p, |ui| {
                let mut listening = self.incoming.is_some();
                if ui
                    .checkbox(&mut listening, "Allow paired Android devices to connect")
                    .changed()
                {
                    if !listening {
                        self.incoming = None;
                    } else if let Err(err) = self.listen_for_android() {
                        self.last_error =
                            Some(format!("Could not listen for Android connections: {err}"));
                    }
                }
                if self.incoming.is_some() {
                    ui.label(
                        RichText::new("On Android, enter this PC’s LAN or Tailscale IPv4 address.")
                            .size(12.0)
                            .color(p.muted),
                    );
                }
                // Keep manual entry above the discovered devices so mDNS updates never shift it.
                ui.horizontal(|ui| {
                    let input_w = (ui.available_width() - 126.0).max(120.0);
                    let resp = ui.add(
                        egui::TextEdit::singleline(&mut self.manual_address)
                            .hint_text("Android IP[:port]")
                            .margin(Margin::symmetric(8, 6))
                            .desired_width(input_w),
                    );
                    let enter = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    let has_input = !self.manual_address.trim().is_empty();
                    let clicked = ui
                        .add_enabled_ui(has_input, |ui| outline_button_sm(ui, "+ Connection", p))
                        .inner
                        .clicked();
                    if (enter || clicked) && has_input {
                        let input = self.manual_address.trim();
                        let target = input
                            .parse::<IpAddr>()
                            .map(|ip| SocketAddr::new(ip, DEFAULT_PORT).to_string())
                            .unwrap_or_else(|_| input.to_owned());
                        let key = format!("ip:{target}");
                        self.connect_network(key, target.clone(), target);
                    }
                });
                self.render_saved_connections(ui, p);

                ui.add_space(8.0);
                hairline_divider(ui, p);
                ui.add_space(8.0);

                if let Some(err) = &self.wifi_error {
                    ui.label(
                        RichText::new(format!("mDNS unavailable: {err}"))
                            .size(12.0)
                            .color(p.accent),
                    );
                } else if wifi_devices.is_empty() && offline_paired.is_empty() {
                    ui.label(
                        RichText::new(
                            "Searching for devices on the local network (_usethatscreen._tcp)…",
                        )
                        .size(12.0)
                        .color(p.muted),
                    );
                } else {
                    for d in &wifi_devices {
                        let key = format!("net:{}", d.name);
                        let connected = self.sessions.contains_key(&key);
                        let paired_id = d
                            .device_id
                            .as_deref()
                            .filter(|id| self.paired_devices.contains_key(*id))
                            .map(str::to_string);
                        let addr_str = d.addrs.first().map(|a| a.to_string()).unwrap_or_default();

                        ui.horizontal(|ui| {
                            ui.vertical(|ui| {
                                let title = if paired_id.is_some() {
                                    d.name.clone()
                                } else {
                                    format!("{} (not paired)", d.name)
                                };
                                ui.label(RichText::new(title).strong().size(13.5).color(p.ink));
                                if !addr_str.is_empty() {
                                    ui.label(
                                        RichText::new(&addr_str)
                                            .monospace()
                                            .size(11.0)
                                            .color(p.muted),
                                    );
                                }
                            });
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if connected {
                                        pill_outline_colored(ui, "CONNECTED", p.forest, p);
                                    } else if !addr_str.is_empty()
                                        && accent_button(ui, "Connect", p).clicked()
                                    {
                                        to_connect = Some((key, d.name.clone(), addr_str));
                                    }
                                    if let Some(id) = paired_id {
                                        if trash_icon_button(ui, p)
                                            .on_hover_text("Forget saved Wi-Fi pairing")
                                            .clicked()
                                        {
                                            to_forget = Some(id);
                                        }
                                    }
                                },
                            );
                        });
                        ui.add_space(4.0);
                    }

                    for (id, name) in &offline_paired {
                        ui.horizontal(|ui| {
                            ui.vertical(|ui| {
                                ui.label(RichText::new(name).size(13.0).color(p.body));
                                ui.label(
                                    RichText::new("Paired · Offline").size(11.0).color(p.muted),
                                );
                            });
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if trash_icon_button(ui, p)
                                        .on_hover_text("Forget saved Wi-Fi pairing")
                                        .clicked()
                                    {
                                        to_forget = Some(id.clone());
                                    }
                                },
                            );
                        });
                        ui.add_space(4.0);
                    }
                }
            });

            if let Some((key, name, addr)) = to_connect {
                self.connect_network(key, name, addr);
            }
            if let Some(id) = to_forget {
                if let Ok(mut store) = PairingStore::open(PairingStore::default_path()) {
                    let _ = store.forget_hex(&id);
                    self.paired_devices = store.devices().clone();
                }
            }
        }

        fn render_saved_connections(&mut self, ui: &mut egui::Ui, p: Palette) {
            if self.saved_connections.is_empty() {
                return;
            }
            let mut reconnect = None;
            egui::CollapsingHeader::new("Saved connections")
                .default_open(true)
                .show(ui, |ui| {
                    for entry in &self.saved_connections {
                        let key = format!("ip:{}", entry.address);
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if self.sessions.contains_key(&key) {
                                pill_outline_colored(ui, "CONNECTED", p.forest, p);
                            } else if accent_button(ui, "Connect", p).clicked() {
                                reconnect = Some((key, entry.clone()));
                            }
                            if outline_button_sm(ui, "Edit", p).clicked() {
                                self.editing_connection = Some(entry.clone());
                            }
                            ui.vertical(|ui| {
                                ui.label(
                                    RichText::new(&entry.name).strong().size(13.5).color(p.ink),
                                );
                                if entry.name != entry.address {
                                    ui.label(
                                        RichText::new(&entry.address)
                                            .monospace()
                                            .size(11.0)
                                            .color(p.muted),
                                    );
                                }
                            });
                        });
                    }
                });
            if let Some((key, entry)) = reconnect {
                self.connect_network(key, entry.name, entry.address);
            }
        }

        fn render_connection_editor(&mut self, ctx: &egui::Context) {
            let Some(entry) = self.editing_connection.as_mut() else {
                return;
            };
            let mut save = false;
            let mut remove = false;
            let mut cancel = false;
            let response =
                egui::Modal::new(egui::Id::new("edit_saved_connection")).show(ctx, |ui| {
                    ui.heading("Edit connection");
                    ui.label(&entry.address);
                    ui.text_edit_singleline(&mut entry.name);
                    ui.horizontal(|ui| {
                        save = ui.button("Save").clicked();
                        remove = ui.button("Remove").clicked();
                        cancel = ui.button("Cancel").clicked();
                    });
                });
            if save || remove {
                let address = entry.address.clone();
                let name = entry.name.trim().to_owned();
                let name = if name.is_empty() { &address } else { &name };
                self.update_saved_connections(|store| {
                    store.edit_connection(&address, save.then_some(name))
                });
            }
            if save || remove || cancel || response.should_close() {
                self.editing_connection = None;
            }
        }

        fn render_settings_section(&mut self, ui: &mut egui::Ui, p: Palette) {
            let has_vdd = self.has_vdd();
            section_card(ui, "DISPLAY & STREAM", p, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Monitor").size(12.5).color(p.body));
                    ui.add_space(4.0);
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
                        .width(235.0)
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

                    if refresh_icon_button(ui, p)
                        .on_hover_text("Refresh monitors")
                        .clicked()
                    {
                        self.monitors = query_monitors();
                        self.last_monitor_refresh = Instant::now();
                    }
                });

                ui.add_space(6.0);

                ui.horizontal(|ui| {
                    ui.label(RichText::new("Codec  ").size(12.5).color(p.body));
                    ui.add_space(4.0);
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

                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.checkbox(
                        &mut self.stream_audio,
                        RichText::new("Stream audio").color(p.ink),
                    );
                    ui.add_space(12.0);
                    ui.checkbox(
                        &mut self.sync_clipboard,
                        RichText::new("Sync clipboard").color(p.ink),
                    );
                });

                if !has_vdd {
                    ui.add_space(6.0);
                    let vdd_installed = std::path::Path::new(r"C:\VirtualDisplayDriver").is_dir();
                    ui.horizontal_wrapped(|ui| {
                        let (msg, link_text, target_url) = if vdd_installed {
                            (
                                "Virtual Display Driver is installed, but displays are set to duplicate or disabled.",
                                "Display Settings ↗",
                                "ms-settings:display",
                            )
                        } else {
                            (
                                "Virtual Display Driver not installed (mirroring only).",
                                "Install VDD ↗",
                                VDD_URL,
                            )
                        };
                        ui.label(RichText::new(msg).size(11.5).color(p.muted));
                        let link = ui.add(
                            egui::Label::new(
                                RichText::new(link_text)
                                    .size(11.5)
                                    .strong()
                                    .color(p.accent),
                            )
                            .sense(egui::Sense::click()),
                        );
                        if link.hovered() {
                            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                        }
                        if link.clicked() {
                            open_url(target_url);
                        }
                    });
                }
            });
        }

        fn render_logs_section(&mut self, ui: &mut egui::Ui, p: Palette) {
            egui::CollapsingHeader::new(RichText::new("Logs").size(12.5).strong().color(p.ink))
                .default_open(false)
                .open(if self.show_logs { Some(true) } else { None })
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        if ui
                            .checkbox(
                                &mut self.verbose_logs,
                                RichText::new("Verbose").color(p.ink),
                            )
                            .changed()
                        {
                            usethatscreen::log::set_verbose(self.verbose_logs);
                        }
                        if outline_button_sm(ui, "Clear", p).clicked() {
                            usethatscreen::log::clear_recent();
                        }
                    });
                    ui.add_space(4.0);
                    let lines = usethatscreen::log::recent();
                    egui::Frame::NONE
                        .fill(p.card_bg)
                        .stroke(Stroke::new(1.0, p.hairline))
                        .corner_radius(CornerRadius::same(6))
                        .inner_margin(Margin::same(8))
                        .show(ui, |ui| {
                            egui::ScrollArea::vertical()
                                .max_height(140.0)
                                .stick_to_bottom(true)
                                .show(ui, |ui| {
                                    ui.set_width(ui.available_width());
                                    for line in &lines {
                                        ui.label(
                                            RichText::new(line)
                                                .monospace()
                                                .size(11.0)
                                                .color(p.body),
                                        );
                                    }
                                });
                        });
                });
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn run_gui_session(
        endpoint: Endpoint,
        key: String,
        title: String,
        monitor_idx: Option<usize>,
        cfg: SessionConfig,
        handle: SessionHandle,
        tx: Sender<UiEvent>,
        ctx: egui::Context,
    ) -> anyhow::Result<Option<String>> {
        let mut store = PairingStore::open(PairingStore::default_path())?;
        let mut platform = usethatscreen::win::WinPlatform::new(monitor_idx)?;

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

        let summary = match endpoint {
            Endpoint::Dial(addr) => session::run_with_state(
                addr,
                &mut store,
                &mut platform,
                &cfg,
                prompt,
                &handle,
                Some(on_state),
            ),
            Endpoint::Incoming(stream) => {
                on_state(SessionState::Handshaking);
                session::run_incoming(
                    stream,
                    &mut store,
                    &mut platform,
                    &cfg,
                    &handle,
                    Some(on_state),
                )
            }
        }?;
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
                "{width}×{height} @ {refresh_hz} Hz · {}{}{}",
                codec.name().to_uppercase(),
                if *hardware_encoder { " (HW)" } else { " (SW)" },
                if *mirroring {
                    " · Mirroring"
                } else {
                    " · Extended"
                },
            ),
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
            .name("usethatscreen-usb-poll".into())
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
        let show_item = MenuItem::new("Show Use That Screen", true, None);
        let quit_item = MenuItem::new("Quit Use That Screen", true, None);
        let show_id = show_item.id().clone();
        let quit_id = quit_item.id().clone();
        let _ = menu.append(&show_item);
        let _ = menu.append(&PredefinedMenuItem::separator());
        let _ = menu.append(&quit_item);

        let (rgba, w, h) = make_icon_rgba(false);
        let tray = Icon::from_rgba(rgba, w, h).ok().and_then(|icon| {
            TrayIconBuilder::new()
                .with_menu(Box::new(menu))
                .with_tooltip(APP_NAME)
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
            .name("usethatscreen-tray-click".into())
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
            .name("usethatscreen-tray-menu".into())
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

    /// Returns the 32×32 app logo RGBA (with a green live dot when streaming).
    fn make_icon_rgba(streaming: bool) -> (Vec<u8>, u32, u32) {
        const W: u32 = 32;
        const H: u32 = 32;
        let mut buf = include_bytes!("../assets/icon-32.rgba").to_vec();
        if streaming {
            // Draw a crisp active indicator dot at the bottom-right corner.
            let (cx, cy) = (24.5_f32, 24.5_f32);
            for y in 18..32u32 {
                for x in 18..32u32 {
                    let dx = x as f32 + 0.5 - cx;
                    let dy = y as f32 + 0.5 - cy;
                    let d = (dx * dx + dy * dy).sqrt();
                    let color = if d <= 4.3 {
                        Some([0xD9, 0xED, 0x9D, 0xFF]) // signature lime
                    } else if d <= 6.0 {
                        Some([0x24, 0x28, 0x20, 0xFF]) // dark ink ring
                    } else {
                        None
                    };
                    if let Some(rgba) = color {
                        let i = ((y * W + x) * 4) as usize;
                        buf[i..i + 4].copy_from_slice(&rgba);
                    }
                }
            }
        }
        (buf, W, H)
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
            .unwrap_or_else(|| "Use That Screen host".into())
    }

    fn open_url(url: &str) {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let _ = std::process::Command::new("cmd")
            .args(["/C", "start", "", url])
            .creation_flags(CREATE_NO_WINDOW)
            .spawn();
    }

    fn display_font() -> egui::FontFamily {
        egui::FontFamily::Name("display".into())
    }

    fn install_fonts(ctx: &egui::Context) {
        let mut fonts = egui::FontDefinitions::default();
        fonts.font_data.insert(
            "dm_sans".into(),
            Arc::new(egui::FontData::from_static(include_bytes!(
                "../assets/fonts/DMSans-Regular.ttf"
            ))),
        );
        fonts.font_data.insert(
            "manrope_semibold".into(),
            Arc::new(egui::FontData::from_static(include_bytes!(
                "../assets/fonts/Manrope-SemiBold.ttf"
            ))),
        );
        fonts
            .families
            .entry(egui::FontFamily::Proportional)
            .or_default()
            .insert(0, "dm_sans".into());
        fonts.families.insert(
            display_font(),
            vec!["manrope_semibold".into(), "dm_sans".into()],
        );
        ctx.set_fonts(fonts);
    }

    fn apply_theme(ctx: &egui::Context, p: Palette) {
        let mut visuals = if p.dark {
            egui::Visuals::dark()
        } else {
            egui::Visuals::light()
        };

        visuals.panel_fill = p.canvas;
        visuals.window_fill = p.canvas;
        visuals.faint_bg_color = p.card_bg;
        visuals.extreme_bg_color = p.input_bg;
        visuals.override_text_color = Some(p.ink);
        visuals.window_stroke = Stroke::new(1.0, p.hairline);
        visuals.window_corner_radius = CornerRadius::same(6);
        visuals.window_shadow = egui::Shadow::NONE;
        visuals.popup_shadow = egui::Shadow::NONE;

        visuals.selection.bg_fill = p.accent;
        visuals.selection.stroke = Stroke::new(1.0, Color32::WHITE);

        visuals.widgets.noninteractive.bg_fill = p.card_bg;
        visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0, p.hairline);
        visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0, p.ink);
        visuals.widgets.noninteractive.corner_radius = CornerRadius::same(5);

        visuals.widgets.inactive.bg_fill = p.input_bg;
        visuals.widgets.inactive.weak_bg_fill = p.input_bg;
        visuals.widgets.inactive.bg_stroke = Stroke::new(1.0, p.hairline);
        visuals.widgets.inactive.fg_stroke = Stroke::new(1.0, p.ink);
        visuals.widgets.inactive.corner_radius = CornerRadius::same(5);

        visuals.widgets.hovered.bg_fill = p.card_bg;
        visuals.widgets.hovered.weak_bg_fill = p.card_bg;
        visuals.widgets.hovered.bg_stroke = Stroke::new(1.0, p.sage);
        visuals.widgets.hovered.fg_stroke = Stroke::new(1.0, p.ink);
        visuals.widgets.hovered.corner_radius = CornerRadius::same(5);

        visuals.widgets.active.bg_fill = p.accent_hover;
        visuals.widgets.active.weak_bg_fill = p.accent_hover;
        visuals.widgets.active.bg_stroke = Stroke::new(1.0, p.accent);
        visuals.widgets.active.fg_stroke = Stroke::new(1.0, Color32::WHITE);
        visuals.widgets.active.corner_radius = CornerRadius::same(5);

        visuals.widgets.open.bg_fill = p.card_bg;
        visuals.widgets.open.weak_bg_fill = p.card_bg;
        visuals.widgets.open.bg_stroke = Stroke::new(1.0, p.ink);
        visuals.widgets.open.fg_stroke = Stroke::new(1.0, p.ink);
        visuals.widgets.open.corner_radius = CornerRadius::same(5);

        ctx.set_visuals(visuals);
        ctx.style_mut(|s| {
            s.spacing.button_padding = egui::vec2(12.0, 5.0);
        });
    }

    fn section_card(ui: &mut egui::Ui, title: &str, p: Palette, body: impl FnOnce(&mut egui::Ui)) {
        let outer_w = ui.available_width();
        egui::Frame::NONE
            .fill(p.card_bg)
            .stroke(Stroke::new(1.0, p.hairline))
            .corner_radius(CornerRadius::same(6))
            .inner_margin(Margin::same(14))
            .show(ui, |ui| {
                let inner_w = (outer_w - 28.0).max(200.0);
                ui.set_min_width(inner_w);
                ui.set_max_width(inner_w);
                ui.label(
                    RichText::new(title)
                        .family(display_font())
                        .size(10.5)
                        .color(p.muted),
                );
                ui.add_space(6.0);
                body(ui);
            });
    }

    fn highlight_card(
        ui: &mut egui::Ui,
        title: &str,
        p: Palette,
        body: impl FnOnce(&mut egui::Ui),
    ) {
        let outer_w = ui.available_width();
        egui::Frame::NONE
            .fill(p.card_highlight)
            .stroke(Stroke::new(1.0, p.card_highlight_border))
            .corner_radius(CornerRadius::same(6))
            .inner_margin(Margin::same(14))
            .show(ui, |ui| {
                let inner_w = (outer_w - 28.0).max(200.0);
                ui.set_min_width(inner_w);
                ui.set_max_width(inner_w);
                ui.label(
                    RichText::new(title)
                        .family(display_font())
                        .size(10.5)
                        .color(p.forest),
                );
                ui.add_space(6.0);
                body(ui);
            });
    }

    /// Square outline icon button (`28×28`, matching standard button height).
    fn icon_button_shell(
        ui: &mut egui::Ui,
        p: Palette,
        draw: impl FnOnce(&egui::Painter, egui::Pos2, Color32, Color32),
    ) -> egui::Response {
        let size = egui::vec2(28.0, 28.0);
        let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::click());
        if resp.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        if ui.is_rect_visible(rect) {
            let bg = if resp.hovered() { p.card_bg } else { p.canvas };
            let border = if resp.hovered() { p.sage } else { p.hairline };
            let painter = ui.painter();
            painter.rect(
                rect,
                CornerRadius::same(5),
                bg,
                Stroke::new(1.0, border),
                egui::StrokeKind::Inside,
            );
            draw(painter, rect.center(), p.ink, bg);
        }
        resp
    }

    /// Vector Sun (when dark) / Moon (when light) theme toggle button.
    fn theme_icon_button(ui: &mut egui::Ui, dark_mode: bool, p: Palette) -> egui::Response {
        icon_button_shell(ui, p, |painter, c, fg, bg| {
            if dark_mode {
                painter.circle_stroke(c, 4.0, Stroke::new(1.5, fg));
                for i in 0..8 {
                    let angle = (i as f32) * std::f32::consts::FRAC_PI_4;
                    let (s, cs) = angle.sin_cos();
                    let p0 = egui::pos2(c.x + cs * 6.0, c.y + s * 6.0);
                    let p1 = egui::pos2(c.x + cs * 8.0, c.y + s * 8.0);
                    painter.line_segment([p0, p1], Stroke::new(1.4, fg));
                }
            } else {
                painter.circle_filled(c, 6.0, fg);
                painter.circle_filled(egui::pos2(c.x + 2.6, c.y - 2.2), 5.2, bg);
            }
        })
    }

    /// Lucide-style RefreshCw vector icon button.
    fn refresh_icon_button(ui: &mut egui::Ui, p: Palette) -> egui::Response {
        icon_button_shell(ui, p, |painter, c, fg, _| {
            let r = 5.5;
            let steps = 16;
            let start = -0.35_f32;
            let end = std::f32::consts::TAU - 1.15;
            let pts: Vec<egui::Pos2> = (0..=steps)
                .map(|i| {
                    let t = start + (end - start) * (i as f32 / steps as f32);
                    egui::pos2(c.x + r * t.cos(), c.y + r * t.sin())
                })
                .collect();
            painter.add(egui::Shape::line(pts, Stroke::new(1.5, fg)));
            // Arrowhead at top-right
            let tip = egui::pos2(c.x + r * start.cos(), c.y + r * start.sin());
            painter.line_segment(
                [tip, egui::pos2(tip.x - 3.5, tip.y - 0.5)],
                Stroke::new(1.5, fg),
            );
            painter.line_segment(
                [tip, egui::pos2(tip.x + 0.5, tip.y - 3.5)],
                Stroke::new(1.5, fg),
            );
        })
    }

    /// Lucide-style Trash2 vector icon button (for forgetting saved Wi-Fi pairings).
    fn trash_icon_button(ui: &mut egui::Ui, p: Palette) -> egui::Response {
        icon_button_shell(ui, p, |painter, c, fg, _| {
            let st = Stroke::new(1.4, fg);
            // Lid + handle
            painter.line_segment(
                [
                    egui::pos2(c.x - 5.5, c.y - 3.5),
                    egui::pos2(c.x + 5.5, c.y - 3.5),
                ],
                st,
            );
            painter.line_segment(
                [
                    egui::pos2(c.x - 2.0, c.y - 5.5),
                    egui::pos2(c.x + 2.0, c.y - 5.5),
                ],
                st,
            );
            // Bin body
            let body = egui::Rect::from_min_max(
                egui::pos2(c.x - 4.2, c.y - 3.5),
                egui::pos2(c.x + 4.2, c.y + 5.5),
            );
            painter.rect_stroke(body, CornerRadius::same(1), st, egui::StrokeKind::Middle);
            // Inner lines
            painter.line_segment(
                [
                    egui::pos2(c.x - 1.5, c.y - 1.0),
                    egui::pos2(c.x - 1.5, c.y + 3.2),
                ],
                st,
            );
            painter.line_segment(
                [
                    egui::pos2(c.x + 1.5, c.y - 1.0),
                    egui::pos2(c.x + 1.5, c.y + 3.2),
                ],
                st,
            );
        })
    }

    fn hairline_divider(ui: &mut egui::Ui, p: Palette) {
        let (rect, _) =
            ui.allocate_exact_size(egui::vec2(ui.available_width(), 1.0), egui::Sense::hover());
        ui.painter().rect_filled(rect, 0.0, p.hairline);
    }

    fn accent_button(ui: &mut egui::Ui, text: &str, p: Palette) -> egui::Response {
        ui.add(
            egui::Button::new(
                RichText::new(text)
                    .size(12.5)
                    .strong()
                    .color(Color32::WHITE),
            )
            .fill(p.accent)
            .stroke(Stroke::NONE)
            .corner_radius(CornerRadius::same(5))
            .min_size(egui::vec2(76.0, 28.0)),
        )
    }

    fn primary_button_sm(ui: &mut egui::Ui, text: &str, p: Palette) -> egui::Response {
        ui.add(
            egui::Button::new(RichText::new(text).size(12.5).strong().color(p.on_primary))
                .fill(p.primary)
                .stroke(Stroke::NONE)
                .corner_radius(CornerRadius::same(5))
                .min_size(egui::vec2(0.0, 28.0)),
        )
    }

    fn outline_button_sm(ui: &mut egui::Ui, text: &str, p: Palette) -> egui::Response {
        ui.add(
            egui::Button::new(RichText::new(text).size(12.5).color(p.ink))
                .fill(p.canvas)
                .stroke(Stroke::new(1.0, p.hairline))
                .corner_radius(CornerRadius::same(5))
                .min_size(egui::vec2(0.0, 28.0)),
        )
    }

    fn pill_outline(ui: &mut egui::Ui, text: &str, p: Palette) -> egui::Response {
        pill_outline_colored(ui, text, p.muted, p)
    }

    fn pill_outline_colored(
        ui: &mut egui::Ui,
        text: &str,
        text_color: Color32,
        p: Palette,
    ) -> egui::Response {
        egui::Frame::NONE
            .fill(p.canvas)
            .stroke(Stroke::new(1.0, p.hairline))
            .corner_radius(CornerRadius::same(4))
            .inner_margin(Margin::symmetric(8, 3))
            .show(ui, |ui| {
                ui.label(RichText::new(text).size(9.5).strong().color(text_color));
            })
            .response
    }

    fn pill_filled(
        ui: &mut egui::Ui,
        text: &str,
        bg: Color32,
        fg: Color32,
        border: Color32,
    ) -> egui::Response {
        egui::Frame::NONE
            .fill(bg)
            .stroke(Stroke::new(1.0, border))
            .corner_radius(CornerRadius::same(4))
            .inner_margin(Margin::symmetric(8, 3))
            .show(ui, |ui| {
                ui.label(RichText::new(text).size(9.5).strong().color(fg));
            })
            .response
    }

    fn error_banner(ui: &mut egui::Ui, msg: &str, p: Palette, on_dismiss: impl FnOnce()) {
        egui::Frame::NONE
            .fill(p.error_bg)
            .stroke(Stroke::new(1.0, p.error_border))
            .corner_radius(CornerRadius::same(6))
            .inner_margin(Margin::same(10))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(msg).size(12.0).color(p.error_text));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .add(
                                egui::Button::new(RichText::new("×").color(p.error_text))
                                    .frame(false),
                            )
                            .clicked()
                        {
                            on_dismiss();
                        }
                    });
                });
            });
    }
}
