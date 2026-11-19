//! Statistics over the battery history: usage per calendar day in several
//! intervals, a forecast of how long the current charge lasts, how long a full
//! charge typically lasts, and charging facts. Pure computation: no UI, no
//! clock, no time zone (the caller passes `now` and a local-day function).
//!
//! The history is split into *discharge segments* separated by *charges*. A
//! charge is a time on external power, or a jump of at least
//! [`RECHARGE_JUMP`] points while discharging (charged elsewhere, or a battery
//! swap). Usage counts only new lows within a segment, so noise of an
//! estimated (voltage-based) reading that bounces up and down is not counted
//! twice. Rates are per calendar day, over the time spent on battery.

use std::collections::BTreeMap;

use crate::history::{Sample, SampleState};

pub const DAY: i64 = 86_400;
/// A rise of this many points while discharging means the device was charged
/// (or its battery swapped) while we were not watching.
pub const RECHARGE_JUMP: u8 = 5;
/// Less coverage than this (in days) gives no per-day rate.
const MIN_COVERAGE_DAYS: f64 = 0.5;
/// The forecast uses the last 7 days once they hold at least this many days of data.
const WEEK_BASIS_MIN_DAYS: f64 = 3.0;
/// A discharge segment counts as a full cycle if it starts at least this high...
const CYCLE_MIN_START: u8 = 90;
/// ...and uses at least this many points before the next charge.
const CYCLE_MIN_USED: u8 = 20;
/// A charge counts as complete at this level (or when the device reports full).
const CHARGE_COMPLETE: u8 = 95;
pub const CHART_DAYS: usize = 30;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Interval {
    Day,
    Week,
    Month,
    All,
}

impl Interval {
    pub const ALL: [Interval; 4] = [Interval::Day, Interval::Week, Interval::Month, Interval::All];

    fn seconds(self) -> Option<i64> {
        match self {
            Interval::Day => Some(DAY),
            Interval::Week => Some(7 * DAY),
            Interval::Month => Some(30 * DAY),
            Interval::All => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct IntervalStats {
    pub interval: Interval,
    /// Charge used, in percentage points.
    pub used: u32,
    /// Points per calendar day on battery; None with too little data.
    pub per_day: Option<f64>,
    pub charges: usize,
    /// Days of data on battery within the interval.
    pub days: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Forecast {
    /// The interval whose rate the forecast uses.
    pub basis: Interval,
    pub per_day: f64,
    /// Days until empty; None while on external power.
    pub days_left: Option<f64>,
    /// Days until the alert threshold; None while on power or already below it.
    pub days_to_alert: Option<f64>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum FullChargeSource {
    /// Median of this many observed discharge cycles.
    Cycles(usize),
    /// Extrapolated from the recent usage rate.
    Usage,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FullCharge {
    pub days: f64,
    pub source: FullChargeSource,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Charge {
    pub start: i64,
    /// When the device went back on battery; None while still on power.
    pub end: Option<i64>,
    pub from: u8,
    pub to: u8,
    /// When it reached [`CHARGE_COMPLETE`] or reported full.
    pub full_at: Option<i64>,
    /// False for a charge inferred from a jump: its timing is unknown.
    pub observed: bool,
}

impl Charge {
    /// Plugged in without gaining anything (e.g. a wired mouse already full) is no charge.
    fn is_real(&self) -> bool {
        self.to > self.from || (self.full_at.is_some() && self.from < CHARGE_COMPLETE)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct DeviceStats {
    pub percent: u8,
    pub state: SampleState,
    pub estimated: bool,
    /// Time of the newest sample.
    pub last_sample: i64,
    pub tracked_since: i64,
    /// One entry per [`Interval::ALL`], in that order.
    pub intervals: Vec<IntervalStats>,
    pub forecast: Option<Forecast>,
    pub full_charge: Option<FullCharge>,
    pub last_charge: Option<Charge>,
    pub days_between_charges: Option<f64>,
    /// Typical time from plugging in to full, in hours.
    pub charge_hours: Option<f64>,
    pub lowest_30d: u8,
    /// Points used per local day; the last entry is today.
    pub daily: [u32; CHART_DAYS],
}

#[derive(Clone, Debug, PartialEq)]
pub struct DeviceReport {
    /// `source:key`, as in the history.
    pub device: String,
    pub name: String,
    pub stats: DeviceStats,
}

/// Statistics for every device in the history, sorted by name.
pub fn reports(samples: &[Sample], now: i64, threshold: u8, local_day: &dyn Fn(i64) -> i64) -> Vec<DeviceReport> {
    let mut by_device: BTreeMap<&str, Vec<&Sample>> = BTreeMap::new();
    for s in samples {
        by_device.entry(&s.device).or_default().push(s);
    }
    let mut out: Vec<DeviceReport> = by_device
        .into_iter()
        .filter_map(|(device, samples)| {
            let stats = analyze(&samples, now, threshold, local_day)?;
            let name = samples.iter().max_by_key(|s| s.at).map(|s| s.name.clone()).unwrap_or_default();
            Some(DeviceReport { device: device.to_string(), name, stats })
        })
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.device.cmp(&b.device)));
    out
}

/// A discharge segment that ended in a charge.
struct Cycle {
    start: i64,
    end: i64,
    from: u8,
    low: u8,
}

#[derive(Default)]
struct Timeline {
    /// (time, points) of every new low.
    drops: Vec<(i64, u8)>,
    /// Every period on power, real charge or not, plus inferred charges.
    charges: Vec<Charge>,
    cycles: Vec<Cycle>,
}

/// Running discharge segment: start time, starting level, lowest level so far.
#[derive(Clone, Copy)]
struct Segment {
    start: i64,
    from: u8,
    low: u8,
}

impl Segment {
    fn new(s: &Sample) -> Segment {
        Segment { start: s.at, from: s.percent, low: s.percent }
    }
}

fn timeline(points: &[&Sample]) -> Timeline {
    let mut t = Timeline::default();
    let mut seg: Option<Segment> = None;
    // A charge in progress, with the segment it interrupted.
    let mut charge: Option<(Charge, Option<Segment>)> = None;
    for p in points {
        if p.state.on_power() {
            let (c, _) = charge.get_or_insert_with(|| {
                let from = seg.map_or(p.percent, |s| s.low.min(p.percent));
                let start = Charge { start: p.at, end: None, from, to: p.percent, full_at: None, observed: true };
                (start, seg.take())
            });
            c.to = c.to.max(p.percent);
            if c.full_at.is_none() && (p.state == SampleState::Full || p.percent >= CHARGE_COMPLETE) {
                c.full_at = Some(p.at);
            }
            continue;
        }
        if let Some((mut c, interrupted)) = charge.take() {
            c.end = Some(p.at);
            let real = c.is_real();
            if real {
                if let Some(s) = interrupted {
                    t.cycles.push(Cycle { start: s.start, end: c.start, from: s.from, low: s.low });
                }
                seg = Some(Segment::new(p));
            } else {
                // Plugged in without charging: the segment simply goes on.
                seg = interrupted;
            }
            t.charges.push(c);
            if real {
                continue;
            }
        }
        match &mut seg {
            Some(s) if p.percent >= s.low.saturating_add(RECHARGE_JUMP) => {
                t.cycles.push(Cycle { start: s.start, end: p.at, from: s.from, low: s.low });
                t.charges.push(Charge {
                    start: p.at,
                    end: Some(p.at),
                    from: s.low,
                    to: p.percent,
                    full_at: (p.percent >= CHARGE_COMPLETE).then_some(p.at),
                    observed: false,
                });
                *s = Segment::new(p);
            }
            Some(s) => {
                if p.percent < s.low {
                    t.drops.push((p.at, s.low - p.percent));
                    s.low = p.percent;
                }
            }
            None => seg = Some(Segment::new(p)),
        }
    }
    if let Some((c, interrupted)) = charge {
        if c.is_real()
            && let Some(s) = interrupted
        {
            t.cycles.push(Cycle { start: s.start, end: c.start, from: s.from, low: s.low });
        }
        t.charges.push(c);
    }
    t
}

fn median(mut values: Vec<f64>) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    values.sort_by(f64::total_cmp);
    let mid = values.len() / 2;
    Some(if values.len() % 2 == 0 { (values[mid - 1] + values[mid]) / 2.0 } else { values[mid] })
}

/// Statistics of one device. `samples` may be in any order; None if empty.
pub fn analyze(samples: &[&Sample], now: i64, threshold: u8, local_day: &dyn Fn(i64) -> i64) -> Option<DeviceStats> {
    let mut points = samples.to_vec();
    points.sort_by_key(|s| s.at);
    let first = *points.first()?;
    let last = *points.last()?;
    let t = timeline(&points);

    // Seconds on power within [lo, now].
    let on_power = |lo: i64| -> i64 {
        t.charges
            .iter()
            .map(|c| {
                let (a, b) = (c.start.max(lo), c.end.unwrap_or(now).min(now));
                (b - a).max(0)
            })
            .sum()
    };
    let real_charges: Vec<&Charge> = t.charges.iter().filter(|c| c.is_real()).collect();

    let intervals: Vec<IntervalStats> = Interval::ALL
        .iter()
        .map(|&interval| {
            let lo = interval.seconds().map_or(first.at, |s| (now - s).max(first.at));
            let used: u32 = t.drops.iter().filter(|(at, _)| *at > lo).map(|(_, d)| u32::from(*d)).sum();
            let days = ((now - lo - on_power(lo)).max(0) as f64) / DAY as f64;
            IntervalStats {
                interval,
                used,
                per_day: (days >= MIN_COVERAGE_DAYS).then(|| f64::from(used) / days),
                charges: real_charges.iter().filter(|c| c.start > lo).count(),
                days,
            }
        })
        .collect();
    let rate_of = |i: Interval| intervals.iter().find(|s| s.interval == i).and_then(|s| s.per_day).filter(|&r| r > 0.0);

    let on_battery = !last.state.on_power();
    let week = &intervals[1];
    let basis = if week.days >= WEEK_BASIS_MIN_DAYS && rate_of(Interval::Week).is_some() {
        Some(Interval::Week)
    } else {
        rate_of(Interval::All).map(|_| Interval::All)
    };
    let forecast = basis.and_then(|basis| {
        let per_day = rate_of(basis)?;
        Some(Forecast {
            basis,
            per_day,
            days_left: on_battery.then(|| f64::from(last.percent) / per_day),
            days_to_alert: (on_battery && last.percent > threshold)
                .then(|| f64::from(last.percent - threshold) / per_day),
        })
    });

    let cycle_days: Vec<f64> = t
        .cycles
        .iter()
        .filter(|c| c.from >= CYCLE_MIN_START && c.from - c.low >= CYCLE_MIN_USED)
        .map(|c| (c.end - c.start) as f64 / DAY as f64 * 100.0 / f64::from(c.from - c.low))
        .collect();
    let cycles = cycle_days.len();
    let full_charge = match median(cycle_days) {
        Some(days) => Some(FullCharge { days, source: FullChargeSource::Cycles(cycles) }),
        None => rate_of(Interval::Month)
            .or_else(|| rate_of(Interval::All))
            .map(|r| FullCharge { days: 100.0 / r, source: FullChargeSource::Usage }),
    };

    let days_between_charges = match (real_charges.first(), real_charges.last()) {
        (Some(a), Some(b)) if real_charges.len() >= 2 => {
            Some((b.start - a.start) as f64 / DAY as f64 / (real_charges.len() - 1) as f64)
        }
        _ => None,
    };
    let charge_hours = median(
        real_charges
            .iter()
            .filter(|c| c.observed)
            .filter_map(|c| c.full_at.map(|f| (f - c.start) as f64 / 3600.0))
            .collect(),
    );

    let month_start = now - 30 * DAY;
    let carried_in = points.iter().rev().find(|s| s.at <= month_start).map(|s| s.percent);
    let lowest_30d = points
        .iter()
        .filter(|s| s.at > month_start)
        .map(|s| s.percent)
        .chain(carried_in)
        .min()
        .unwrap_or(last.percent);

    let mut daily = [0u32; CHART_DAYS];
    let today = local_day(now);
    for &(at, d) in &t.drops {
        let ago = today - local_day(at);
        if (0..CHART_DAYS as i64).contains(&ago) {
            daily[CHART_DAYS - 1 - ago as usize] += u32::from(d);
        }
    }

    Some(DeviceStats {
        percent: last.percent,
        state: last.state,
        estimated: last.estimated,
        last_sample: last.at,
        tracked_since: first.at,
        intervals,
        forecast,
        full_charge,
        last_charge: real_charges.last().map(|c| (*c).clone()),
        days_between_charges,
        charge_hours,
        lowest_30d,
        daily,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOUR: i64 = 3600;

    fn sample(at: i64, percent: u8, state: SampleState) -> Sample {
        Sample { at, device: "t:1".into(), name: "Mouse".into(), percent, state, estimated: false }
    }

    fn dis(at: i64, percent: u8) -> Sample {
        sample(at, percent, SampleState::Discharging)
    }

    /// One point per sample from `from` down to `to`, evenly spread over [start, end].
    fn drain(start: i64, end: i64, from: u8, to: u8) -> Vec<Sample> {
        let steps = i64::from(from - to);
        (0..=steps).map(|i| dis(start + (end - start) * i / steps, from - i as u8)).collect()
    }

    fn utc_day(t: i64) -> i64 {
        t.div_euclid(DAY)
    }

    fn run(samples: &[Sample], now: i64) -> DeviceStats {
        let refs: Vec<&Sample> = samples.iter().collect();
        analyze(&refs, now, 15, &utc_day).unwrap()
    }

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 0.01
    }

    #[test]
    fn steady_drain_gives_rate_and_forecast() {
        // 100 -> 50 over 10 days: 5 points per day.
        let st = run(&drain(0, 10 * DAY, 100, 50), 10 * DAY);
        let week = &st.intervals[1];
        assert!(close(week.days, 7.0));
        assert!(close(week.per_day.unwrap(), 5.0), "{week:?}");
        assert_eq!(st.intervals[3].used, 50);
        let f = st.forecast.unwrap();
        assert_eq!(f.basis, Interval::Week);
        assert!(close(f.days_left.unwrap(), 10.0));
        assert!(close(f.days_to_alert.unwrap(), 7.0));
        // No completed cycle yet: a full charge is extrapolated from usage.
        let full = st.full_charge.unwrap();
        assert_eq!(full.source, FullChargeSource::Usage);
        assert!(close(full.days, 20.0));
        assert_eq!(st.tracked_since, 0);
        assert_eq!(st.lowest_30d, 50);
    }

    #[test]
    fn jitter_does_not_inflate_usage() {
        // Voltage noise: every other reading 2 points higher than the trend.
        let samples: Vec<Sample> = (0..=40)
            .map(|i| dis(i * HOUR * 6, (90 - i + if i % 2 == 1 { 2 } else { 0 }) as u8))
            .collect();
        let st = run(&samples, 10 * DAY);
        assert_eq!(st.intervals[3].used, 40);
        assert_eq!(st.intervals[3].charges, 0);
    }

    #[test]
    fn charging_splits_cycles_and_is_not_counted_as_time_on_battery() {
        let mut s = drain(0, 8 * DAY, 100, 60);
        s.push(sample(8 * DAY, 60, SampleState::Charging));
        s.push(sample(8 * DAY + HOUR, 80, SampleState::Charging));
        s.push(sample(8 * DAY + 2 * HOUR, 100, SampleState::Full));
        s.extend(drain(8 * DAY + DAY / 2, 10 * DAY + DAY / 2, 100, 90));
        let st = run(&s, 10 * DAY + DAY / 2);

        let all = &st.intervals[3];
        assert_eq!(all.used, 50);
        assert_eq!(all.charges, 1);
        assert!(close(all.days, 10.0), "{all:?}");
        assert!(close(all.per_day.unwrap(), 5.0));
        let c = st.last_charge.unwrap();
        assert_eq!((c.start, c.from, c.to, c.end), (8 * DAY, 60, 100, Some(8 * DAY + DAY / 2)));
        assert!(close(st.charge_hours.unwrap(), 2.0));
        // 100 -> 60 in 8 days is one cycle: 100 points would last 20 days.
        let full = st.full_charge.unwrap();
        assert_eq!(full.source, FullChargeSource::Cycles(1));
        assert!(close(full.days, 20.0));
    }

    #[test]
    fn forecast_pauses_while_charging() {
        let mut s = drain(0, 4 * DAY, 100, 80);
        s.push(sample(4 * DAY, 80, SampleState::Charging));
        let st = run(&s, 4 * DAY + HOUR);
        let f = st.forecast.unwrap();
        assert_eq!((f.days_left, f.days_to_alert), (None, None));
    }

    #[test]
    fn a_jump_while_discharging_is_a_charge_made_elsewhere() {
        let mut s = drain(0, 2 * DAY, 50, 40);
        s.extend(drain(3 * DAY, 5 * DAY, 80, 70));
        let st = run(&s, 5 * DAY);
        let all = &st.intervals[3];
        assert_eq!(all.used, 20);
        assert_eq!(all.charges, 1);
        let c = st.last_charge.unwrap();
        assert_eq!((c.from, c.to, c.observed), (40, 80, false));
        assert_eq!(st.charge_hours, None, "an unseen charge has no duration");
        // A rise below the jump is noise, not a charge.
        let st = run(&[dis(0, 50), dis(DAY, 46), dis(2 * DAY, 50)], 2 * DAY);
        assert_eq!(st.intervals[3].charges, 0);
    }

    #[test]
    fn plugging_in_without_gaining_charge_is_no_charge() {
        let mut s = drain(0, 2 * DAY, 100, 90);
        s.push(sample(2 * DAY, 90, SampleState::Charging));
        s.extend(drain(2 * DAY + HOUR, 4 * DAY, 90, 80));
        let st = run(&s, 4 * DAY);
        assert_eq!(st.intervals[3].charges, 0);
        assert_eq!(st.intervals[3].used, 20);
        assert_eq!(st.last_charge, None);
    }

    #[test]
    fn too_little_data_gives_no_rate() {
        let st = run(&drain(0, 10 * HOUR, 100, 95), 10 * HOUR);
        assert_eq!(st.intervals[3].per_day, None);
        assert_eq!(st.forecast, None);
        assert_eq!(st.full_charge, None);
        // A single reading still reports the charge.
        let st = run(&[dis(0, 77)], 0);
        assert_eq!((st.percent, st.lowest_30d), (77, 77));
    }

    #[test]
    fn short_history_uses_the_all_time_rate() {
        let st = run(&drain(0, 2 * DAY, 100, 90), 2 * DAY);
        let f = st.forecast.unwrap();
        assert_eq!(f.basis, Interval::All);
        assert!(close(f.per_day, 5.0));
    }

    #[test]
    fn recent_week_outweighs_older_usage() {
        // 2 points/day for 5 days, then 6 points/day for 5 days.
        let mut s = drain(0, 5 * DAY, 100, 90);
        s.extend(drain(5 * DAY + 1, 10 * DAY, 89, 60));
        let st = run(&s, 10 * DAY);
        let f = st.forecast.unwrap();
        assert_eq!(f.basis, Interval::Week);
        // Last 7 days: 4 points from days 3-5, 30 from days 5-10.
        assert!(close(f.per_day, 34.0 / 7.0), "{f:?}");
    }

    #[test]
    fn full_charge_is_the_median_of_cycles() {
        let mut s = Vec::new();
        let mut t = 0;
        for days in [10, 20, 30] {
            s.extend(drain(t, t + days * DAY, 100, 50));
            t += days * DAY;
            s.push(sample(t, 50, SampleState::Charging));
            s.push(sample(t + HOUR, 100, SampleState::Full));
            t += 2 * HOUR;
        }
        let st = run(&s, t);
        let full = st.full_charge.unwrap();
        assert_eq!(full.source, FullChargeSource::Cycles(3));
        assert!(close(full.days, 40.0), "{full:?}");
        assert!(close(st.days_between_charges.unwrap(), (25 * DAY + 2 * HOUR) as f64 / DAY as f64));
        assert_eq!(st.intervals[3].charges, 3);
    }

    #[test]
    fn daily_buckets_follow_the_local_day() {
        let now = 10 * DAY + 12 * HOUR;
        let s = [dis(8 * DAY, 90), dis(9 * DAY + 23 * HOUR, 85), dis(10 * DAY + HOUR, 80)];
        let refs: Vec<&Sample> = s.iter().collect();
        let utc = analyze(&refs, now, 15, &utc_day).unwrap();
        assert_eq!(utc.daily[CHART_DAYS - 2..], [5, 5]);
        // Two hours ahead of UTC, 23:00 is already the next day.
        let plus2 = |t: i64| (t + 2 * HOUR).div_euclid(DAY);
        let local = analyze(&refs, now, 15, &plus2).unwrap();
        assert_eq!(local.daily[CHART_DAYS - 2..], [0, 10]);
    }

    #[test]
    fn lowest_charge_includes_the_level_carried_into_the_month() {
        // Window starts at day 15.
        let st = run(&[dis(0, 30), dis(40 * DAY, 25), dis(41 * DAY, 90)], 45 * DAY);
        assert_eq!(st.lowest_30d, 25);
        // Still at 20 % when the window opened, charged later.
        let st = run(&[dis(0, 20), dis(20 * DAY, 90)], 45 * DAY);
        assert_eq!(st.lowest_30d, 20);
        // Charged before the window: the old low no longer counts.
        let st = run(&[dis(0, 10), dis(10 * DAY, 90)], 45 * DAY);
        assert_eq!(st.lowest_30d, 90);
    }

    #[test]
    fn reports_group_by_device_and_use_the_latest_name() {
        let mut a = dis(0, 50);
        a.name = "Old name".into();
        let mut b = dis(DAY, 49);
        b.name = "Zeta".into();
        let mut c = dis(0, 80);
        c.device = "t:2".into();
        c.name = "Alpha".into();
        let r = reports(&[a, c, b], DAY, 15, &utc_day);
        let names: Vec<_> = r.iter().map(|r| (r.device.as_str(), r.name.as_str())).collect();
        assert_eq!(names, [("t:2", "Alpha"), ("t:1", "Zeta")]);
    }
}
