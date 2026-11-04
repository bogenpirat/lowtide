//! User settings, stored as TOML in `%APPDATA%\lowtide\config.toml`.
//! Every field has a default, so partial or older files load fine, and every
//! value is clamped to a sane range on load (all percentages are 0-100).

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::i18n::{t, Language, LanguageSetting};

pub const APP_DIR: &str = "lowtide";

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Anchor {
    TopLeft,
    Top,
    TopRight,
    Left,
    Center,
    Right,
    BottomLeft,
    Bottom,
    BottomRight,
}

impl Anchor {
    pub const ALL: [Anchor; 9] = [
        Anchor::TopLeft,
        Anchor::Top,
        Anchor::TopRight,
        Anchor::Left,
        Anchor::Center,
        Anchor::Right,
        Anchor::BottomLeft,
        Anchor::Bottom,
        Anchor::BottomRight,
    ];

    pub fn label(self) -> &'static str {
        let s = t();
        match self {
            Anchor::TopLeft => s.anchor_top_left,
            Anchor::Top => s.anchor_top,
            Anchor::TopRight => s.anchor_top_right,
            Anchor::Left => s.anchor_left,
            Anchor::Center => s.anchor_center,
            Anchor::Right => s.anchor_right,
            Anchor::BottomLeft => s.anchor_bottom_left,
            Anchor::Bottom => s.anchor_bottom,
            Anchor::BottomRight => s.anchor_bottom_right,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct TextStyle {
    pub font_family: String,
    pub size_pt: f32,
    /// 100 (thin) .. 900 (black); 400 regular, 700 bold.
    pub weight: u16,
    pub italic: bool,
    /// `#RRGGBB`
    pub text_color: String,
    /// 0-100 %
    pub text_opacity: u8,
    /// `#RRGGBB`
    pub background: String,
    /// 0-100 %; 0 means no background box.
    pub background_opacity: u8,
    /// Space between text and box edge, in DIPs.
    pub padding: u16,
    pub corner_radius: u16,
}

impl Default for TextStyle {
    fn default() -> Self {
        TextStyle {
            font_family: "Segoe UI".into(),
            size_pt: 20.0,
            weight: 600,
            italic: false,
            text_color: "#FFFFFF".into(),
            text_opacity: 100,
            background: "#B00020".into(),
            background_opacity: 80,
            padding: 12,
            corner_radius: 8,
        }
    }
}

impl TextStyle {
    fn sanitize(&mut self, fallback: &TextStyle) {
        if self.font_family.trim().is_empty() {
            self.font_family = fallback.font_family.clone();
        }
        if !self.size_pt.is_finite() {
            self.size_pt = fallback.size_pt;
        }
        self.size_pt = self.size_pt.clamp(6.0, 200.0);
        self.weight = self.weight.clamp(100, 900);
        if parse_color(&self.text_color).is_none() {
            self.text_color = fallback.text_color.clone();
        }
        if parse_color(&self.background).is_none() {
            self.background = fallback.background.clone();
        }
        self.text_opacity = self.text_opacity.min(100);
        self.background_opacity = self.background_opacity.min(100);
        self.padding = self.padding.min(200);
        self.corner_radius = self.corner_radius.min(200);
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct AlertConfig {
    /// Placeholders: `{device}`, `{percent}`, `{status}`. One line per low device.
    /// Empty means the default message of the current language.
    pub message: String,
    pub anchor: Anchor,
    /// Distance from the work-area edge, in DIPs.
    pub margin: i32,
    pub style: TextStyle,
}

impl AlertConfig {
    /// The template to use: the custom message, or the language's default.
    pub fn effective_message(&self) -> &str {
        if self.message.trim().is_empty() { t().alert_default_message } else { &self.message }
    }
}

impl Default for AlertConfig {
    fn default() -> Self {
        AlertConfig {
            message: String::new(),
            anchor: Anchor::BottomRight,
            margin: 24,
            style: TextStyle::default(),
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct InfoConfig {
    pub anchor: Anchor,
    pub margin: i32,
    /// Extra shift `[x, y]` in DIPs, applied after anchoring.
    pub offset: [i32; 2],
    pub duration_ms: u32,
    pub style: TextStyle,
}

impl Default for InfoConfig {
    fn default() -> Self {
        InfoConfig {
            anchor: Anchor::Center,
            margin: 24,
            offset: [0, 0],
            duration_ms: 2500,
            style: TextStyle {
                size_pt: 16.0,
                weight: 400,
                background: "#202428".into(),
                background_opacity: 88,
                padding: 16,
                ..TextStyle::default()
            },
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct Config {
    pub language: LanguageSetting,
    /// Show the alert below this charge (0-100 %).
    pub threshold_percent: u8,
    pub suppress_while_charging: bool,
    /// e.g. `Ctrl+Alt+Shift+B`; empty disables the hotkey.
    pub hotkey: String,
    pub poll_interval_s: u32,
    pub alert: AlertConfig,
    pub info: InfoConfig,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            language: LanguageSetting::Auto,
            threshold_percent: 15,
            suppress_while_charging: true,
            hotkey: "Ctrl+Alt+Shift+B".into(),
            poll_interval_s: 60,
            alert: AlertConfig::default(),
            info: InfoConfig::default(),
        }
    }
}

impl Config {
    pub fn sanitize(mut self) -> Config {
        let defaults = Config::default();
        self.threshold_percent = self.threshold_percent.min(100);
        self.poll_interval_s = self.poll_interval_s.clamp(5, 3600);
        self.alert.margin = self.alert.margin.clamp(-2000, 2000);
        self.info.margin = self.info.margin.clamp(-2000, 2000);
        self.info.offset = self.info.offset.map(|v| v.clamp(-4000, 4000));
        self.info.duration_ms = self.info.duration_ms.clamp(100, 60_000);
        self.alert.style.sanitize(&defaults.alert.style);
        self.info.style.sanitize(&defaults.info.style);
        // A default message saved in any language means "default", so it
        // follows a later language change instead of sticking in one language.
        let msg = self.alert.message.trim();
        if Language::ALL.iter().any(|l| l.strings().alert_default_message == msg) {
            self.alert.message.clear();
        }
        self
    }

    pub fn from_toml(text: &str) -> Result<Config, String> {
        toml::from_str::<Config>(text).map(Config::sanitize).map_err(|e| e.to_string())
    }

    pub fn to_toml(&self) -> String {
        toml::to_string_pretty(self).unwrap_or_default()
    }

    pub fn path() -> Option<PathBuf> {
        std::env::var_os("APPDATA").map(|d| PathBuf::from(d).join(APP_DIR).join("config.toml"))
    }

    /// Load the config, creating the file with defaults on first run.
    /// A broken file is left untouched (so edits are not lost) and defaults are used.
    pub fn load() -> (Config, Option<String>) {
        let Some(path) = Config::path() else {
            return (Config::default(), None);
        };
        match std::fs::read_to_string(&path) {
            Ok(text) => match Config::from_toml(&text) {
                Ok(cfg) => (cfg, None),
                Err(e) => (Config::default(), Some(format!("{}: {e}", path.display()))),
            },
            Err(_) => {
                let cfg = Config::default();
                let _ = cfg.save();
                (cfg, None)
            }
        }
    }

    /// Write atomically: temp file, then rename over the old one.
    pub fn save(&self) -> std::io::Result<()> {
        let path = Config::path().ok_or(std::io::ErrorKind::NotFound)?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("toml.tmp");
        std::fs::write(&tmp, self.to_toml())?;
        std::fs::rename(&tmp, &path)
    }
}

/// `#RRGGBB` -> (r, g, b)
pub fn parse_color(s: &str) -> Option<(u8, u8, u8)> {
    let hex = s.trim().strip_prefix('#')?;
    if hex.len() != 6 || !hex.is_ascii() {
        return None;
    }
    let v = u32::from_str_radix(hex, 16).ok()?;
    Some(((v >> 16) as u8, (v >> 8) as u8, v as u8))
}

pub fn format_color((r, g, b): (u8, u8, u8)) -> String {
    format!("#{r:02X}{g:02X}{b:02X}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_round_trip() {
        let cfg = Config::default();
        assert_eq!(Config::from_toml(&cfg.to_toml()).unwrap(), cfg);
    }

    #[test]
    fn partial_file_falls_back_to_defaults() {
        let cfg = Config::from_toml("threshold_percent = 30\n[alert]\nanchor = \"top_left\"\n").unwrap();
        assert_eq!(cfg.threshold_percent, 30);
        assert_eq!(cfg.alert.anchor, Anchor::TopLeft);
        assert_eq!(cfg.alert.message, AlertConfig::default().message);
        assert_eq!(cfg.info, InfoConfig::default());
    }

    #[test]
    fn percentages_are_clamped_to_0_100() {
        let cfg = Config::from_toml(
            "threshold_percent = 250\n[alert.style]\ntext_opacity = 180\nbackground_opacity = 101\n",
        )
        .unwrap();
        assert_eq!(cfg.threshold_percent, 100);
        assert_eq!(cfg.alert.style.text_opacity, 100);
        assert_eq!(cfg.alert.style.background_opacity, 100);
    }

    #[test]
    fn out_of_range_values_are_rejected_or_clamped() {
        // Negative percentages cannot even be represented: the file is rejected.
        assert!(Config::from_toml("threshold_percent = -5").is_err());
        let cfg = Config::from_toml("poll_interval_s = 0\n[info]\nduration_ms = 5\n").unwrap();
        assert_eq!(cfg.poll_interval_s, 5);
        assert_eq!(cfg.info.duration_ms, 100);
    }

    #[test]
    fn bad_colors_fall_back() {
        let cfg = Config::from_toml("[alert.style]\ntext_color = \"red\"\nbackground = \"#12345\"\n").unwrap();
        assert_eq!(cfg.alert.style.text_color, "#FFFFFF");
        assert_eq!(cfg.alert.style.background, "#B00020");
    }

    #[test]
    fn saved_default_message_follows_the_language() {
        let cfg = Config::from_toml("[alert]
message = \"{device} battery low: {percent}%\"
").unwrap();
        assert_eq!(cfg.alert.message, "");
        let custom = Config::from_toml("[alert]
message = \"Charge {device}!\"
").unwrap();
        assert_eq!(custom.alert.effective_message(), "Charge {device}!");
    }

    #[test]
    fn language_defaults_to_auto() {
        assert_eq!(Config::from_toml("").unwrap().language, LanguageSetting::Auto);
        assert_eq!(Config::from_toml("language = \"swabian\"").unwrap().language, LanguageSetting::Swabian);
    }

    #[test]
    fn color_parsing() {
        assert_eq!(parse_color("#B00020"), Some((0xB0, 0x00, 0x20)));
        assert_eq!(parse_color(" #ffffff "), Some((255, 255, 255)));
        assert_eq!(parse_color("B00020"), None);
        assert_eq!(format_color((0xB0, 0, 0x20)), "#B00020");
    }
}
