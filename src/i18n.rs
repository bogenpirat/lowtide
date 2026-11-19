//! User-visible text in English, German and Swabian.
//!
//! Every language is a complete [`Strings`] table, so a missing translation is
//! a compile error. Templates use named `{placeholders}` filled by [`fill`];
//! a test checks that all languages use the same placeholders.

use std::sync::atomic::{AtomicU8, Ordering};

use serde::{Deserialize, Serialize};

/// The configured choice; `Auto` follows the Windows display language.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LanguageSetting {
    #[default]
    Auto,
    #[serde(rename = "en")]
    English,
    #[serde(rename = "de")]
    German,
    Swabian,
}

impl LanguageSetting {
    pub const ALL: [LanguageSetting; 4] =
        [LanguageSetting::Auto, LanguageSetting::English, LanguageSetting::German, LanguageSetting::Swabian];

    pub fn resolve(self) -> Language {
        match self {
            LanguageSetting::Auto => system_language(),
            LanguageSetting::English => Language::English,
            LanguageSetting::German => Language::German,
            LanguageSetting::Swabian => Language::Swabian,
        }
    }

    /// Name shown in the language picker: endonyms, plus "automatic" in the current language.
    pub fn label(self) -> &'static str {
        match self {
            LanguageSetting::Auto => t().language_auto,
            LanguageSetting::English => "English",
            LanguageSetting::German => "Deutsch",
            LanguageSetting::Swabian => "Schwäbisch",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Language {
    English = 0,
    German = 1,
    Swabian = 2,
}

impl Language {
    pub const ALL: [Language; 3] = [Language::English, Language::German, Language::Swabian];

    pub fn strings(self) -> &'static Strings {
        match self {
            Language::English => &EN,
            Language::German => &DE,
            Language::Swabian => &SWABIAN,
        }
    }
}

static CURRENT: AtomicU8 = AtomicU8::new(Language::English as u8);

pub fn set_language(lang: Language) {
    CURRENT.store(lang as u8, Ordering::Relaxed);
}

pub fn current() -> Language {
    match CURRENT.load(Ordering::Relaxed) {
        1 => Language::German,
        2 => Language::Swabian,
        _ => Language::English,
    }
}

/// Strings of the active language.
pub fn t() -> &'static Strings {
    current().strings()
}

/// German if Windows is displayed in German, otherwise English.
fn system_language() -> Language {
    const LANG_GERMAN: u16 = 0x07;
    let langid = unsafe { windows::Win32::Globalization::GetUserDefaultUILanguage() };
    if langid & 0x3FF == LANG_GERMAN { Language::German } else { Language::English }
}

/// Fill `{name}` placeholders: `fill!(t().err_range, field = name, min = 0, max = 100)`.
macro_rules! fill {
    ($template:expr $(, $name:ident = $value:expr)* $(,)?) => {{
        #[allow(unused_mut)]
        let mut out = String::from($template);
        $( out = out.replace(concat!("{", stringify!($name), "}"), &$value.to_string()); )*
        out
    }};
}
pub(crate) use fill;

macro_rules! strings {
    ($($field:ident),* $(,)?) => {
        pub struct Strings {
            $(pub $field: &'static str,)*
        }

        impl Strings {
            /// (field name, text) for every entry; used by tests.
            #[cfg_attr(not(test), allow(dead_code))]
            pub fn entries(&self) -> Vec<(&'static str, &'static str)> {
                vec![$((stringify!($field), self.$field)),*]
            }
        }
    };
}

strings! {
    // battery and connection state
    charge_discharging, charge_charging, charge_full, charge_not_charging, charge_error, charge_unknown,
    connection_wired, connection_receiver,
    // overlays
    alert_default_message,
    info_no_devices, info_external_power, info_no_battery, info_not_responding, info_not_responding_since,
    info_firmware, duration_seconds, duration_minutes, duration_hours,
    // tray
    tray_scanning, tray_no_devices, tray_device, tray_not_responding,
    menu_status, menu_refresh, menu_mute, menu_settings, menu_quit, tray_muted,
    notify_config_error, notify_hotkey_unavailable, hotkey_in_use,
    app_busy, autostart_failed, save_failed, start_failed,
    // devices
    sample_device, simulated_device, generic_logitech_device,
    // hotkey parsing
    hotkey_two_keys, hotkey_unknown_key, hotkey_no_key, hotkey_needs_modifier,
    // settings dialog
    dlg_title, dlg_general, dlg_threshold, dlg_suppress, dlg_poll, dlg_seconds, dlg_hotkey,
    dlg_autostart, dlg_language, language_auto,
    dlg_alert_group, dlg_message, dlg_placeholders, dlg_position, dlg_margin,
    dlg_font_alert, dlg_font_info, dlg_text_color, dlg_opacity, dlg_background, dlg_padding, dlg_radius,
    dlg_fade, dlg_fade_down_to, dlg_fade_charge_unit, dlg_fade_from, dlg_fade_to,
    dlg_info_group, dlg_offset_x, dlg_show_for,
    dlg_preview, dlg_ok, dlg_cancel, dlg_apply, dlg_open_failed,
    anchor_top_left, anchor_top, anchor_top_right, anchor_left, anchor_center, anchor_right,
    anchor_bottom_left, anchor_bottom, anchor_bottom_right,
    font_label, font_italic,
    weight_thin, weight_extra_light, weight_light, weight_regular, weight_medium, weight_semibold,
    weight_bold, weight_extra_bold, weight_black,
    err_range, err_hotkey, err_message_empty, err_fade_opacity, err_fade_charge,
    field_threshold, field_poll, field_alert_margin, field_text_opacity, field_bg_opacity,
    field_padding, field_radius, field_info_margin, field_offset_x, field_offset_y, field_duration,
    field_fade_start, field_fade_end, field_fade_charge,
    // statistics window
    menu_statistics, notify_history_failed, stats_title, stats_open_failed, stats_close, stats_device,
    stats_current, stats_last_reading, stats_no_history,
    stats_forecast_group, stats_lasts, stats_lasts_charging, stats_alert_in, stats_alert_below,
    stats_full_cycles, stats_full_usage, stats_basis_week, stats_basis_all, stats_not_enough,
    stats_usage_group, stats_col_period, stats_col_used, stats_col_per_day, stats_col_charges, stats_col_covered,
    stats_period_day, stats_period_week, stats_period_month, stats_period_all,
    stats_charging_group, stats_last_charge, stats_charging_since, stats_no_charge, stats_between,
    stats_charge_time, stats_lowest, stats_since,
    stats_chart_group, stats_chart_start, stats_chart_today, stats_chart_empty,
    percent, decimal_separator, duration_days, duration_one_day, duration_short_hours,
    // command line
    cli_source_count, cli_scan_summary, cli_watching, cli_no_devices, cli_device_line, cli_estimated, cli_stale,
}

pub static EN: Strings = Strings {
    charge_discharging: "discharging",
    charge_charging: "charging",
    charge_full: "fully charged",
    charge_not_charging: "not charging",
    charge_error: "charging error",
    charge_unknown: "unknown",
    connection_wired: "wired (USB cable)",
    connection_receiver: "wireless (receiver {pid}, slot {slot})",

    alert_default_message: "{device} battery low: {percent}%",
    info_no_devices: "No battery-powered devices found",
    info_external_power: ", on external power",
    info_no_battery: "no battery information",
    info_not_responding: "not responding",
    info_not_responding_since: "not responding, last read {ago} ago",
    info_firmware: "Firmware {version}",
    duration_seconds: "{s} s",
    duration_minutes: "{m} min",
    duration_hours: "{h} h {m} min",

    tray_scanning: "Lowtide: looking for devices…",
    tray_no_devices: "Lowtide: no devices found",
    tray_device: "{device}: {percent}% ({status})",
    tray_not_responding: ", not responding",
    menu_status: "Show battery status",
    menu_refresh: "Refresh now",
    menu_settings: "Settings…",
    menu_quit: "Quit Lowtide",
    menu_mute: "Mute low-battery alert",
    tray_muted: "Lowtide (alert muted)",
    notify_config_error: "Lowtide: config file has errors, using defaults",
    notify_hotkey_unavailable: "Lowtide: hotkey unavailable",
    hotkey_in_use: "{hotkey} is already used by another program",
    app_busy: "Lowtide is busy, try again.",
    autostart_failed: "Could not change autostart: {error}",
    save_failed: "Could not save the settings: {error}",
    start_failed: "Lowtide could not start:\n{error}",

    sample_device: "Sample mouse",
    simulated_device: "Simulated mouse",
    generic_logitech_device: "Logitech device",

    hotkey_two_keys: "more than one key in \"{hotkey}\"",
    hotkey_unknown_key: "unknown key \"{key}\"",
    hotkey_no_key: "no key in \"{hotkey}\"",
    hotkey_needs_modifier: "a hotkey needs Ctrl, Alt or Win",

    dlg_title: "Lowtide settings",
    dlg_general: "General",
    dlg_threshold: "Alert when charge drops below",
    dlg_suppress: "Don't alert while charging",
    dlg_poll: "Check battery every",
    dlg_seconds: "seconds",
    dlg_hotkey: "Status hotkey",
    dlg_autostart: "Start with Windows",
    dlg_language: "Language",
    language_auto: "Automatic (Windows language)",
    dlg_alert_group: "Low-battery alert",
    dlg_message: "Message",
    dlg_placeholders: "Placeholders: {device}  {percent}  {status}.  One line per low device.",
    dlg_position: "Position",
    dlg_margin: "Margin",
    dlg_font_alert: "F&ont...",
    dlg_font_info: "Fo&nt...",
    dlg_text_color: "Text color",
    dlg_opacity: "Opacity",
    dlg_background: "Background",
    dlg_padding: "Padding",
    dlg_radius: "Corner radius",
    dlg_fade: "Fade in as the charge drops",
    dlg_fade_down_to: "down to",
    dlg_fade_charge_unit: "% charge",
    dlg_fade_from: "Opacity from",
    dlg_fade_to: "to",
    dlg_info_group: "Status popup (hotkey)",
    dlg_offset_x: "Offset X",
    dlg_show_for: "Show for",
    dlg_preview: "&Preview",
    dlg_ok: "OK",
    dlg_cancel: "Cancel",
    dlg_apply: "&Apply",
    dlg_open_failed: "Could not open the settings dialog: {error}",
    anchor_top_left: "Top left",
    anchor_top: "Top",
    anchor_top_right: "Top right",
    anchor_left: "Left",
    anchor_center: "Center",
    anchor_right: "Right",
    anchor_bottom_left: "Bottom left",
    anchor_bottom: "Bottom",
    anchor_bottom_right: "Bottom right",
    font_label: "{font}, {size} pt, {weight}",
    font_italic: ", italic",
    weight_thin: "thin",
    weight_extra_light: "extra light",
    weight_light: "light",
    weight_regular: "regular",
    weight_medium: "medium",
    weight_semibold: "semibold",
    weight_bold: "bold",
    weight_extra_bold: "extra bold",
    weight_black: "black",
    err_range: "{field} must be a whole number from {min} to {max}.",
    err_hotkey: "Hotkey: {error}.",
    err_message_empty: "The alert message must not be empty.",
    err_fade_opacity: "The fade must end at least as opaque as it starts.",
    err_fade_charge: "The fade must end below the alert threshold ({threshold} %).",
    field_threshold: "Alert threshold",
    field_poll: "Check interval",
    field_alert_margin: "Alert margin",
    field_text_opacity: "Text opacity",
    field_bg_opacity: "Background opacity",
    field_padding: "Padding",
    field_radius: "Corner radius",
    field_info_margin: "Popup margin",
    field_offset_x: "Offset X",
    field_offset_y: "Offset Y",
    field_duration: "Popup duration",
    field_fade_start: "Fade start opacity",
    field_fade_end: "Fade end opacity",
    field_fade_charge: "Fade end charge",

    menu_statistics: "Statistics…",
    notify_history_failed: "Lowtide: battery history could not be saved",
    stats_title: "Lowtide statistics",
    stats_open_failed: "Could not open the statistics window: {error}",
    stats_close: "Close",
    stats_device: "Device",
    stats_current: "Now: {percent}, {status}",
    stats_last_reading: "Last change {date}: {percent}, {status}",
    stats_no_history: "No history yet. Lowtide records the charge while it runs.",
    stats_forecast_group: "Forecast",
    stats_lasts: "The current charge lasts about {days}, until {date}.",
    stats_lasts_charging: "On power: the forecast resumes when the device runs on battery again.",
    stats_alert_in: "The alert at {percent} comes around {date} (in {days}).",
    stats_alert_below: "Already below the alert threshold of {percent}.",
    stats_full_cycles: "A full charge lasts about {days} (median of {cycles} cycles).",
    stats_full_usage: "A full charge lasts about {days} at this usage.",
    stats_basis_week: "Based on {rate} per day over the last 7 days.",
    stats_basis_all: "Based on {rate} per day since {date}.",
    stats_not_enough: "Not enough data for a forecast yet.",
    stats_usage_group: "Usage per calendar day",
    stats_col_period: "Period",
    stats_col_used: "Used",
    stats_col_per_day: "Per day",
    stats_col_charges: "Charges",
    stats_col_covered: "Covered",
    stats_period_day: "Last 24 hours",
    stats_period_week: "Last 7 days",
    stats_period_month: "Last 30 days",
    stats_period_all: "All time",
    stats_charging_group: "Charging",
    stats_last_charge: "Last charged {date} ({ago} ago), from {from} to {to}.",
    stats_charging_since: "Charging since {date}, from {from}.",
    stats_no_charge: "No charge recorded yet.",
    stats_between: "Charged every {days} on average.",
    stats_charge_time: "Charging to full takes about {hours}.",
    stats_lowest: "Lowest charge in the last 30 days: {percent}.",
    stats_since: "Recording since {date}.",
    stats_chart_group: "Charge used per day, last 30 days",
    stats_chart_start: "30 days ago",
    stats_chart_today: "today",
    stats_chart_empty: "No use recorded in the last 30 days.",
    percent: "{n}%",
    decimal_separator: ".",
    duration_days: "{n} days",
    duration_one_day: "1 day",
    duration_short_hours: "{n} h",

    cli_source_count: "source {source}: {count} device(s)",
    cli_scan_summary: "{count} device(s), scan took {ms} ms",
    cli_watching: "watching (poll every {s} s), Ctrl+C to stop",
    cli_no_devices: "no devices",
    cli_device_line: "{device} via {connection}: {battery}",
    cli_estimated: " (est.)",
    cli_stale: " [not responding]",
};

pub static DE: Strings = Strings {
    charge_discharging: "entlädt",
    charge_charging: "lädt",
    charge_full: "vollständig geladen",
    charge_not_charging: "lädt nicht",
    charge_error: "Ladefehler",
    charge_unknown: "unbekannt",
    connection_wired: "kabelgebunden (USB-Kabel)",
    connection_receiver: "kabellos (Empfänger {pid}, Steckplatz {slot})",

    alert_default_message: "{device}: Akku fast leer ({percent} %)",
    info_no_devices: "Keine Geräte mit Akku gefunden",
    info_external_power: ", an externer Stromversorgung",
    info_no_battery: "keine Akkuinformationen",
    info_not_responding: "antwortet nicht",
    info_not_responding_since: "antwortet nicht, zuletzt gelesen vor {ago}",
    info_firmware: "Firmware {version}",
    duration_seconds: "{s} s",
    duration_minutes: "{m} Min.",
    duration_hours: "{h} Std. {m} Min.",

    tray_scanning: "Lowtide: Suche nach Geräten…",
    tray_no_devices: "Lowtide: keine Geräte gefunden",
    tray_device: "{device}: {percent} % ({status})",
    tray_not_responding: ", antwortet nicht",
    menu_status: "Akkustand anzeigen",
    menu_refresh: "Jetzt aktualisieren",
    menu_settings: "Einstellungen…",
    menu_quit: "Lowtide beenden",
    menu_mute: "Akkuwarnung stummschalten",
    tray_muted: "Lowtide (Warnung stummgeschaltet)",
    notify_config_error: "Lowtide: Konfigurationsdatei fehlerhaft, Standardwerte werden verwendet",
    notify_hotkey_unavailable: "Lowtide: Tastenkürzel nicht verfügbar",
    hotkey_in_use: "{hotkey} wird bereits von einem anderen Programm verwendet",
    app_busy: "Lowtide ist beschäftigt, bitte erneut versuchen.",
    autostart_failed: "Autostart konnte nicht geändert werden: {error}",
    save_failed: "Die Einstellungen konnten nicht gespeichert werden: {error}",
    start_failed: "Lowtide konnte nicht starten:\n{error}",

    sample_device: "Beispielmaus",
    simulated_device: "Simulierte Maus",
    generic_logitech_device: "Logitech-Gerät",

    hotkey_two_keys: "mehr als eine Taste in „{hotkey}“",
    hotkey_unknown_key: "unbekannte Taste „{key}“",
    hotkey_no_key: "keine Taste in „{hotkey}“",
    hotkey_needs_modifier: "ein Tastenkürzel braucht Strg, Alt oder Win",

    dlg_title: "Lowtide-Einstellungen",
    dlg_general: "Allgemein",
    dlg_threshold: "Warnen, wenn der Akku unter",
    dlg_suppress: "Beim Laden nicht warnen",
    dlg_poll: "Akku prüfen alle",
    dlg_seconds: "Sekunden",
    dlg_hotkey: "Tastenkürzel für Status",
    dlg_autostart: "Mit Windows starten",
    dlg_language: "Sprache",
    language_auto: "Automatisch (Windows-Sprache)",
    dlg_alert_group: "Warnung bei schwachem Akku",
    dlg_message: "Meldung",
    dlg_placeholders: "Platzhalter: {device}  {percent}  {status}.  Eine Zeile pro Gerät.",
    dlg_position: "Position",
    dlg_margin: "Abstand",
    dlg_font_alert: "&Schrift...",
    dlg_font_info: "Sch&rift...",
    dlg_text_color: "Textfarbe",
    dlg_opacity: "Deckkraft",
    dlg_background: "Hintergrund",
    dlg_padding: "Innenabstand",
    dlg_radius: "Eckenradius",
    dlg_fade: "Einblenden, je leerer der Akku wird",
    dlg_fade_down_to: "bis",
    dlg_fade_charge_unit: "% Ladung",
    dlg_fade_from: "Deckkraft von",
    dlg_fade_to: "bis",
    dlg_info_group: "Statusanzeige (Tastenkürzel)",
    dlg_offset_x: "Versatz X",
    dlg_show_for: "Anzeigen für",
    dlg_preview: "&Vorschau",
    dlg_ok: "OK",
    dlg_cancel: "Abbrechen",
    dlg_apply: "Ü&bernehmen",
    dlg_open_failed: "Das Einstellungsfenster konnte nicht geöffnet werden: {error}",
    anchor_top_left: "Oben links",
    anchor_top: "Oben",
    anchor_top_right: "Oben rechts",
    anchor_left: "Links",
    anchor_center: "Mitte",
    anchor_right: "Rechts",
    anchor_bottom_left: "Unten links",
    anchor_bottom: "Unten",
    anchor_bottom_right: "Unten rechts",
    font_label: "{font}, {size} pt, {weight}",
    font_italic: ", kursiv",
    weight_thin: "dünn",
    weight_extra_light: "extraleicht",
    weight_light: "leicht",
    weight_regular: "normal",
    weight_medium: "mittel",
    weight_semibold: "halbfett",
    weight_bold: "fett",
    weight_extra_bold: "extrafett",
    weight_black: "schwarz",
    err_range: "{field} muss eine ganze Zahl von {min} bis {max} sein.",
    err_hotkey: "Tastenkürzel: {error}.",
    err_message_empty: "Die Warnmeldung darf nicht leer sein.",
    err_fade_opacity: "Die Einblendung muss mindestens so deckend enden, wie sie beginnt.",
    err_fade_charge: "Die Einblendung muss unter der Warnschwelle ({threshold} %) enden.",
    field_threshold: "Warnschwelle",
    field_poll: "Prüfintervall",
    field_alert_margin: "Abstand der Warnung",
    field_text_opacity: "Textdeckkraft",
    field_bg_opacity: "Hintergrunddeckkraft",
    field_padding: "Innenabstand",
    field_radius: "Eckenradius",
    field_info_margin: "Abstand der Statusanzeige",
    field_offset_x: "Versatz X",
    field_offset_y: "Versatz Y",
    field_duration: "Anzeigedauer",
    field_fade_start: "Anfangsdeckkraft",
    field_fade_end: "Enddeckkraft",
    field_fade_charge: "Ladestand am Ende der Einblendung",

    menu_statistics: "Statistik…",
    notify_history_failed: "Lowtide: Akkuverlauf konnte nicht gespeichert werden",
    stats_title: "Lowtide-Statistik",
    stats_open_failed: "Das Statistikfenster konnte nicht geöffnet werden: {error}",
    stats_close: "Schließen",
    stats_device: "Gerät",
    stats_current: "Jetzt: {percent}, {status}",
    stats_last_reading: "Zuletzt geändert {date}: {percent}, {status}",
    stats_no_history: "Noch kein Verlauf. Lowtide zeichnet den Ladestand auf, solange es läuft.",
    stats_forecast_group: "Prognose",
    stats_lasts: "Die aktuelle Ladung reicht noch etwa {days}, bis {date}.",
    stats_lasts_charging: "Am Strom: Die Prognose geht weiter, sobald das Gerät wieder mit Akku läuft.",
    stats_alert_in: "Die Warnung bei {percent} kommt etwa am {date} (noch {days}).",
    stats_alert_below: "Schon unter der Warnschwelle von {percent}.",
    stats_full_cycles: "Eine volle Ladung reicht etwa {days} (Median aus {cycles} Zyklen).",
    stats_full_usage: "Eine volle Ladung reicht bei dieser Nutzung etwa {days}.",
    stats_basis_week: "Grundlage: {rate} pro Tag in den letzten 7 Tagen.",
    stats_basis_all: "Grundlage: {rate} pro Tag seit {date}.",
    stats_not_enough: "Noch nicht genug Daten für eine Prognose.",
    stats_usage_group: "Verbrauch pro Kalendertag",
    stats_col_period: "Zeitraum",
    stats_col_used: "Verbraucht",
    stats_col_per_day: "Pro Tag",
    stats_col_charges: "Ladungen",
    stats_col_covered: "Erfasst",
    stats_period_day: "Letzte 24 Stunden",
    stats_period_week: "Letzte 7 Tage",
    stats_period_month: "Letzte 30 Tage",
    stats_period_all: "Gesamt",
    stats_charging_group: "Laden",
    stats_last_charge: "Zuletzt geladen am {date} ({ago} her), von {from} auf {to}.",
    stats_charging_since: "Lädt seit {date}, ab {from}.",
    stats_no_charge: "Noch keine Ladung aufgezeichnet.",
    stats_between: "Im Schnitt alle {days} geladen.",
    stats_charge_time: "Volladen dauert etwa {hours}.",
    stats_lowest: "Niedrigster Ladestand der letzten 30 Tage: {percent}.",
    stats_since: "Aufzeichnung seit {date}.",
    stats_chart_group: "Verbrauch pro Tag, letzte 30 Tage",
    stats_chart_start: "vor 30 Tagen",
    stats_chart_today: "heute",
    stats_chart_empty: "In den letzten 30 Tagen kein Verbrauch aufgezeichnet.",
    percent: "{n} %",
    decimal_separator: ",",
    duration_days: "{n} Tage",
    duration_one_day: "1 Tag",
    duration_short_hours: "{n} h",

    cli_source_count: "Quelle {source}: {count} Gerät(e)",
    cli_scan_summary: "{count} Gerät(e), Suche dauerte {ms} ms",
    cli_watching: "beobachte (Abfrage alle {s} s), Strg+C zum Beenden",
    cli_no_devices: "keine Geräte",
    cli_device_line: "{device} über {connection}: {battery}",
    cli_estimated: " (geschätzt)",
    cli_stale: " [antwortet nicht]",
};

/// Schwäbisch, leaning towards the Upper Swabian spoken around Biberach an der
/// Riß (e.g. "it" for "nicht", "geit" for "gibt"). Understandable to any
/// German speaker.
pub static SWABIAN: Strings = Strings {
    charge_discharging: "wird leerer",
    charge_charging: "lädt grad",
    charge_full: "proppavoll",
    charge_not_charging: "lädt it",
    charge_error: "Ladefehler – so a Glomp",
    charge_unknown: "woiß mr it",
    connection_wired: "am Kabel (USB)",
    connection_receiver: "ohne Kabel (Empfänger {pid}, Plätzle {slot})",

    alert_default_message: "{device}: Dr Akku isch glei leer ({percent} %)",
    info_no_devices: "Koi Grät mit Akku gfonda",
    info_external_power: ", hängt am Strom",
    info_no_battery: "koi Ahnung vom Akku",
    info_not_responding: "geit koi Antwort",
    info_not_responding_since: "geit koi Antwort, zletscht glesa vor {ago}",
    info_firmware: "Firmware {version}",
    duration_seconds: "{s} Sekonda",
    duration_minutes: "{m} Minuta",
    duration_hours: "{h} Stond {m} Minuta",

    tray_scanning: "Lowtide: guckt nach Grät…",
    tray_no_devices: "Lowtide: koine Grät gfonda",
    tray_device: "{device}: {percent} % ({status})",
    tray_not_responding: ", geit koi Antwort",
    menu_status: "Akku azeiga",
    menu_refresh: "Glei nomol gucka",
    menu_settings: "Eistellunga…",
    menu_quit: "Lowtide: Feierobed",
    menu_mute: "S Meckra abstella",
    tray_muted: "Lowtide (Meckra abgstellt)",
    notify_config_error: "Lowtide: D Eistellungsdatei isch hee, mir nemmet dr Standard",
    notify_hotkey_unavailable: "Lowtide: Des Tastakürzel goht it",
    hotkey_in_use: "{hotkey} hot scho a anders Programm",
    app_busy: "Lowtide isch grad am Schaffa, probier's nomol.",
    autostart_failed: "Autostart hot it klappt: {error}",
    save_failed: "D Eistellunga ließet se it speichra: {error}",
    start_failed: "Lowtide kommt it en d Gäng:\n{error}",

    sample_device: "Beispielmäusle",
    simulated_device: "Simuliert's Mäusle",
    generic_logitech_device: "Logitech-Grät",

    hotkey_two_keys: "z'viel Tasta en „{hotkey}“",
    hotkey_unknown_key: "dia Tast „{key}“ kenn i it",
    hotkey_no_key: "koi Tast en „{hotkey}“",
    hotkey_needs_modifier: "a Tastakürzel braucht Strg, Alt oder Win",

    dlg_title: "Lowtide-Eistellunga",
    dlg_general: "Allgemoi",
    dlg_threshold: "Meckra, wenn dr Akku onder",
    dlg_suppress: "Beim Lada it meckra",
    dlg_poll: "Akku gucka älle",
    dlg_seconds: "Sekonda",
    dlg_hotkey: "Tastakürzel für Status",
    dlg_autostart: "Mit Windows starta",
    dlg_language: "Sproch",
    language_auto: "Automatisch (wia Windows)",
    dlg_alert_group: "Wenn dr Akku schlapp macht",
    dlg_message: "Meldong",
    dlg_placeholders: "Platzhalter: {device}  {percent}  {status}.  Pro Grät a Zeil.",
    dlg_position: "Platz",
    dlg_margin: "Abstand",
    dlg_font_alert: "&Schrift...",
    dlg_font_info: "Sch&rift...",
    dlg_text_color: "Textfarb",
    dlg_opacity: "Deckkraft",
    dlg_background: "Hentergrond",
    dlg_padding: "Innaabstand",
    dlg_radius: "Eckaradius",
    dlg_fade: "Langsam eiblenda, je leerer dr Akku",
    dlg_fade_down_to: "bis",
    dlg_fade_charge_unit: "% Ladong",
    dlg_fade_from: "Deckkraft vo",
    dlg_fade_to: "bis",
    dlg_info_group: "Statusazeig (Tastakürzel)",
    dlg_offset_x: "Versatz X",
    dlg_show_for: "Azeiga für",
    dlg_preview: "&Vorgucka",
    dlg_ok: "Passt",
    dlg_cancel: "Lass bleiba",
    dlg_apply: "Ü&bernemma",
    dlg_open_failed: "S Eistellungsfenschter goht it uf: {error}",
    anchor_top_left: "Oba links",
    anchor_top: "Oba",
    anchor_top_right: "Oba rechts",
    anchor_left: "Links",
    anchor_center: "En dr Mitt",
    anchor_right: "Rechts",
    anchor_bottom_left: "Donda links",
    anchor_bottom: "Donda",
    anchor_bottom_right: "Donda rechts",
    font_label: "{font}, {size} pt, {weight}",
    font_italic: ", schräg",
    weight_thin: "dürr",
    weight_extra_light: "arg leicht",
    weight_light: "leicht",
    weight_regular: "normal",
    weight_medium: "mittel",
    weight_semibold: "halbfett",
    weight_bold: "fett",
    weight_extra_bold: "arg fett",
    weight_black: "schwarz",
    err_range: "{field} muss a ganze Zahl vo {min} bis {max} sei.",
    err_hotkey: "Tastakürzel: {error}.",
    err_message_empty: "D Meldong derf it leer sei.",
    err_fade_opacity: "Am End muss es mindeschtens so deckend sei wie am Afang.",
    err_fade_charge: "S Eiblenda muss onder dr Meckergrenz ({threshold} %) aufhöra.",
    field_threshold: "D Meckergrenz",
    field_poll: "Dr Guckabstand",
    field_alert_margin: "Dr Abstand vom Meckra",
    field_text_opacity: "D Textdeckkraft",
    field_bg_opacity: "D Hentergronddeckkraft",
    field_padding: "Dr Innaabstand",
    field_radius: "Dr Eckaradius",
    field_info_margin: "Dr Abstand vo dr Azeig",
    field_offset_x: "Dr Versatz X",
    field_offset_y: "Dr Versatz Y",
    field_duration: "D Azeigdauer",
    field_fade_start: "D Afangsdeckkraft",
    field_fade_end: "D Enddeckkraft",
    field_fade_charge: "Dr Ladestand am End vom Eiblenda",

    menu_statistics: "Statistik…",
    notify_history_failed: "Lowtide: Dr Akkuverlauf ließ se it speichra",
    stats_title: "Lowtide-Statistik",
    stats_open_failed: "S Statistikfenschter goht it uf: {error}",
    stats_close: "Zuamacha",
    stats_device: "Grät",
    stats_current: "Jetzt: {percent}, {status}",
    stats_last_reading: "Zletscht gändert {date}: {percent}, {status}",
    stats_no_history: "No nix aufgschrieba. Lowtide schreibt dr Akku mit, solang's lauft.",
    stats_forecast_group: "Was no kommt",
    stats_lasts: "D Ladong langt no ebba {days}, bis {date}.",
    stats_lasts_charging: "Hängt am Strom: S Rechna goht weiter, wenn's wieder am Akku lauft.",
    stats_alert_in: "S Meckra bei {percent} kommt ebba am {date} (no {days}).",
    stats_alert_below: "Scho onder dr Meckergrenz vo {percent}.",
    stats_full_cycles: "A volle Ladong langt ebba {days} (Median aus {cycles} Durchgäng).",
    stats_full_usage: "A volle Ladong langt bei dem Verbrauch ebba {days}.",
    stats_basis_week: "Grondlag: {rate} am Dag en de letschte 7 Däg.",
    stats_basis_all: "Grondlag: {rate} am Dag seit {date}.",
    stats_not_enough: "No z'wenig Zahla zum Rechna.",
    stats_usage_group: "Verbrauch pro Kalenderdag",
    stats_col_period: "Zeitraum",
    stats_col_used: "Verbraucht",
    stats_col_per_day: "Am Dag",
    stats_col_charges: "Ladonga",
    stats_col_covered: "Erfasst",
    stats_period_day: "Letschte 24 Stond",
    stats_period_week: "Letschte 7 Däg",
    stats_period_month: "Letschte 30 Däg",
    stats_period_all: "Älles zsamma",
    stats_charging_group: "Lada",
    stats_last_charge: "Zletscht glada am {date} ({ago} her), vo {from} uf {to}.",
    stats_charging_since: "Lädt seit {date}, ab {from}.",
    stats_no_charge: "No nia glada, soviel mr woiß.",
    stats_between: "Em Schnitt älle {days} glada.",
    stats_charge_time: "Vollmacha dauert ebba {hours}.",
    stats_lowest: "Am leerschta en de letschte 30 Däg: {percent}.",
    stats_since: "Aufgschrieba seit {date}.",
    stats_chart_group: "Verbrauch am Dag, letschte 30 Däg",
    stats_chart_start: "vor 30 Däg",
    stats_chart_today: "heit",
    stats_chart_empty: "En de letschte 30 Däg nix verbraucht.",
    percent: "{n} %",
    decimal_separator: ",",
    duration_days: "{n} Däg",
    duration_one_day: "1 Dag",
    duration_short_hours: "{n} Stond",

    cli_source_count: "Quelle {source}: {count} Grät",
    cli_scan_summary: "{count} Grät, s Sucha hot {ms} ms dauert",
    cli_watching: "guckt zu (älle {s} s), Strg+C zum Aufhöra",
    cli_no_devices: "koine Grät",
    cli_device_line: "{device} über {connection}: {battery}",
    cli_estimated: " (gschätzt)",
    cli_stale: " [geit koi Antwort]",
};

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    fn placeholders(s: &str) -> BTreeSet<String> {
        let mut out = BTreeSet::new();
        let mut rest = s;
        while let Some(start) = rest.find('{') {
            let Some(len) = rest[start..].find('}') else { break };
            out.insert(rest[start + 1..start + len].to_string());
            rest = &rest[start + len + 1..];
        }
        out
    }

    #[test]
    fn every_language_uses_the_same_placeholders() {
        let en = EN.entries();
        for lang in [Language::German, Language::Swabian] {
            for ((key, en_text), (_, text)) in en.iter().zip(lang.strings().entries()) {
                assert_eq!(placeholders(en_text), placeholders(text), "{lang:?}.{key}: {text}");
            }
        }
    }

    #[test]
    fn no_translation_is_empty() {
        for lang in Language::ALL {
            for (key, text) in lang.strings().entries() {
                assert!(!text.trim().is_empty(), "{lang:?}.{key} is empty");
            }
        }
    }

    #[test]
    fn mnemonics_are_unique_per_dialog() {
        for lang in Language::ALL {
            let s = lang.strings();
            let keys: Vec<char> = [s.dlg_font_alert, s.dlg_font_info, s.dlg_preview, s.dlg_apply]
                .iter()
                .map(|t| t.split('&').nth(1).and_then(|r| r.chars().next()).unwrap().to_ascii_lowercase())
                .collect();
            let unique: BTreeSet<_> = keys.iter().collect();
            assert_eq!(unique.len(), keys.len(), "{lang:?}: {keys:?}");
        }
    }

    #[test]
    fn fill_replaces_named_placeholders() {
        let text = fill!(DE.err_range, field = "Warnschwelle", min = 0, max = 100);
        assert_eq!(text, "Warnschwelle muss eine ganze Zahl von 0 bis 100 sein.");
    }

    #[test]
    fn language_setting_names_in_config() {
        #[derive(Serialize, Deserialize)]
        struct W {
            l: LanguageSetting,
        }
        for (setting, name) in [
            (LanguageSetting::Auto, "auto"),
            (LanguageSetting::English, "en"),
            (LanguageSetting::German, "de"),
            (LanguageSetting::Swabian, "swabian"),
        ] {
            let text = toml::to_string(&W { l: setting }).unwrap();
            assert_eq!(text.trim(), format!("l = \"{name}\""));
            assert_eq!(toml::from_str::<W>(&text).unwrap().l, setting);
        }
    }
}
