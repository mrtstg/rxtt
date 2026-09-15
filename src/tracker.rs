use crate::presentation::escape_terminal;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::SyncSender;
use std::time::{Duration, Instant};

use chrono::Local;

use crate::Result;
use crate::dispatcher::DispatcherFailure;
use crate::model::{
    ActivityEvent, ActivityState, IntervalId, OpenInterval, WindowInfo, same_window,
};
use crate::x11::{X11Event, X11Source};

#[derive(Debug, Clone, Copy)]
pub struct TrackerConfig {
    pub idle_threshold: Duration,
    pub sample_interval: Duration,
    pub title_change_min_interval: Duration,
}

pub struct Tracker {
    source: X11Source,
    config: TrackerConfig,
    current_window: Option<WindowInfo>,
    interval: Option<OpenInterval>,
    events: SyncSender<ActivityEvent>,
    dispatcher_failure: DispatcherFailure,
    next_interval_id: IntervalId,
    title_updates: TitleUpdateThrottle,
}

impl Tracker {
    pub fn new(
        source: X11Source,
        config: TrackerConfig,
        events: SyncSender<ActivityEvent>,
        dispatcher_failure: DispatcherFailure,
    ) -> Self {
        Self {
            source,
            config,
            current_window: None,
            interval: None,
            events,
            dispatcher_failure,
            next_interval_id: 1,
            title_updates: TitleUpdateThrottle::new(config.title_change_min_interval),
        }
    }

    pub fn run(&mut self, shutdown_requested: &AtomicBool) -> Result<()> {
        self.ensure_dispatcher_running()?;
        self.source.subscribe_to_active_window_changes()?;
        self.current_window = self.source.read_active_window();
        self.subscribe_to_current_window_title_changes();
        self.reconcile_current_state("startup")?;
        self.emit(ActivityEvent::Status(format!(
            "Watching _NET_ACTIVE_WINDOW; idle threshold={}s; safety polling={}s. Press Ctrl+C to stop.",
            self.config.idle_threshold.as_secs_f64(),
            self.config.sample_interval.as_secs_f64(),
        )))?;

        while !shutdown_requested.load(Ordering::SeqCst) {
            self.ensure_dispatcher_running()?;
            self.handle_x11_events(self.source.drain_events()?)?;
            self.source.wait_for_events(self.config.sample_interval)?;
            if shutdown_requested.load(Ordering::SeqCst) {
                break;
            }
            self.handle_x11_events(self.source.drain_events()?)?;
            self.refresh_active_window("safety poll", false)?;
            self.flush_pending_title_update()?;
            self.reconcile_current_state("periodic sample")?;
        }

        self.finish_interval("shutdown")?;
        Ok(())
    }

    pub fn finish_interval(&mut self, reason: &str) -> Result<()> {
        if let Some(interval) = self.interval.take() {
            self.emit(interval.finish_event(reason))?;
        }
        Ok(())
    }

    fn reconcile(
        &mut self,
        state: ActivityState,
        window: Option<WindowInfo>,
        reason: &str,
    ) -> Result<()> {
        if let Some(interval) = &self.interval {
            let same = interval.state == state
                && (state == ActivityState::Idle || same_window(&interval.window, &window));
            if same {
                return Ok(());
            }
            self.finish_interval(reason)?;
        }

        let interval = OpenInterval {
            id: self.next_interval_id,
            state,
            window,
            started_wall: Local::now(),
            started_mono: Instant::now(),
        };
        self.next_interval_id = self
            .next_interval_id
            .checked_add(1)
            .ok_or("interval identifier space exhausted")?;
        self.emit(interval.started_event(reason))?;
        self.interval = Some(interval);
        Ok(())
    }

    fn reconcile_current_state(&mut self, reason: &str) -> Result<()> {
        let (state, reason) = match self.source.idle_duration() {
            Some(idle) if idle >= self.config.idle_threshold => (
                ActivityState::Idle,
                format!("{reason}; idle={:.1}s", idle.as_secs_f64()),
            ),
            _ => (ActivityState::Active, reason.to_owned()),
        };
        self.reconcile(state, self.current_window.clone(), &reason)
    }

    fn handle_x11_events(&mut self, events: Vec<X11Event>) -> Result<()> {
        for event in events {
            match event {
                X11Event::ActiveWindowChanged => {
                    self.refresh_active_window("_NET_ACTIVE_WINDOW changed", true)?
                }
                X11Event::WindowTitleChanged { window_id } => {
                    self.refresh_window_metadata(window_id)?
                }
            }
        }
        Ok(())
    }

    fn refresh_active_window(&mut self, reason: &str, ignore_transient_none: bool) -> Result<()> {
        let new_window = self.source.read_active_window();
        if ignore_transient_none && new_window.is_none() {
            return Ok(());
        }
        if same_window(&self.current_window, &new_window) {
            return Ok(());
        }
        self.current_window = new_window;
        self.title_updates.reset();
        self.subscribe_to_current_window_title_changes();

        // Idle reconciliation intentionally keeps the currently open interval,
        // even when the remembered focused window changes.
        self.reconcile_current_state(reason)
    }

    fn refresh_window_metadata(&mut self, window_id: u32) -> Result<()> {
        let Some(current_window) = self.current_window.as_ref() else {
            return Ok(());
        };
        if current_window.window_id != window_id {
            return Ok(());
        }
        let updated_window = self.source.read_window_info(window_id);
        if current_window == &updated_window {
            return Ok(());
        }

        let Some(interval_id) = self.interval.as_ref().map(|interval| interval.id) else {
            return Ok(());
        };
        self.current_window = Some(updated_window.clone());
        if let Some(update) = self
            .title_updates
            .push(interval_id, updated_window, Instant::now())
        {
            self.emit_title_update(update.interval_id, update.window)?;
        }
        Ok(())
    }

    fn subscribe_to_current_window_title_changes(&self) {
        let Some(window) = self.current_window.as_ref() else {
            return;
        };
        if let Err(error) = self
            .source
            .subscribe_to_window_title_changes(window.window_id)
        {
            let error = escape_terminal(&error.to_string());
            eprintln!("WARNING: cannot watch active window title changes: {error}");
        }
    }

    fn flush_pending_title_update(&mut self) -> Result<()> {
        if let Some(window) = self.title_updates.flush(Instant::now()) {
            self.emit_title_update(window.interval_id, window.window)?;
        }
        Ok(())
    }

    fn emit_title_update(&self, interval_id: IntervalId, window: WindowInfo) -> Result<()> {
        self.emit(ActivityEvent::WindowMetadataChanged {
            interval_id,
            window,
            observed_at: Local::now(),
        })
    }

    fn emit(&self, event: ActivityEvent) -> Result<()> {
        self.ensure_dispatcher_running()?;
        self.events
            .send(event)
            .map_err(|_| "activity event dispatcher stopped unexpectedly")?;
        self.ensure_dispatcher_running()
    }

    fn ensure_dispatcher_running(&self) -> Result<()> {
        if let Some(error) = self.dispatcher_failure.error() {
            return Err(format!("activity event dispatcher failed: {error}").into());
        }
        Ok(())
    }
}

struct TitleUpdateThrottle {
    minimum_interval: Duration,
    last_emitted_at: Option<Instant>,
    pending: Option<PendingTitleUpdate>,
}

struct PendingTitleUpdate {
    interval_id: IntervalId,
    window: WindowInfo,
}

impl TitleUpdateThrottle {
    fn new(minimum_interval: Duration) -> Self {
        Self {
            minimum_interval,
            last_emitted_at: None,
            pending: None,
        }
    }

    fn reset(&mut self) {
        self.last_emitted_at = None;
        self.pending = None;
    }

    fn push(
        &mut self,
        interval_id: u64,
        window: WindowInfo,
        now: Instant,
    ) -> Option<PendingTitleUpdate> {
        if self
            .last_emitted_at
            .is_none_or(|last_emitted| now.duration_since(last_emitted) >= self.minimum_interval)
        {
            self.last_emitted_at = Some(now);
            return Some(PendingTitleUpdate {
                interval_id,
                window,
            });
        }
        self.pending = Some(PendingTitleUpdate {
            interval_id,
            window,
        });
        None
    }

    fn flush(&mut self, now: Instant) -> Option<PendingTitleUpdate> {
        if self
            .last_emitted_at
            .is_some_and(|last_emitted| now.duration_since(last_emitted) < self.minimum_interval)
        {
            return None;
        }
        let pending = self.pending.take()?;
        self.last_emitted_at = Some(now);
        Some(pending)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window(title: &str) -> WindowInfo {
        WindowInfo {
            window_id: 1,
            title: Some(title.into()),
            wm_instance: None,
            wm_class: None,
            pid: None,
            executable: None,
        }
    }

    #[test]
    fn title_updates_are_rate_limited_and_latest_value_is_flushed() {
        let start = Instant::now();
        let mut throttle = TitleUpdateThrottle::new(Duration::from_secs(5));

        assert_eq!(
            throttle
                .push(1, window("one"), start)
                .map(|update| update.window),
            Some(window("one"))
        );
        assert!(
            throttle
                .push(1, window("two"), start + Duration::from_secs(1))
                .is_none()
        );
        assert!(
            throttle
                .push(1, window("three"), start + Duration::from_secs(2))
                .is_none()
        );
        assert!(throttle.flush(start + Duration::from_secs(4)).is_none());
        assert_eq!(
            throttle
                .flush(start + Duration::from_secs(5))
                .map(|update| update.window),
            Some(window("three"))
        );
    }
}
