//! Hotkey strings like `Ctrl+Alt+Shift+B` <-> modifier flags and virtual key,
//! plus conversion to and from the native hotkey control's encoding.

use windows::Win32::UI::Input::KeyboardAndMouse::{
    HOT_KEY_MODIFIERS, MOD_ALT, MOD_CONTROL, MOD_NOREPEAT, MOD_SHIFT, MOD_WIN,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Hotkey {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub win: bool,
    pub vk: u16,
}

/// Named keys besides letters, digits and F-keys.
const NAMED_KEYS: &[(&str, u16)] = &[
    ("Space", 0x20),
    ("PageUp", 0x21),
    ("PageDown", 0x22),
    ("End", 0x23),
    ("Home", 0x24),
    ("Left", 0x25),
    ("Up", 0x26),
    ("Right", 0x27),
    ("Down", 0x28),
    ("PrintScreen", 0x2C),
    ("Insert", 0x2D),
    ("Delete", 0x2E),
    ("Pause", 0x13),
    ("ScrollLock", 0x91),
    ("NumMultiply", 0x6A),
    ("NumAdd", 0x6B),
    ("NumSubtract", 0x6D),
    ("NumDecimal", 0x6E),
    ("NumDivide", 0x6F),
    (";", 0xBA),
    ("=", 0xBB),
    (",", 0xBC),
    ("-", 0xBD),
    (".", 0xBE),
    ("/", 0xBF),
    ("`", 0xC0),
    ("[", 0xDB),
    ("\\", 0xDC),
    ("]", 0xDD),
    ("'", 0xDE),
];

fn key_from_name(name: &str) -> Option<u16> {
    let upper = name.to_ascii_uppercase();
    let b = upper.as_bytes();
    if b.len() == 1 && (b[0].is_ascii_uppercase() || b[0].is_ascii_digit()) {
        return Some(u16::from(b[0]));
    }
    if let Some(n) = upper.strip_prefix('F').and_then(|n| n.parse::<u16>().ok())
        && (1..=24).contains(&n) {
            return Some(0x70 + n - 1);
        }
    if let Some(n) = upper.strip_prefix("NUM").and_then(|n| n.parse::<u16>().ok())
        && n <= 9 {
            return Some(0x60 + n);
        }
    NAMED_KEYS.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|&(_, vk)| vk)
}

fn key_name(vk: u16) -> Option<String> {
    match vk {
        0x30..=0x39 | 0x41..=0x5A => Some(char::from(vk as u8).to_string()),
        0x60..=0x69 => Some(format!("Num{}", vk - 0x60)),
        0x70..=0x87 => Some(format!("F{}", vk - 0x70 + 1)),
        _ => NAMED_KEYS.iter().find(|&&(_, v)| v == vk).map(|(k, _)| k.to_string()),
    }
}

impl Hotkey {
    /// Parse `Ctrl+Alt+B`. Empty input means "no hotkey".
    pub fn parse(s: &str) -> Result<Option<Hotkey>, String> {
        let s = s.trim();
        if s.is_empty() {
            return Ok(None);
        }
        let mut hk = Hotkey { ctrl: false, alt: false, shift: false, win: false, vk: 0 };
        for part in s.split('+').map(str::trim) {
            match part.to_ascii_lowercase().as_str() {
                "ctrl" | "control" => hk.ctrl = true,
                "alt" => hk.alt = true,
                "shift" => hk.shift = true,
                "win" | "super" => hk.win = true,
                _ if hk.vk != 0 => return Err(format!("more than one key in \"{s}\"")),
                _ => hk.vk = key_from_name(part).ok_or_else(|| format!("unknown key \"{part}\""))?,
            }
        }
        if hk.vk == 0 {
            return Err(format!("no key in \"{s}\""));
        }
        if !(hk.ctrl || hk.alt || hk.win) {
            return Err("a hotkey needs Ctrl, Alt or Win".into());
        }
        Ok(Some(hk))
    }

    pub fn modifiers(&self) -> HOT_KEY_MODIFIERS {
        let mut m = MOD_NOREPEAT;
        if self.ctrl {
            m |= MOD_CONTROL;
        }
        if self.alt {
            m |= MOD_ALT;
        }
        if self.shift {
            m |= MOD_SHIFT;
        }
        if self.win {
            m |= MOD_WIN;
        }
        m
    }

    /// From the hotkey control (`HKM_GETHOTKEY`): low byte vk, high byte HOTKEYF_* flags.
    pub fn from_control(value: u16) -> Option<Hotkey> {
        let vk = value & 0xFF;
        let flags = value >> 8;
        (vk != 0).then_some(Hotkey {
            shift: flags & 0x01 != 0,
            ctrl: flags & 0x02 != 0,
            alt: flags & 0x04 != 0,
            win: false,
            vk,
        })
    }

    pub fn to_control(self) -> u16 {
        let flags = u16::from(self.shift) | u16::from(self.ctrl) << 1 | u16::from(self.alt) << 2;
        flags << 8 | (self.vk & 0xFF)
    }
}

impl std::fmt::Display for Hotkey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut parts = Vec::new();
        if self.ctrl {
            parts.push("Ctrl".to_string());
        }
        if self.alt {
            parts.push("Alt".to_string());
        }
        if self.shift {
            parts.push("Shift".to_string());
        }
        if self.win {
            parts.push("Win".to_string());
        }
        parts.push(key_name(self.vk).unwrap_or_else(|| format!("0x{:02X}", self.vk)));
        f.write_str(&parts.join("+"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_default_hotkey() {
        let hk = Hotkey::parse("Ctrl+Alt+Shift+B").unwrap().unwrap();
        assert_eq!(hk, Hotkey { ctrl: true, alt: true, shift: true, win: false, vk: 0x42 });
        assert_eq!(hk.to_string(), "Ctrl+Alt+Shift+B");
    }

    #[test]
    fn parses_named_and_function_keys_case_insensitively() {
        assert_eq!(Hotkey::parse("win+f12").unwrap().unwrap().vk, 0x7B);
        assert_eq!(Hotkey::parse("ctrl + PageUp").unwrap().unwrap().vk, 0x21);
        assert_eq!(Hotkey::parse("Alt+Num5").unwrap().unwrap().vk, 0x65);
        assert_eq!(Hotkey::parse("Ctrl+.").unwrap().unwrap().to_string(), "Ctrl+.");
    }

    #[test]
    fn rejects_bad_input() {
        assert_eq!(Hotkey::parse("  "), Ok(None));
        assert!(Hotkey::parse("B").is_err(), "needs a modifier");
        assert!(Hotkey::parse("Shift+B").is_err(), "shift alone is too easy to hit");
        assert!(Hotkey::parse("Ctrl+Alt").is_err());
        assert!(Hotkey::parse("Ctrl+A+B").is_err());
        assert!(Hotkey::parse("Ctrl+Banana").is_err());
    }

    #[test]
    fn hotkey_control_round_trip() {
        let hk = Hotkey::parse("Ctrl+Shift+F9").unwrap().unwrap();
        assert_eq!(Hotkey::from_control(hk.to_control()), Some(hk));
        assert_eq!(Hotkey::from_control(0), None);
    }
}
