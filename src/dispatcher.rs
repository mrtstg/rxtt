use std::sync::mpsc::{Receiver, SyncSender, sync_channel};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};

use crate::Result;
use crate::model::ActivityEvent;
use crate::output::render_event;
use crate::storage::Storage;

const EVENT_QUEUE_CAPACITY: usize = 1_024;

#[derive(Clone, Default)]
pub struct DispatcherFailure(Arc<Mutex<Option<String>>>);

impl DispatcherFailure {
    pub fn error(&self) -> Option<String> {
        self.0
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone()
    }

    fn record(&self, error: &str) {
        let mut slot = self
            .0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if slot.is_none() {
            *slot = Some(error.to_owned());
        }
    }
}

pub struct ActivityDispatcher {
    sender: SyncSender<ActivityEvent>,
    failure: DispatcherFailure,
    worker: JoinHandle<std::result::Result<(), String>>,
}

pub fn spawn_activity_dispatcher(storage: Storage) -> ActivityDispatcher {
    let (sender, receiver) = sync_channel(EVENT_QUEUE_CAPACITY);
    let failure = DispatcherFailure::default();
    let worker_failure = failure.clone();
    let worker = thread::spawn(move || {
        let mut storage = storage;
        let result = dispatch_events(receiver, &mut storage).map_err(|error| error.to_string());
        if let Err(error) = &result {
            worker_failure.record(error);
        }
        result
    });
    ActivityDispatcher {
        sender,
        failure,
        worker,
    }
}

impl ActivityDispatcher {
    pub fn sender(&self) -> SyncSender<ActivityEvent> {
        self.sender.clone()
    }

    pub fn failure(&self) -> DispatcherFailure {
        self.failure.clone()
    }

    pub fn join(self) -> Result<()> {
        let Self {
            sender,
            failure: _,
            worker,
        } = self;
        drop(sender);
        match worker.join() {
            Ok(Ok(())) => Ok(()),
            Ok(Err(error)) => Err(error.into()),
            Err(_) => Err("activity event dispatcher panicked".into()),
        }
    }
}

fn dispatch_events(receiver: Receiver<ActivityEvent>, storage: &mut Storage) -> Result<()> {
    for event in receiver {
        if !matches!(event, ActivityEvent::Status(_)) {
            storage.record(&event)?;
        }
        render_event(&event);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::env;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::mpsc::channel;
    use std::time::{Duration, Instant};

    use chrono::Local;
    use rusqlite::Connection;

    use super::*;
    use crate::model::{ActivityState, WindowInfo};

    static NEXT_TEST_ID: AtomicU64 = AtomicU64::new(0);

    fn window() -> WindowInfo {
        WindowInfo {
            window_id: 1,
            title: Some("title".to_owned()),
            wm_instance: None,
            wm_class: Some("Example".to_owned()),
            pid: None,
            executable: None,
        }
    }

    #[test]
    fn sqlite_write_failure_is_reported_to_the_tracker() -> Result<()> {
        let path = env::temp_dir().join(format!(
            "rxtt-dispatcher-test-{}-{}-{}.sqlite3",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_nanos(),
            NEXT_TEST_ID.fetch_add(1, Ordering::Relaxed)
        ));
        let storage = Storage::open(&path)?;
        let connection = Connection::open(&path)?;
        connection.execute_batch(
            "CREATE TRIGGER reject_interval_write BEFORE INSERT ON activity_interval
             BEGIN SELECT RAISE(ABORT, 'intentional write failure'); END;",
        )?;

        let dispatcher = spawn_activity_dispatcher(storage);
        let sender = dispatcher.sender();
        let failure = dispatcher.failure();
        let started_at = Local::now();
        sender.send(ActivityEvent::IntervalStarted {
            interval_id: 1,
            state: ActivityState::Active,
            window: Some(window()),
            started_at,
            reason: "start".to_owned(),
        })?;
        sender.send(ActivityEvent::IntervalFinished {
            interval_id: 1,
            state: ActivityState::Active,
            window: Some(window()),
            started_at,
            ended_at: Local::now(),
            duration: Duration::from_secs(1),
            reason: "finish".to_owned(),
        })?;
        drop(sender);

        assert!(dispatcher.join().is_err());
        let deadline = Instant::now() + Duration::from_secs(1);
        while failure.error().is_none() && Instant::now() < deadline {
            std::thread::yield_now();
        }
        assert!(failure.error().is_some());
        Ok(())
    }

    #[test]
    fn join_closes_the_dispatcher_sender() -> Result<()> {
        let dispatcher =
            spawn_activity_dispatcher(Storage::open(std::path::Path::new(":memory:"))?);
        let tracker_sender = dispatcher.sender();
        drop(tracker_sender);

        let (completed, receiver) = channel();
        std::thread::spawn(move || completed.send(dispatcher.join().is_ok()).unwrap());
        assert!(receiver.recv_timeout(Duration::from_secs(1))?);
        Ok(())
    }
}
