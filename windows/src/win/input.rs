//! Input injection: pen and multitouch through synthetic pointer devices, keyboard, text, mouse
//! and wheel through SendInput. Translation logic lives in `crate::input`.

use super::monitors;
use crate::input::*;
use crate::keymap::{self, WinKey};
use crate::proto::messages::{mouse_action, InputEvent, Key, Mouse, Scroll};
use std::time::{Duration, Instant};
use windows::Win32::Foundation::{POINT, RECT};
use windows::Win32::UI::Controls::*;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::Input::Pointer::*;
use windows::Win32::UI::WindowsAndMessaging::*;

/// Stationary touch contacts are re-sent this often so Windows keeps them alive.
const TOUCH_REFRESH: Duration = Duration::from_millis(100);

pub struct Injector {
    pen: Option<HSYNTHETICPOINTERDEVICE>,
    touch: Option<HSYNTHETICPOINTERDEVICE>,
    target: Option<InputTarget>,
    pen_state: PenTracker,
    touch_slots: TouchSlots,
    modifiers: ModifierSync,
    scroll: ScrollAccumulator,
    last_touch: Instant,
    mouse_buttons: Vec<u8>,
    /// Set the first time Windows refuses an injection (UIPI: elevated window in focus).
    pub blocked: Option<String>,
}

// Synthetic pointer device handles are plain handles usable from any thread.
unsafe impl Send for Injector {}

impl Injector {
    pub fn new() -> Injector {
        let create = |t, n| unsafe { CreateSyntheticPointerDevice(t, n, POINTER_FEEDBACK_DEFAULT) };
        let pen = create(PT_PEN, 1)
            .map_err(|e| crate::warn!("synthetic pen unavailable: {e}"))
            .ok();
        let touch = create(PT_TOUCH, MAX_TOUCH_SLOTS as u32)
            .map_err(|e| crate::warn!("synthetic touch unavailable: {e}"))
            .ok();
        Injector {
            pen,
            touch,
            target: None,
            pen_state: PenTracker::default(),
            touch_slots: TouchSlots::default(),
            modifiers: ModifierSync::default(),
            scroll: ScrollAccumulator::default(),
            last_touch: Instant::now(),
            mouse_buttons: Vec::new(),
            blocked: None,
        }
    }

    pub fn has_pen(&self) -> bool {
        self.pen.is_some()
    }

    pub fn has_touch(&self) -> bool {
        self.touch.is_some()
    }

    pub fn set_target(&mut self, target: Option<InputTarget>) {
        self.target = target;
    }

    pub fn handle(&mut self, event: InputEvent) {
        let Some(target) = self.target else { return };
        match event {
            InputEvent::Touch(t) => {
                let frame = self.touch_slots.frame(&t.contacts, &target);
                self.inject_touch(&frame);
            }
            InputEvent::Pen(p) => {
                for s in &p.samples {
                    let points = self.pen_state.sample(s, &target);
                    for pt in points {
                        self.inject_pen(&pt);
                    }
                }
            }
            InputEvent::Key(k) => self.key(&k),
            InputEvent::Text(t) => self.text(&t.text),
            InputEvent::Scroll(s) => self.scroll(&s, &target),
            InputEvent::Mouse(m) => self.mouse(&m, &target),
        }
    }

    pub fn tick(&mut self) {
        if self.touch_slots.active() && self.last_touch.elapsed() >= TOUCH_REFRESH {
            let frame = self.touch_slots.repeat();
            self.inject_touch(&frame);
        }
    }

    /// Lifts contacts, the pen, keys and buttons.
    pub fn reset(&mut self) {
        let frame = self.touch_slots.cancel_all();
        self.inject_touch(&frame);
        for p in self.pen_state.cancel() {
            self.inject_pen(&p);
        }
        for k in self.modifiers.release_all() {
            self.send_key(k);
        }
        for b in std::mem::take(&mut self.mouse_buttons) {
            self.send(&[mouse_input(0, 0, button_flags(b, false), 0)]);
        }
        self.scroll.reset();
    }

    fn failed(&mut self, what: &str, e: impl std::fmt::Display) {
        if self.blocked.is_none() {
            crate::warn!("{what} injection failed: {e}");
            self.blocked = Some(format!("{what} injection failed: {e}"));
        } else {
            crate::debug!("{what} injection failed: {e}");
        }
    }

    fn inject_pen(&mut self, p: &PenPoint) {
        let Some(dev) = self.pen else { return };
        let mut info = POINTER_TYPE_INFO {
            r#type: PT_PEN,
            ..Default::default()
        };
        info.Anonymous.penInfo = POINTER_PEN_INFO {
            pointerInfo: POINTER_INFO {
                pointerType: PT_PEN,
                pointerFlags: POINTER_FLAGS(p.flags),
                ptPixelLocation: POINT { x: p.x, y: p.y },
                ..Default::default()
            },
            penFlags: p.pen_flags,
            penMask: PEN_MASK_PRESSURE | PEN_MASK_ROTATION | PEN_MASK_TILT_X | PEN_MASK_TILT_Y,
            pressure: p.pressure,
            rotation: p.rotation,
            tiltX: p.tilt_x,
            tiltY: p.tilt_y,
        };
        if let Err(e) = unsafe { InjectSyntheticPointerInput(dev, &[info]) } {
            self.failed("pen", e);
        }
    }

    fn inject_touch(&mut self, frame: &[TouchPoint]) {
        let Some(dev) = self.touch else { return };
        if frame.is_empty() {
            return;
        }
        let infos: Vec<POINTER_TYPE_INFO> = frame
            .iter()
            .map(|t| {
                let mut info = POINTER_TYPE_INFO {
                    r#type: PT_TOUCH,
                    ..Default::default()
                };
                let rc = RECT {
                    left: t.x - t.radius,
                    top: t.y - t.radius,
                    right: t.x + t.radius,
                    bottom: t.y + t.radius,
                };
                info.Anonymous.touchInfo = POINTER_TOUCH_INFO {
                    pointerInfo: POINTER_INFO {
                        pointerType: PT_TOUCH,
                        pointerId: t.slot,
                        pointerFlags: POINTER_FLAGS(t.flags),
                        ptPixelLocation: POINT { x: t.x, y: t.y },
                        ..Default::default()
                    },
                    touchFlags: 0,
                    touchMask: TOUCH_MASK_CONTACTAREA | TOUCH_MASK_PRESSURE,
                    rcContact: rc,
                    rcContactRaw: rc,
                    orientation: 0,
                    pressure: t.pressure,
                };
                info
            })
            .collect();
        self.last_touch = Instant::now();
        if let Err(e) = unsafe { InjectSyntheticPointerInput(dev, &infos) } {
            self.failed("touch", e);
        }
    }

    fn send(&mut self, inputs: &[INPUT]) {
        if inputs.is_empty() {
            return;
        }
        let n = unsafe { SendInput(inputs, size_of::<INPUT>() as i32) };
        if n as usize != inputs.len() {
            self.failed("keyboard/mouse", windows::core::Error::from_thread());
        }
    }

    fn send_key(&mut self, k: KeyStroke) {
        let Some(key) = keymap::win_key(k.hid_usage) else {
            crate::debug!("unmapped HID usage 0x{:02x}", k.hid_usage);
            return;
        };
        let up = if k.down {
            KEYBD_EVENT_FLAGS(0)
        } else {
            KEYEVENTF_KEYUP
        };
        let ki = match key {
            WinKey::Scan { code, extended } => KEYBDINPUT {
                wVk: VIRTUAL_KEY(0),
                wScan: code,
                dwFlags: KEYEVENTF_SCANCODE
                    | up
                    | if extended {
                        KEYEVENTF_EXTENDEDKEY
                    } else {
                        KEYBD_EVENT_FLAGS(0)
                    },
                time: 0,
                dwExtraInfo: 0,
            },
            WinKey::Vk(vk) => KEYBDINPUT {
                wVk: VIRTUAL_KEY(vk),
                wScan: 0,
                // Num Lock is an extended key when sent by virtual-key code.
                dwFlags: up
                    | if vk == VK_NUMLOCK.0 {
                        KEYEVENTF_EXTENDEDKEY
                    } else {
                        KEYBD_EVENT_FLAGS(0)
                    },
                time: 0,
                dwExtraInfo: 0,
            },
        };
        self.send(&[INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 { ki },
        }]);
    }

    fn key(&mut self, k: &Key) {
        for s in self.modifiers.key(k.action == 1, k.hid_usage, k.modifiers) {
            self.send_key(s);
        }
    }

    fn text(&mut self, text: &str) {
        let inputs: Vec<INPUT> = utf16_units(text)
            .into_iter()
            .flat_map(|u| {
                [KEYEVENTF_UNICODE, KEYEVENTF_UNICODE | KEYEVENTF_KEYUP].map(|flags| INPUT {
                    r#type: INPUT_KEYBOARD,
                    Anonymous: INPUT_0 {
                        ki: KEYBDINPUT {
                            wVk: VIRTUAL_KEY(0),
                            wScan: u,
                            dwFlags: flags,
                            time: 0,
                            dwExtraInfo: 0,
                        },
                    },
                })
            })
            .collect();
        self.send(&inputs);
    }

    fn absolute(&self, target: &InputTarget, nx: u16, ny: u16) -> (i32, i32) {
        let (px, py) = target.map(nx, ny);
        virtual_desk_abs(px, py, monitors::virtual_screen())
    }

    fn mouse(&mut self, m: &Mouse, target: &InputTarget) {
        let (ax, ay) = self.absolute(target, m.x, m.y);
        let move_flags = MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK;
        let mut inputs = vec![mouse_input(ax, ay, move_flags, 0)];
        match m.action {
            mouse_action::DOWN => {
                inputs.push(mouse_input(0, 0, button_flags(m.button, true), 0));
                if !self.mouse_buttons.contains(&m.button) {
                    self.mouse_buttons.push(m.button);
                }
            }
            mouse_action::UP => {
                inputs.push(mouse_input(0, 0, button_flags(m.button, false), 0));
                self.mouse_buttons.retain(|&b| b != m.button);
            }
            _ => {}
        }
        self.send(&inputs);
    }

    fn scroll(&mut self, s: &Scroll, target: &InputTarget) {
        let (ax, ay) = self.absolute(target, s.x, s.y);
        let (wheel, hwheel) = self.scroll.add(s.dx, s.dy, target);
        let mut inputs = vec![mouse_input(
            ax,
            ay,
            MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK,
            0,
        )];
        if wheel != 0 {
            inputs.push(mouse_input(0, 0, MOUSEEVENTF_WHEEL, wheel as u32));
        }
        if hwheel != 0 {
            inputs.push(mouse_input(0, 0, MOUSEEVENTF_HWHEEL, hwheel as u32));
        }
        if inputs.len() > 1 {
            self.send(&inputs);
        }
    }
}

impl Default for Injector {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for Injector {
    fn drop(&mut self) {
        self.reset();
        unsafe {
            if let Some(d) = self.pen.take() {
                DestroySyntheticPointerDevice(d);
            }
            if let Some(d) = self.touch.take() {
                DestroySyntheticPointerDevice(d);
            }
        }
    }
}

fn mouse_input(dx: i32, dy: i32, flags: MOUSE_EVENT_FLAGS, data: u32) -> INPUT {
    INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx,
                dy,
                mouseData: data,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

fn button_flags(button: u8, down: bool) -> MOUSE_EVENT_FLAGS {
    match (button, down) {
        (1, true) => MOUSEEVENTF_RIGHTDOWN,
        (1, false) => MOUSEEVENTF_RIGHTUP,
        (2, true) => MOUSEEVENTF_MIDDLEDOWN,
        (2, false) => MOUSEEVENTF_MIDDLEUP,
        (_, true) => MOUSEEVENTF_LEFTDOWN,
        (_, false) => MOUSEEVENTF_LEFTUP,
    }
}
