use std::path::Path;
use std::time::Instant;

use chrono::{DateTime, Local};

pub type IntervalId = u64;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WindowInfo {
    pub window_id: u32,
    pub title: Option<String>,
    pub wm_instance: Option<String>,
    pub wm_class: Option<String>,
    pub pid: Option<u32>,
    pub executable: Option<String>,
}

impl WindowInfo {
    pub fn app_id(&self) -> String {
        self.wm_class
            .clone()
            .or_else(|| {
                self.executable.as_deref().and_then(|path| {
                    Path::new(path)
                        .file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                })
            })
            .or_else(|| self.wm_instance.clone())
            .unwrap_or_else(|| "<unknown>".to_owned())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActivityState {
    Active,
    Idle,
}

impl ActivityState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Idle => "idle",
        }
    }
}

pub struct OpenInterval {
    pub id: IntervalId,
    pub state: ActivityState,
    pub window: Option<WindowInfo>,
    pub started_wall: DateTime<Local>,
    pub started_mono: Instant,
}

#[derive(Debug)]
pub enum ActivityEvent {
    IntervalStarted {
        interval_id: IntervalId,
        state: ActivityState,
        window: Option<WindowInfo>,
        started_at: DateTime<Local>,
        reason: String,
    },
    IntervalFinished {
        interval_id: IntervalId,
        state: ActivityState,
        window: Option<WindowInfo>,
        started_at: DateTime<Local>,
        ended_at: DateTime<Local>,
        duration: std::time::Duration,
        reason: String,
    },
    WindowMetadataChanged {
        interval_id: IntervalId,
        window: WindowInfo,
        observed_at: DateTime<Local>,
    },
    Status(String),
}

impl OpenInterval {
    pub fn started_event(&self, reason: impl Into<String>) -> ActivityEvent {
        ActivityEvent::IntervalStarted {
            interval_id: self.id,
            state: self.state,
            window: self.window.clone(),
            started_at: self.started_wall,
            reason: reason.into(),
        }
    }

    pub fn finish_event(self, reason: impl Into<String>) -> ActivityEvent {
        ActivityEvent::IntervalFinished {
            interval_id: self.id,
            state: self.state,
            window: self.window,
            started_at: self.started_wall,
            ended_at: Local::now(),
            duration: self.started_mono.elapsed(),
            reason: reason.into(),
        }
    }
}

pub fn same_window(left: &Option<WindowInfo>, right: &Option<WindowInfo>) -> bool {
    match (left, right) {
        (Some(left), Some(right)) => left.window_id == right.window_id,
        (None, None) => true,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_id_uses_the_specified_fallback_order() {
        let mut info = WindowInfo {
            window_id: 1,
            title: None,
            wm_instance: Some("instance".into()),
            wm_class: Some("class".into()),
            pid: None,
            executable: Some("/usr/bin/example".into()),
        };
        assert_eq!(info.app_id(), "class");
        info.wm_class = None;
        assert_eq!(info.app_id(), "example");
        info.executable = None;
        assert_eq!(info.app_id(), "instance");
        info.wm_instance = None;
        assert_eq!(info.app_id(), "<unknown>");
    }

    #[test]
    fn interval_events_snapshot_start_and_finish_metadata() {
        let started_at = Local::now();
        let interval = OpenInterval {
            id: 42,
            state: ActivityState::Active,
            window: None,
            started_wall: started_at,
            started_mono: Instant::now(),
        };

        assert!(matches!(
            interval.started_event("startup"),
            ActivityEvent::IntervalStarted {
                interval_id: 42,
                state: ActivityState::Active,
                window: None,
                started_at: event_started_at,
                reason,
            } if event_started_at == started_at && reason == "startup"
        ));
        assert!(matches!(
            interval.finish_event("shutdown"),
            ActivityEvent::IntervalFinished {
                interval_id: 42,
                state: ActivityState::Active,
                window: None,
                started_at: event_started_at,
                reason,
                ..
            } if event_started_at == started_at && reason == "shutdown"
        ));
    }
}
