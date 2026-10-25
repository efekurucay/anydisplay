//! Recognizing the monitor of the open-source Virtual Display Driver
//! (github.com/VirtualDrivers/Virtual-Display-Driver, MIT; an IddCx driver). Its default EDID
//! uses PnP manufacturer `MTT`, product code `0x1337`, monitor name "VDD by MTT"; Windows shows
//! the monitor device path as `\\?\DISPLAY#MTT1337#...`.

pub const PNP_ID: &str = "MTT";
pub const PRODUCT_CODE: u16 = 0x1337;
pub const MONITOR_NAME: &str = "VDD by MTT";

/// Decodes a PnP manufacturer id as stored in EDID bytes 8-9 (big-endian, three 5-bit letters).
pub fn pnp_id(edid_be: u16) -> String {
    [
        (edid_be >> 10) & 0x1f,
        (edid_be >> 5) & 0x1f,
        edid_be & 0x1f,
    ]
    .iter()
    .map(|&c| {
        if (1..=26).contains(&c) {
            (b'A' + c as u8 - 1) as char
        } else {
            '?'
        }
    })
    .collect()
}

/// `edid_ids`: `(edidManufactureId, edidProductCodeId)` from DisplayConfig when valid. The
/// manufacturer id is checked in both byte orders since APIs differ in how they expose it.
pub fn is_vdd_monitor(
    friendly_name: &str,
    device_path: &str,
    edid_ids: Option<(u16, u16)>,
) -> bool {
    if friendly_name.trim() == MONITOR_NAME {
        return true;
    }
    if device_path
        .to_ascii_uppercase()
        .contains(&format!("#{PNP_ID}{PRODUCT_CODE:04X}#"))
    {
        return true;
    }
    matches!(edid_ids, Some((m, p)) if p == PRODUCT_CODE && (pnp_id(m) == PNP_ID || pnp_id(m.swap_bytes()) == PNP_ID))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_vdd() {
        assert_eq!(pnp_id(0x3694), "MTT");
        assert!(is_vdd_monitor("VDD by MTT", "", None));
        assert!(is_vdd_monitor(
            "",
            r"\\?\DISPLAY#MTT1337#5&2b7e4d0&0&UID256#{e6f07b5f-ee97-4a90-b076-33f57bf4eaa7}",
            None
        ));
        assert!(is_vdd_monitor("Custom", "", Some((0x9436, 0x1337))));
        assert!(!is_vdd_monitor(
            "DELL U2720Q",
            r"\\?\DISPLAY#DELA0B5#...",
            Some((0x10ac, 0xa0b5))
        ));
    }
}
