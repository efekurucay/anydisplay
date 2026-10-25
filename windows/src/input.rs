//! Platform-independent input translation (PROTOCOL.md 5.5) for the Windows injector:
//! coordinate mapping, touch slots, pen pointer states, modifier sync, scroll accumulation.

use crate::keymap;
use crate::proto::messages::{
    modifiers, pen_action, touch_action, PenSample, TouchContact, PEN_TOOL_ERASER,
};

/// A rectangle in desktop pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

/// Where input lands: the captured monitor on the desktop and the stream size the client sees.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InputTarget {
    pub monitor: Rect,
    pub stream_w: u32,
    pub stream_h: u32,
}

impl InputTarget {
    /// Normalized `u16` video coordinates (0 = left/top edge, 65535 = right/bottom edge) → desktop pixel.
    pub fn map(&self, nx: u16, ny: u16) -> (i32, i32) {
        let m = &self.monitor;
        let scale =
            |n: u16, len: i32| ((n as i64 * (len.max(1) as i64 - 1) + 32767) / 65535) as i32;
        (m.x + scale(nx, m.w), m.y + scale(ny, m.h))
    }

    /// Desktop pixels per stream pixel (for SCROLL deltas, which are stream pixels).
    pub fn scale(&self) -> (f64, f64) {
        (
            self.monitor.w as f64 / self.stream_w.max(1) as f64,
            self.monitor.h as f64 / self.stream_h.max(1) as f64,
        )
    }
}

/// Desktop pixel → `SendInput` absolute coordinate with `MOUSEEVENTF_VIRTUALDESK` (0..65535 over
/// the virtual screen; Windows maps back with `abs * width / 65536`).
pub fn virtual_desk_abs(px: i32, py: i32, vdesk: Rect) -> (i32, i32) {
    let f = |p: i32, origin: i32, len: i32| {
        let len = len.max(1) as i64;
        ((((p - origin) as i64) * 65536 + len - 1) / len).clamp(0, 65535) as i32
    };
    (f(px, vdesk.x, vdesk.w), f(py, vdesk.y, vdesk.h))
}

// ---------------------------------------------------------------- pointer flags (WinUser.h values)

pub mod pointer_flags {
    pub const INRANGE: u32 = 0x0000_0002;
    pub const INCONTACT: u32 = 0x0000_0004;
    pub const CANCELED: u32 = 0x0000_8000;
    pub const DOWN: u32 = 0x0001_0000;
    pub const UPDATE: u32 = 0x0002_0000;
    pub const UP: u32 = 0x0004_0000;
}

pub mod pen_flags {
    pub const BARREL: u32 = 0x1;
    pub const INVERTED: u32 = 0x2;
    pub const ERASER: u32 = 0x4;
}

use pointer_flags::*;

// ---------------------------------------------------------------- touch

pub const MAX_TOUCH_SLOTS: usize = 10;

/// One contact of a Windows touch frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TouchPoint {
    /// Pointer id 0..9.
    pub slot: u32,
    pub flags: u32,
    pub x: i32,
    pub y: i32,
    /// 0..1024.
    pub pressure: u32,
    /// Contact rectangle half-size in desktop pixels.
    pub radius: i32,
}

/// Maps client contact ids (0..255) to the 10 pointer ids of the synthetic touch device and
/// builds complete frames: every active contact appears in every injection.
#[derive(Default)]
pub struct TouchSlots {
    slots: [Option<(u8, TouchPoint)>; MAX_TOUCH_SLOTS],
}

impl TouchSlots {
    pub fn frame(&mut self, contacts: &[TouchContact], target: &InputTarget) -> Vec<TouchPoint> {
        let mut out = Vec::new();
        let mut seen = [false; MAX_TOUCH_SLOTS];
        for c in contacts {
            let existing = self
                .slots
                .iter()
                .position(|s| matches!(s, Some((id, _)) if *id == c.id));
            let (x, y) = target.map(c.x, c.y);
            let ending = c.action == touch_action::UP || c.action == touch_action::CANCEL;
            let (slot, flags) = match existing {
                Some(i) if ending => (
                    i,
                    if c.action == touch_action::CANCEL {
                        UP | CANCELED
                    } else {
                        UP
                    },
                ),
                Some(i) => (i, UPDATE | INRANGE | INCONTACT),
                None if ending => continue,
                None => match self.slots.iter().position(Option::is_none) {
                    Some(i) => (i, DOWN | INRANGE | INCONTACT),
                    None => continue, // more than 10 fingers
                },
            };
            let radius =
                ((c.size as f64 / 65535.0) * target.monitor.w as f64 / 2.0).clamp(2.0, 64.0) as i32;
            let p = TouchPoint {
                slot: slot as u32,
                flags,
                x,
                y,
                pressure: (c.pressure as u32 * 1024) / 65535,
                radius,
            };
            seen[slot] = true;
            self.slots[slot] = if ending { None } else { Some((c.id, p)) };
            out.push(p);
        }
        // A contact the client stopped listing has ended.
        for (i, s) in self.slots.iter_mut().enumerate() {
            if let (Some((_, p)), false) = (s.as_ref(), seen[i]) {
                out.push(TouchPoint { flags: UP, ..*p });
                *s = None;
            }
        }
        out
    }

    /// Re-sends the active contacts unchanged (keeps stationary fingers alive).
    pub fn repeat(&self) -> Vec<TouchPoint> {
        self.slots
            .iter()
            .flatten()
            .map(|(_, p)| TouchPoint {
                flags: UPDATE | INRANGE | INCONTACT,
                ..*p
            })
            .collect()
    }

    /// Lifts every active contact.
    pub fn cancel_all(&mut self) -> Vec<TouchPoint> {
        self.slots
            .iter_mut()
            .filter_map(|s| s.take())
            .map(|(_, p)| TouchPoint {
                flags: UP | CANCELED,
                ..p
            })
            .collect()
    }

    pub fn active(&self) -> bool {
        self.slots.iter().any(Option::is_some)
    }
}

// ---------------------------------------------------------------- pen

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PenPoint {
    pub flags: u32,
    pub pen_flags: u32,
    pub x: i32,
    pub y: i32,
    /// 0..1024.
    pub pressure: u32,
    /// Degrees, -90..90.
    pub tilt_x: i32,
    pub tilt_y: i32,
    /// Degrees, 0..359.
    pub rotation: u32,
}

/// Turns PEN samples into synthetic pointer states: hover enter/leave, contact down/up, and a
/// leave/re-enter when the tool flips between tip and eraser, so applications see the change.
#[derive(Default)]
pub struct PenTracker {
    in_range: bool,
    in_contact: bool,
    eraser: bool,
    last: Option<PenPoint>,
}

impl PenTracker {
    pub fn sample(&mut self, s: &PenSample, target: &InputTarget) -> Vec<PenPoint> {
        let (x, y) = target.map(s.x, s.y);
        let eraser = s.tool == PEN_TOOL_ERASER;
        let mut out = Vec::new();
        if self.in_range && eraser != self.eraser {
            out.extend(self.leave());
        }
        self.eraser = eraser;
        let mut pen = if eraser { pen_flags::INVERTED } else { 0 };
        if s.buttons != 0 {
            pen |= pen_flags::BARREL;
        }
        let base = PenPoint {
            flags: 0,
            pen_flags: pen,
            x,
            y,
            pressure: (s.pressure as u32 * 1024) / 65535,
            tilt_x: (s.tilt_x as i32 / 100).clamp(-90, 90),
            tilt_y: (s.tilt_y as i32 / 100).clamp(-90, 90),
            rotation: (s.twist as u32 / 100) % 360,
        };
        let contact_pen = pen | if eraser { pen_flags::ERASER } else { 0 };
        match s.action {
            pen_action::HOVER_ENTER | pen_action::HOVER_MOVE => {
                if self.in_contact {
                    out.push(self.emit(PenPoint {
                        flags: INRANGE | UP,
                        pressure: 0,
                        ..base
                    }));
                    self.in_contact = false;
                }
                out.push(self.emit(PenPoint {
                    flags: INRANGE | UPDATE,
                    pressure: 0,
                    ..base
                }));
                self.in_range = true;
            }
            pen_action::DOWN | pen_action::MOVE => {
                if !self.in_range {
                    out.push(self.emit(PenPoint {
                        flags: INRANGE | UPDATE,
                        pressure: 0,
                        ..base
                    }));
                    self.in_range = true;
                }
                let flags = if self.in_contact {
                    INRANGE | INCONTACT | UPDATE
                } else {
                    INRANGE | INCONTACT | DOWN
                };
                out.push(self.emit(PenPoint {
                    flags,
                    pen_flags: contact_pen,
                    ..base
                }));
                self.in_contact = true;
            }
            pen_action::UP => {
                if self.in_contact {
                    out.push(self.emit(PenPoint {
                        flags: INRANGE | UP,
                        pressure: 0,
                        ..base
                    }));
                    self.in_contact = false;
                }
            }
            pen_action::HOVER_EXIT => {
                if self.in_range || self.in_contact {
                    self.last = Some(PenPoint {
                        x,
                        y,
                        ..self.last.unwrap_or(base)
                    });
                    out.extend(self.leave());
                }
            }
            pen_action::CANCEL => out.extend(self.cancel()),
            _ => {}
        }
        out
    }

    fn emit(&mut self, p: PenPoint) -> PenPoint {
        self.last = Some(p);
        p
    }

    /// Lift (if touching) and leave range at the last position.
    fn leave(&mut self) -> Vec<PenPoint> {
        let Some(last) = self.last else { return vec![] };
        let mut out = Vec::new();
        if self.in_contact {
            out.push(PenPoint {
                flags: INRANGE | UP,
                pressure: 0,
                ..last
            });
        }
        out.push(PenPoint {
            flags: UPDATE,
            pressure: 0,
            ..last
        });
        self.in_contact = false;
        self.in_range = false;
        out
    }

    pub fn cancel(&mut self) -> Vec<PenPoint> {
        let Some(last) = self.last else { return vec![] };
        let out = if self.in_contact {
            vec![PenPoint {
                flags: UP | CANCELED,
                pressure: 0,
                ..last
            }]
        } else if self.in_range {
            vec![PenPoint {
                flags: UPDATE | CANCELED,
                pressure: 0,
                ..last
            }]
        } else {
            vec![]
        };
        self.in_contact = false;
        self.in_range = false;
        out
    }
}

// ---------------------------------------------------------------- keyboard

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyStroke {
    pub hid_usage: u16,
    pub down: bool,
}

/// KEY carries a modifier bitmask with every key. Windows reads modifier state from key events,
/// so modifiers in the mask that are not already held get pressed around the key and released after.
#[derive(Default)]
pub struct ModifierSync {
    /// Modifier usages the client pressed explicitly.
    held: Vec<u16>,
    /// Modifier usages this sync pressed on the client's behalf.
    synthesized: Vec<u16>,
}

const MODIFIER_BITS: [(u16, u16, u16); 4] = [
    // (mask bit, left usage, right usage)
    (modifiers::CTRL, 0xE0, 0xE4),
    (modifiers::SHIFT, 0xE1, 0xE5),
    (modifiers::ALT, 0xE2, 0xE6),
    (modifiers::META, 0xE3, 0xE7),
];

impl ModifierSync {
    pub fn key(&mut self, down: bool, hid_usage: u16, mods: u16) -> Vec<KeyStroke> {
        let mut out = Vec::new();
        if keymap::is_modifier(hid_usage) {
            self.held.retain(|&u| u != hid_usage);
            self.synthesized.retain(|&u| u != hid_usage);
            if down {
                self.held.push(hid_usage);
            }
            out.push(KeyStroke { hid_usage, down });
            return out;
        }
        if down {
            for (bit, left, right) in MODIFIER_BITS {
                let active = self
                    .held
                    .iter()
                    .chain(&self.synthesized)
                    .any(|&u| u == left || u == right);
                if mods & bit != 0 && !active {
                    self.synthesized.push(left);
                    out.push(KeyStroke {
                        hid_usage: left,
                        down: true,
                    });
                } else if mods & bit == 0 {
                    // The client says this modifier is up: drop our synthesized press.
                    if let Some(i) = self
                        .synthesized
                        .iter()
                        .position(|&u| u == left || u == right)
                    {
                        out.push(KeyStroke {
                            hid_usage: self.synthesized.remove(i),
                            down: false,
                        });
                    }
                }
            }
            out.push(KeyStroke {
                hid_usage,
                down: true,
            });
        } else {
            out.push(KeyStroke {
                hid_usage,
                down: false,
            });
            out.extend(self.synthesized.drain(..).rev().map(|u| KeyStroke {
                hid_usage: u,
                down: false,
            }));
        }
        out
    }

    /// Releases everything still pressed (end of session).
    pub fn release_all(&mut self) -> Vec<KeyStroke> {
        let mut out: Vec<KeyStroke> = self
            .synthesized
            .drain(..)
            .map(|u| KeyStroke {
                hid_usage: u,
                down: false,
            })
            .collect();
        out.extend(self.held.drain(..).map(|u| KeyStroke {
            hid_usage: u,
            down: false,
        }));
        out
    }
}

// ---------------------------------------------------------------- scroll

/// SCROLL deltas are stream pixels, positive = content moves right/down. Windows wheel deltas
/// are positive for "scroll up" (content moves down) and "scroll right" (content moves left);
/// `WHEEL_DELTA` (120) is one notch, about three lines.
pub struct ScrollAccumulator {
    /// Wheel units per desktop pixel.
    pub units_per_px: f64,
    acc: (f64, f64),
}

impl Default for ScrollAccumulator {
    fn default() -> Self {
        // 60 desktop px (three ~20 px lines) per notch.
        Self {
            units_per_px: 2.0,
            acc: (0.0, 0.0),
        }
    }
}

impl ScrollAccumulator {
    /// Returns `(wheel, hwheel)` deltas to inject now; fractions carry over.
    pub fn add(&mut self, dx: i16, dy: i16, target: &InputTarget) -> (i32, i32) {
        let (sx, sy) = target.scale();
        self.acc.0 += -(dx as f64) * sx * self.units_per_px;
        self.acc.1 += dy as f64 * sy * self.units_per_px;
        let h = self.acc.0.trunc();
        let v = self.acc.1.trunc();
        self.acc.0 -= h;
        self.acc.1 -= v;
        (v as i32, h as i32)
    }

    pub fn reset(&mut self) {
        self.acc = (0.0, 0.0);
    }
}

/// UTF-16 units for `KEYEVENTF_UNICODE` (surrogate pairs stay as two units).
pub fn utf16_units(text: &str) -> Vec<u16> {
    text.encode_utf16().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::messages::PEN_BUTTON_BARREL;

    fn target() -> InputTarget {
        InputTarget {
            monitor: Rect {
                x: 1920,
                y: -200,
                w: 2560,
                h: 1600,
            },
            stream_w: 1280,
            stream_h: 800,
        }
    }

    #[test]
    fn maps_normalized_edges_onto_monitor() {
        let t = target();
        assert_eq!(t.map(0, 0), (1920, -200));
        assert_eq!(t.map(65535, 65535), (1920 + 2559, -200 + 1599));
        assert_eq!(t.map(32768, 32768), (1920 + 1280, -200 + 800));
        assert_eq!(t.scale(), (2.0, 2.0));
    }

    #[test]
    fn virtual_desk_round_trips_through_windows_formula() {
        let vd = Rect {
            x: -1280,
            y: 0,
            w: 1280 + 1920 + 2560,
            h: 1600,
        };
        for px in [-1280, -1, 0, 1919, 1920, 4479] {
            let (ax, _) = virtual_desk_abs(px, 0, vd);
            assert_eq!(
                vd.x + (ax as i64 * vd.w as i64 / 65536) as i32,
                px,
                "px {px}"
            );
        }
    }

    fn c(id: u8, action: u8, x: u16) -> TouchContact {
        TouchContact {
            id,
            action,
            x,
            y: 0,
            pressure: 65535,
            size: 0,
        }
    }

    #[test]
    fn touch_frames_track_slots() {
        let t = target();
        let mut s = TouchSlots::default();
        let f = s.frame(&[c(40, touch_action::DOWN, 0)], &t);
        assert_eq!(
            (f[0].slot, f[0].flags, f[0].pressure),
            (0, DOWN | INRANGE | INCONTACT, 1024)
        );
        let f = s.frame(
            &[
                c(40, touch_action::MOVE, 100),
                c(7, touch_action::DOWN, 200),
            ],
            &t,
        );
        assert_eq!(
            f.iter().map(|p| (p.slot, p.flags)).collect::<Vec<_>>(),
            vec![
                (0, UPDATE | INRANGE | INCONTACT),
                (1, DOWN | INRANGE | INCONTACT)
            ]
        );
        let f = s.frame(
            &[c(40, touch_action::UP, 100), c(7, touch_action::MOVE, 200)],
            &t,
        );
        assert_eq!(
            f.iter().map(|p| (p.slot, p.flags)).collect::<Vec<_>>(),
            vec![(0, UP), (1, UPDATE | INRANGE | INCONTACT)]
        );
        assert_eq!(s.repeat().len(), 1);
        // Slot 0 is free again; a contact missing from the frame ends.
        let f = s.frame(&[c(9, touch_action::DOWN, 0)], &t);
        assert_eq!(
            f.iter().map(|p| (p.slot, p.flags)).collect::<Vec<_>>(),
            vec![(0, DOWN | INRANGE | INCONTACT), (1, UP)]
        );
        assert_eq!(s.cancel_all()[0].flags, UP | CANCELED);
        assert!(!s.active());
    }

    fn pen(action: u8, tool: u8) -> PenSample {
        PenSample {
            action,
            tool,
            buttons: 0,
            x: 0,
            y: 0,
            pressure: 32768,
            tilt_x: -4550,
            tilt_y: 9999,
            twist: 36050,
            time_ms: 0,
        }
    }

    #[test]
    fn pen_states() {
        let t = target();
        let mut p = PenTracker::default();
        let flags = |v: Vec<PenPoint>| v.iter().map(|p| p.flags).collect::<Vec<_>>();
        assert_eq!(
            flags(p.sample(&pen(pen_action::HOVER_ENTER, 0), &t)),
            vec![INRANGE | UPDATE]
        );
        let down = p.sample(&pen(pen_action::DOWN, 0), &t);
        assert_eq!(flags(down.clone()), vec![INRANGE | INCONTACT | DOWN]);
        assert_eq!(
            (
                down[0].pressure,
                down[0].tilt_x,
                down[0].tilt_y,
                down[0].rotation
            ),
            (512, -45, 90, 0)
        );
        assert_eq!(
            flags(p.sample(&pen(pen_action::MOVE, 0), &t)),
            vec![INRANGE | INCONTACT | UPDATE]
        );
        assert_eq!(
            flags(p.sample(&pen(pen_action::UP, 0), &t)),
            vec![INRANGE | UP]
        );
        // Flipping to the eraser while hovering leaves and re-enters inverted.
        let flip = p.sample(&pen(pen_action::HOVER_MOVE, PEN_TOOL_ERASER), &t);
        assert_eq!(flags(flip.clone()), vec![UPDATE, INRANGE | UPDATE]);
        assert_eq!(flip[1].pen_flags, pen_flags::INVERTED);
        let mut s = pen(pen_action::DOWN, PEN_TOOL_ERASER);
        s.buttons = PEN_BUTTON_BARREL;
        assert_eq!(
            p.sample(&s, &t)[0].pen_flags,
            pen_flags::INVERTED | pen_flags::ERASER | pen_flags::BARREL
        );
        assert_eq!(
            flags(p.sample(&pen(pen_action::HOVER_EXIT, PEN_TOOL_ERASER), &t)),
            vec![INRANGE | UP, UPDATE]
        );
        // Down without hover enters range first; cancel lifts.
        assert_eq!(
            flags(p.sample(&pen(pen_action::DOWN, 0), &t)),
            vec![INRANGE | UPDATE, INRANGE | INCONTACT | DOWN]
        );
        assert_eq!(
            flags(p.sample(&pen(pen_action::CANCEL, 0), &t)),
            vec![UP | CANCELED]
        );
    }

    #[test]
    fn modifiers_are_synthesized_around_keys() {
        let mut m = ModifierSync::default();
        let k = |u, d| KeyStroke {
            hid_usage: u,
            down: d,
        };
        // Ctrl+C with only the mask.
        assert_eq!(
            m.key(true, 0x06, modifiers::CTRL),
            vec![k(0xE0, true), k(0x06, true)]
        );
        assert_eq!(
            m.key(false, 0x06, modifiers::CTRL),
            vec![k(0x06, false), k(0xE0, false)]
        );
        // Explicit shift held: not synthesized again, not released by the key.
        assert_eq!(m.key(true, 0xE5, modifiers::SHIFT), vec![k(0xE5, true)]);
        assert_eq!(m.key(true, 0x04, modifiers::SHIFT), vec![k(0x04, true)]);
        assert_eq!(m.key(false, 0x04, modifiers::SHIFT), vec![k(0x04, false)]);
        assert_eq!(m.release_all(), vec![k(0xE5, false)]);
    }

    #[test]
    fn scroll_direction_and_accumulation() {
        let t = target(); // 2 desktop px per stream px
        let mut s = ScrollAccumulator::default();
        // Content moves down = wheel up (positive); content moves right = hwheel left (negative).
        assert_eq!(s.add(0, 30, &t), (120, 0));
        assert_eq!(s.add(15, 0, &t), (0, -60));
        s.units_per_px = 0.1;
        assert_eq!(s.add(0, 1, &t), (0, 0));
        assert_eq!(s.add(0, 4, &t), (1, 0));
    }

    #[test]
    fn utf16_keeps_surrogates() {
        assert_eq!(utf16_units("aé😀"), vec![0x61, 0xE9, 0xD83D, 0xDE00]);
    }
}
