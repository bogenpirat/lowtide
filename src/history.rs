//! Battery history: one CSV line per change of a device's charge or charging
//! state, appended to `%APPDATA%\lowtide\history.csv` and kept forever. The
//! statistics are computed from it (see `stats.rs`).
//!
//! Line format: `unix_secs,device,percent,state,estimated,name`, where `device`
//! is `source:key` of the [`DeviceId`] and `state` is `d` (discharging or
//! idle), `c` (charging or on external power) or `f` (full). Text fields are
//! percent-escaped so they never contain a comma or a line break. Lines
//! starting with `#` and lines that don't parse are skipped.

use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::config::APP_DIR;
use crate::model::{BatteryInfo, ChargeState, DeviceId, Snapshot};

const HEADER: &str = "# lowtide battery history v1: unix_secs,device,percent,state,estimated,name\n";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SampleState {
    /// Running on the battery (or idle without external power).
    Discharging,
    /// Charging, or plugged in without charging.
    Charging,
    Full,
}

impl SampleState {
    pub fn of(b: &BatteryInfo) -> SampleState {
        match b.charge {
            ChargeState::Full => SampleState::Full,
            ChargeState::Charging => SampleState::Charging,
            _ if b.external_power => SampleState::Charging,
            _ => SampleState::Discharging,
        }
    }

    pub fn on_power(self) -> bool {
        self != SampleState::Discharging
    }

    fn code(self) -> char {
        match self {
            SampleState::Discharging => 'd',
            SampleState::Charging => 'c',
            SampleState::Full => 'f',
        }
    }

    fn from_code(s: &str) -> Option<SampleState> {
        match s {
            "d" => Some(SampleState::Discharging),
            "c" => Some(SampleState::Charging),
            "f" => Some(SampleState::Full),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Sample {
    /// Unix time in seconds.
    pub at: i64,
    /// `source:key` of the device.
    pub device: String,
    pub name: String,
    pub percent: u8,
    pub state: SampleState,
    pub estimated: bool,
}

impl Sample {
    fn to_line(&self) -> String {
        format!(
            "{},{},{},{},{},{}\n",
            self.at,
            escape(&self.device),
            self.percent,
            self.state.code(),
            u8::from(self.estimated),
            escape(&self.name)
        )
    }

    fn parse(line: &str) -> Option<Sample> {
        let mut f = line.split(',');
        let sample = Sample {
            at: f.next()?.trim().parse().ok()?,
            device: unescape(f.next()?)?,
            percent: f.next()?.parse::<u8>().ok().filter(|&p| p <= 100)?,
            state: SampleState::from_code(f.next()?)?,
            estimated: match f.next()? {
                "0" => false,
                "1" => true,
                _ => return None,
            },
            name: unescape(f.next()?.trim_end())?,
        };
        (f.next().is_none() && !sample.device.is_empty()).then_some(sample)
    }
}

/// The key a device's samples are filed under.
pub fn device_key(id: &DeviceId) -> String {
    format!("{}:{}", id.source, id.key)
}

fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '%' => out.push_str("%25"),
            ',' => out.push_str("%2C"),
            '\n' => out.push_str("%0A"),
            '\r' => out.push_str("%0D"),
            c => out.push(c),
        }
    }
    out
}

fn unescape(s: &str) -> Option<String> {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('%') {
        out.push_str(&rest[..i]);
        let code = rest.get(i + 1..i + 3)?;
        out.push(match code {
            "25" => '%',
            "2C" => ',',
            "0A" => '\n',
            "0D" => '\r',
            _ => return None,
        });
        rest = &rest[i + 3..];
    }
    out.push_str(rest);
    Some(out)
}

pub struct History {
    /// None keeps the history in memory only (`--simulate`).
    path: Option<PathBuf>,
    samples: Vec<Sample>,
    /// Last recorded (percent, state) per device, to write changes only.
    last: HashMap<String, (u8, SampleState)>,
}

impl History {
    pub fn default_path() -> Option<PathBuf> {
        std::env::var_os("APPDATA").map(|d| PathBuf::from(d).join(APP_DIR).join("history.csv"))
    }

    /// The history at its default location; empty if there is none yet.
    pub fn load() -> History {
        match History::default_path() {
            Some(path) => History::open(path),
            None => History::in_memory(),
        }
    }

    pub fn open(path: PathBuf) -> History {
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        let mut history = History::in_memory();
        history.path = Some(path);
        for line in text.lines().filter(|l| !l.starts_with('#') && !l.trim().is_empty()) {
            if let Some(s) = Sample::parse(line) {
                history.last.insert(s.device.clone(), (s.percent, s.state));
                history.samples.push(s);
            }
        }
        history
    }

    pub fn in_memory() -> History {
        History { path: None, samples: Vec::new(), last: HashMap::new() }
    }

    pub fn samples(&self) -> &[Sample] {
        &self.samples
    }

    /// Note every responding device whose charge or charging state changed.
    /// The samples are kept in memory even if writing them fails.
    pub fn record(&mut self, snap: &Snapshot, now: SystemTime) -> std::io::Result<()> {
        let at = now.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64);
        let mut new = Vec::new();
        for d in snap.devices.iter().filter(|d| !d.stale) {
            let Some(b) = &d.battery else { continue };
            let device = device_key(&d.id);
            let state = SampleState::of(b);
            if self.last.get(&device) == Some(&(b.percent, state)) {
                continue;
            }
            self.last.insert(device.clone(), (b.percent, state));
            new.push(Sample { at, device, name: d.name.clone(), percent: b.percent, state, estimated: b.estimated });
        }
        if new.is_empty() {
            return Ok(());
        }
        let text: String = new.iter().map(Sample::to_line).collect();
        self.samples.extend(new);
        let Some(path) = &self.path else { return Ok(()) };
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let fresh = !path.exists();
        let mut file = OpenOptions::new().create(true).append(true).open(path)?;
        if fresh {
            file.write_all(HEADER.as_bytes())?;
        }
        file.write_all(text.as_bytes())
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::model::{Connection, DeviceState};

    fn device(key: &str, name: &str, percent: u8, charge: ChargeState) -> DeviceState {
        DeviceState {
            id: DeviceId { source: "test", key: key.into() },
            name: name.into(),
            kind: None,
            connection: Connection::Wired,
            battery: Some(BatteryInfo::new(percent, charge)),
            last_update: None,
            stale: false,
            details: vec![],
        }
    }

    fn snap(devices: Vec<DeviceState>) -> Snapshot {
        Snapshot { devices }
    }

    fn at(secs: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(secs)
    }

    fn temp_file(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("lowtide-test-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir.join("history.csv")
    }

    #[test]
    fn lines_round_trip_with_awkward_names() {
        let s = Sample {
            at: 1_700_000_000,
            device: "logitech:name:G903, 100% wireless".into(),
            name: "G903, 100%\nwireless".into(),
            percent: 42,
            state: SampleState::Charging,
            estimated: true,
        };
        let line = s.to_line();
        assert_eq!(line.matches(',').count(), 5);
        assert_eq!(Sample::parse(line.trim_end()), Some(s));
    }

    #[test]
    fn corrupt_lines_are_skipped() {
        for bad in [
            "",
            "x,dev,50,d,0,name",
            "1,dev,101,d,0,name",
            "1,dev,50,q,0,name",
            "1,dev,50,d,2,name",
            "1,dev,50,d,0,name,extra",
            "1,,50,d,0,name",
            "1,dev,50,d,0,bad%zz",
            "1,dev,50",
        ] {
            assert_eq!(Sample::parse(bad), None, "{bad:?}");
        }
        assert!(Sample::parse("1,dev,50,d,0,name").is_some());
    }

    #[test]
    fn only_changes_are_recorded_and_stale_devices_are_skipped() {
        let mut h = History::in_memory();
        let dis = ChargeState::Discharging;
        h.record(&snap(vec![device("a", "A", 80, dis)]), at(10)).unwrap();
        h.record(&snap(vec![device("a", "A", 80, dis)]), at(20)).unwrap();
        h.record(&snap(vec![device("a", "A", 80, ChargeState::Charging)]), at(30)).unwrap();
        let mut stale = device("a", "A", 70, dis);
        stale.stale = true;
        h.record(&snap(vec![stale]), at(40)).unwrap();
        h.record(&snap(vec![device("a", "A", 79, dis)]), at(50)).unwrap();
        let got: Vec<_> = h.samples().iter().map(|s| (s.at, s.percent, s.state)).collect();
        assert_eq!(
            got,
            [(10, 80, SampleState::Discharging), (30, 80, SampleState::Charging), (50, 79, SampleState::Discharging)]
        );
    }

    #[test]
    fn external_power_counts_as_charging() {
        let mut b = BatteryInfo::new(50, ChargeState::NotCharging);
        assert_eq!(SampleState::of(&b), SampleState::Discharging);
        b.external_power = true;
        assert_eq!(SampleState::of(&b), SampleState::Charging);
    }

    #[test]
    fn file_survives_a_restart_without_duplicate_lines() {
        let path = temp_file("restart");
        let mut h = History::open(path.clone());
        h.record(&snap(vec![device("a", "Mouse, left", 80, ChargeState::Discharging)]), at(10)).unwrap();
        h.record(&snap(vec![device("a", "Mouse, left", 79, ChargeState::Discharging)]), at(20)).unwrap();

        let mut again = History::open(path.clone());
        assert_eq!(again.samples(), h.samples());
        // Same level after the restart: nothing new.
        again.record(&snap(vec![device("a", "Mouse, left", 79, ChargeState::Discharging)]), at(30)).unwrap();
        assert_eq!(again.samples().len(), 2);
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.starts_with('#'));
        assert_eq!(text.lines().count(), 3);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }
}
