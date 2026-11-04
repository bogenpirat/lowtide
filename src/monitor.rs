//! Background thread that owns all battery sources. It sleeps in a single
//! `WaitForMultipleObjects` on the sources' input handles plus a wake event,
//! so it costs nothing between events and scheduled polls.

use std::sync::mpsc::{channel, Receiver, Sender};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use windows::core::PCWSTR;
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::System::Threading::{CreateEventW, SetEvent, WaitForMultipleObjects};

use crate::model::Snapshot;
use crate::sources::BatterySource;

#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    /// Hardware was plugged or unplugged.
    Rescan,
    /// Read all devices now (e.g. the user asked for status).
    RefreshNow,
    SetPollInterval(Duration),
    Quit,
}

/// Lower bound between automatic rescans after a device vanished, so a
/// flapping device can never turn the loop into a busy spin.
const MIN_RESCAN_GAP: Duration = Duration::from_secs(2);
/// WaitForMultipleObjects limit.
const MAX_HANDLES: usize = 64;

struct WakeEvent(HANDLE);
// An event handle may be signaled from any thread.
unsafe impl Send for WakeEvent {}
unsafe impl Sync for WakeEvent {}

pub struct Monitor {
    tx: Sender<Command>,
    wake: std::sync::Arc<WakeEvent>,
    thread: Option<JoinHandle<()>>,
}

impl Monitor {
    pub fn spawn(
        sources: Vec<Box<dyn BatterySource>>,
        poll_interval: Duration,
        publish: impl Fn(Snapshot) + Send + 'static,
    ) -> Monitor {
        let (tx, rx) = channel();
        let event = unsafe { CreateEventW(None, false, false, PCWSTR::null()) }
            .expect("CreateEventW failed");
        let wake = std::sync::Arc::new(WakeEvent(event));
        let thread_wake = wake.clone();
        let thread = std::thread::Builder::new()
            .name("lowtide-monitor".into())
            .spawn(move || {
                let mut worker = Worker { sources, poll_interval, rx, wake: thread_wake, last: None };
                worker.run(&publish);
            })
            .expect("failed to start monitor thread");
        Monitor { tx, wake, thread: Some(thread) }
    }

    pub fn send(&self, cmd: Command) {
        if self.tx.send(cmd).is_ok() {
            unsafe {
                let _ = SetEvent(self.wake.0);
            }
        }
    }
}

impl Drop for Monitor {
    fn drop(&mut self) {
        self.send(Command::Quit);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        unsafe {
            let _ = CloseHandle(self.wake.0);
        }
    }
}

struct Worker {
    sources: Vec<Box<dyn BatterySource>>,
    poll_interval: Duration,
    rx: Receiver<Command>,
    wake: std::sync::Arc<WakeEvent>,
    last: Option<Snapshot>,
}

impl Worker {
    fn run(&mut self, publish: &dyn Fn(Snapshot)) {
        for s in &mut self.sources {
            s.rescan();
        }
        let mut last_rescan = Instant::now();
        let mut next_poll = Instant::now() + self.poll_interval;
        self.publish_if_changed(publish);

        loop {
            let now = Instant::now();
            let mut deadline = next_poll;
            let auto_rescan = self.sources.iter().any(|s| s.needs_rescan());
            if auto_rescan {
                deadline = deadline.min(last_rescan + MIN_RESCAN_GAP);
            }
            let mut handles = vec![self.wake.0];
            for s in &mut self.sources {
                handles.extend(s.wait_handles());
            }
            handles.truncate(MAX_HANDLES);
            let ms = u32::try_from(deadline.saturating_duration_since(now).as_millis())
                .unwrap_or(u32::MAX - 1);
            unsafe { WaitForMultipleObjects(&handles, false, ms) };

            let mut rescan = false;
            let mut refresh = false;
            for cmd in self.rx.try_iter() {
                match cmd {
                    Command::Quit => return,
                    Command::Rescan => rescan = true,
                    Command::RefreshNow => refresh = true,
                    Command::SetPollInterval(d) => {
                        self.poll_interval = d;
                        next_poll = Instant::now() + d;
                    }
                }
            }

            let mut changed = false;
            for s in &mut self.sources {
                changed |= s.process_pending();
            }

            let now = Instant::now();
            let auto_due = now >= last_rescan + MIN_RESCAN_GAP;
            if rescan || (auto_due && self.sources.iter().any(|s| s.needs_rescan())) {
                for s in &mut self.sources {
                    if rescan || s.needs_rescan() {
                        s.rescan();
                    }
                }
                last_rescan = now;
                changed = true;
            }
            if refresh || now >= next_poll {
                for s in &mut self.sources {
                    s.poll();
                }
                next_poll = Instant::now() + self.poll_interval;
                changed = true;
            }
            if changed {
                self.publish_if_changed(publish);
            }
        }
    }

    fn publish_if_changed(&mut self, publish: &dyn Fn(Snapshot)) {
        let snap = Snapshot::from_devices(self.sources.iter().flat_map(|s| s.devices()));
        if self.last.as_ref() != Some(&snap) {
            self.last = Some(snap.clone());
            publish(snap);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{mpsc, Arc};

    use super::*;
    use crate::model::{BatteryInfo, ChargeState, Connection, DeviceId, DeviceState};

    struct FakeSource {
        polls: Arc<AtomicUsize>,
        rescans: Arc<AtomicUsize>,
    }

    impl BatterySource for FakeSource {
        fn name(&self) -> &'static str {
            "fake"
        }
        fn rescan(&mut self) {
            self.rescans.fetch_add(1, Ordering::SeqCst);
        }
        fn poll(&mut self) {
            self.polls.fetch_add(1, Ordering::SeqCst);
        }
        fn wait_handles(&mut self) -> Vec<HANDLE> {
            vec![]
        }
        fn process_pending(&mut self) -> bool {
            false
        }
        fn needs_rescan(&self) -> bool {
            false
        }
        fn devices(&self) -> Vec<DeviceState> {
            let polls = self.polls.load(Ordering::SeqCst);
            vec![DeviceState {
                id: DeviceId { source: "fake", key: "1".into() },
                name: "Fake".into(),
                kind: None,
                connection: Connection::Wired,
                battery: Some(BatteryInfo::new(100 - polls as u8, ChargeState::Discharging)),
                last_update: None,
                stale: false,
                details: vec![],
            }]
        }
    }

    #[test]
    fn publishes_initial_state_and_reacts_to_commands() {
        let polls = Arc::new(AtomicUsize::new(0));
        let rescans = Arc::new(AtomicUsize::new(0));
        let src = FakeSource { polls: polls.clone(), rescans: rescans.clone() };
        let (tx, rx) = mpsc::channel();
        let monitor = Monitor::spawn(vec![Box::new(src)], Duration::from_secs(3600), move |s| {
            let _ = tx.send(s);
        });
        let percent = |s: Snapshot| s.devices[0].battery.as_ref().unwrap().percent;
        let t = Duration::from_secs(5);

        assert_eq!(percent(rx.recv_timeout(t).unwrap()), 100);
        assert_eq!(rescans.load(Ordering::SeqCst), 1);

        monitor.send(Command::RefreshNow);
        assert_eq!(percent(rx.recv_timeout(t).unwrap()), 99);

        monitor.send(Command::Rescan);
        monitor.send(Command::RefreshNow);
        assert_eq!(percent(rx.recv_timeout(t).unwrap()), 98);
        assert_eq!(rescans.load(Ordering::SeqCst), 2);

        drop(monitor); // must join promptly
    }

    #[test]
    fn polls_on_schedule() {
        let polls = Arc::new(AtomicUsize::new(0));
        let src = FakeSource { polls: polls.clone(), rescans: Arc::default() };
        let (tx, rx) = mpsc::channel();
        let _monitor = Monitor::spawn(vec![Box::new(src)], Duration::from_millis(50), move |s| {
            let _ = tx.send(s);
        });
        for _ in 0..3 {
            rx.recv_timeout(Duration::from_secs(5)).unwrap();
        }
        assert!(polls.load(Ordering::SeqCst) >= 2);
    }
}
