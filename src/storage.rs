use std::collections::HashMap;
use std::env;
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::{DateTime, Local};
use rusqlite::{Connection, OpenFlags, params};

use crate::Result;
use crate::model::{ActivityEvent, ActivityState, IntervalId, WindowInfo};

const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

pub fn default_database_path() -> Result<PathBuf> {
    state_database_path(
        env::var_os("XDG_STATE_HOME").as_deref(),
        env::var_os("HOME").as_deref(),
    )
}

fn state_database_path(xdg_state_home: Option<&OsStr>, home: Option<&OsStr>) -> Result<PathBuf> {
    let state_home = match xdg_state_home.filter(|value| !value.is_empty()) {
        Some(path) => PathBuf::from(path),
        None => PathBuf::from(home.ok_or("cannot determine state directory: HOME is unset")?)
            .join(".local/state"),
    };
    Ok(state_home.join("rxtt/activity.sqlite3"))
}

pub struct Storage {
    connection: Connection,
    pending: HashMap<IntervalId, PendingInterval>,
}

struct PendingInterval {
    state: ActivityState,
    window: Option<WindowInfo>,
    started_at: DateTime<Local>,
    start_reason: String,
    metadata: Vec<PendingMetadata>,
}

struct PendingMetadata {
    window: WindowInfo,
    observed_at: DateTime<Local>,
}

impl Storage {
    pub fn open(path: &Path) -> Result<Self> {
        Ok(Self {
            connection: open_database(path)?,
            pending: HashMap::new(),
        })
    }

    pub fn record(&mut self, event: &ActivityEvent) -> Result<()> {
        match event {
            ActivityEvent::IntervalStarted {
                interval_id,
                state,
                window,
                started_at,
                reason,
            } => {
                let replaced = self.pending.insert(
                    *interval_id,
                    PendingInterval {
                        state: *state,
                        window: window.clone(),
                        started_at: *started_at,
                        start_reason: reason.clone(),
                        metadata: Vec::new(),
                    },
                );
                if replaced.is_some() {
                    return Err(
                        format!("duplicate interval start for local id {interval_id}").into(),
                    );
                }
            }
            ActivityEvent::WindowMetadataChanged {
                interval_id,
                window,
                observed_at,
            } => {
                let pending = self.pending.get_mut(interval_id).ok_or_else(|| {
                    format!("metadata references unknown local interval id {interval_id}")
                })?;
                pending.metadata.push(PendingMetadata {
                    window: window.clone(),
                    observed_at: *observed_at,
                });
            }
            ActivityEvent::IntervalFinished {
                interval_id,
                state,
                window,
                started_at,
                ended_at,
                duration: _,
                reason,
            } => {
                let pending = self.pending.remove(interval_id).ok_or_else(|| {
                    format!("finish references unknown local interval id {interval_id}")
                })?;
                if pending.state != *state
                    || pending.window != *window
                    || pending.started_at != *started_at
                {
                    return Err(format!(
                        "finish does not match start for local interval id {interval_id}"
                    )
                    .into());
                }
                self.persist_completed_interval(*interval_id, pending, *ended_at, reason.clone())?;
            }
            ActivityEvent::Status(_) => {}
        }
        Ok(())
    }

    fn persist_completed_interval(
        &mut self,
        tracker_interval_id: IntervalId,
        pending: PendingInterval,
        ended_at: DateTime<Local>,
        end_reason: String,
    ) -> Result<()> {
        let tracker_interval_id = i64::try_from(tracker_interval_id)
            .map_err(|_| "local interval identifier exceeds SQLite integer range")?;
        let transaction = self.connection.transaction()?;
        let window = pending.window.as_ref();
        let app_id = window.map_or_else(|| "<unknown>".to_owned(), WindowInfo::app_id);
        transaction.execute(
            "INSERT INTO activity_interval (
                tracker_interval_id, state, started_at, ended_at,
                start_reason, end_reason, initial_window_id, initial_title,
                initial_wm_instance, initial_wm_class, initial_pid, initial_executable, app_id
             ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            params![
                tracker_interval_id,
                pending.state.as_str(),
                pending.started_at.timestamp(),
                ended_at.timestamp(),
                pending.start_reason,
                end_reason,
                window.map(|window| i64::from(window.window_id)),
                window.and_then(|window| window.title.as_deref()),
                window.and_then(|window| window.wm_instance.as_deref()),
                window.and_then(|window| window.wm_class.as_deref()),
                window.and_then(|window| window.pid.map(i64::from)),
                window.and_then(|window| window.executable.as_deref()),
                app_id,
            ],
        )?;
        let interval_id = transaction.last_insert_rowid();

        for metadata in pending.metadata {
            let window = metadata.window;
            let app_id = window.app_id();
            transaction.execute(
                "INSERT INTO window_metadata_change (
                    interval_id, observed_at, window_id, title, wm_instance, wm_class, pid,
                    executable, app_id
                 ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
                params![
                    interval_id,
                    metadata.observed_at.timestamp(),
                    i64::from(window.window_id),
                    window.title,
                    window.wm_instance,
                    window.wm_class,
                    window.pid.map(i64::from),
                    window.executable,
                    app_id,
                ],
            )?;
        }
        transaction.commit()?;
        Ok(())
    }
}

pub fn open_database(path: &Path) -> Result<Connection> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)?;
    }

    let mut connection = Connection::open(path)?;
    connection.busy_timeout(BUSY_TIMEOUT)?;
    connection.execute_batch("PRAGMA foreign_keys = ON; PRAGMA journal_mode = WAL;")?;
    initialize_schema(&mut connection)?;
    Ok(connection)
}

pub fn open_database_read_only(path: &Path) -> Result<Connection> {
    let connection =
        Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).map_err(|error| {
            format!(
                "cannot open existing activity database {}: {error}",
                path.display()
            )
        })?;
    connection.busy_timeout(BUSY_TIMEOUT)?;
    let version: i64 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if version != 1 {
        return Err(format!("unsupported or uninitialized database schema version {version}; reporting requires version 1").into());
    }
    Ok(connection)
}

fn initialize_schema(connection: &mut Connection) -> Result<()> {
    let version: i64 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
    match version {
        0 => connection.execute_batch(&format!(
            "BEGIN IMMEDIATE; {SCHEMA_V1} PRAGMA user_version = 1; COMMIT;"
        ))?,
        1 => {}
        _ => {
            return Err(format!("unsupported database schema version {version}").into());
        }
    }
    Ok(())
}

const SCHEMA_V1: &str = "
    CREATE TABLE activity_interval (
        id INTEGER PRIMARY KEY,
        tracker_interval_id INTEGER NOT NULL,
        state TEXT NOT NULL CHECK (state IN ('active', 'idle')),
        started_at INTEGER NOT NULL,
        ended_at INTEGER NOT NULL,
        start_reason TEXT NOT NULL,
        end_reason TEXT NOT NULL,
        initial_window_id INTEGER,
        initial_title TEXT,
        initial_wm_instance TEXT,
        initial_wm_class TEXT,
        initial_pid INTEGER,
        initial_executable TEXT,
        app_id TEXT NOT NULL
    );
    CREATE TABLE window_metadata_change (
        id INTEGER PRIMARY KEY,
        interval_id INTEGER NOT NULL REFERENCES activity_interval(id) ON DELETE CASCADE,
        observed_at INTEGER NOT NULL,
        window_id INTEGER NOT NULL,
        title TEXT,
        wm_instance TEXT,
        wm_class TEXT,
        pid INTEGER,
        executable TEXT,
        app_id TEXT NOT NULL
    );
    CREATE INDEX activity_interval_active_started_app_idx
        ON activity_interval(started_at, app_id) WHERE state = 'active';
    CREATE INDEX window_metadata_change_interval_observed_idx
        ON window_metadata_change(interval_id, observed_at);
    CREATE VIEW activity_title_segment AS
    WITH title_points AS (
        SELECT id AS interval_id, app_id, initial_title AS title,
               started_at AS segment_start, ended_at AS interval_end, 0 AS point_order
        FROM activity_interval WHERE state = 'active'
        UNION ALL
        SELECT m.interval_id, m.app_id, m.title, m.observed_at, i.ended_at, m.id
        FROM window_metadata_change AS m
        JOIN activity_interval AS i ON i.id = m.interval_id
        WHERE i.state = 'active'
    ), segments AS (
        SELECT interval_id, app_id, title, segment_start,
               LEAD(segment_start, 1, interval_end) OVER (
                   PARTITION BY interval_id ORDER BY segment_start, point_order
               ) AS segment_end
        FROM title_points
    )
    SELECT interval_id, app_id, title, segment_start, segment_end,
           segment_end - segment_start AS duration_seconds
    FROM segments;
";

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use chrono::{DateTime, Duration as ChronoDuration, FixedOffset, TimeZone};

    use super::*;

    static NEXT_TEST_ID: AtomicU64 = AtomicU64::new(0);

    fn window(title: &str) -> WindowInfo {
        WindowInfo {
            window_id: 99,
            title: Some(title.to_owned()),
            wm_instance: Some("example".to_owned()),
            wm_class: Some("Example".to_owned()),
            pid: Some(123),
            executable: Some("/usr/bin/example".to_owned()),
        }
    }

    fn timestamp(seconds: i64) -> DateTime<Local> {
        FixedOffset::east_opt(0)
            .unwrap()
            .with_ymd_and_hms(2025, 1, 1, 0, 0, 0)
            .unwrap()
            .checked_add_signed(ChronoDuration::seconds(seconds))
            .unwrap()
            .with_timezone(&Local)
    }

    fn start(interval_id: IntervalId, started_at: DateTime<Local>, title: &str) -> ActivityEvent {
        ActivityEvent::IntervalStarted {
            interval_id,
            state: ActivityState::Active,
            window: Some(window(title)),
            started_at,
            reason: "start".to_owned(),
        }
    }

    fn finish(
        interval_id: IntervalId,
        started_at: DateTime<Local>,
        ended_at: DateTime<Local>,
        title: &str,
    ) -> ActivityEvent {
        ActivityEvent::IntervalFinished {
            interval_id,
            state: ActivityState::Active,
            window: Some(window(title)),
            started_at,
            ended_at,
            duration: Duration::from_secs(20),
            reason: "finish".to_owned(),
        }
    }

    #[test]
    fn state_path_prefers_xdg_state_home_and_creates_database() -> Result<()> {
        assert_eq!(
            state_database_path(Some(OsStr::new("/state")), Some(OsStr::new("/home/a")))?,
            PathBuf::from("/state/rxtt/activity.sqlite3")
        );
        assert_eq!(
            state_database_path(None, Some(OsStr::new("/home/a")))?,
            PathBuf::from("/home/a/.local/state/rxtt/activity.sqlite3")
        );

        let path = env::temp_dir()
            .join(format!(
                "rxtt-storage-test-{}-{}",
                std::process::id(),
                NEXT_TEST_ID.fetch_add(1, Ordering::Relaxed)
            ))
            .join("nested/activity.sqlite3");
        let storage = Storage::open(&path)?;
        assert!(path.exists());
        let version: i64 = storage
            .connection
            .pragma_query_value(None, "user_version", |row| row.get(0))?;
        assert_eq!(version, 1);
        Ok(())
    }

    #[test]
    fn unsupported_schema_versions_are_rejected() -> Result<()> {
        let path = env::temp_dir().join(format!(
            "rxtt-storage-version-test-{}-{}.sqlite3",
            std::process::id(),
            NEXT_TEST_ID.fetch_add(1, Ordering::Relaxed)
        ));
        let connection = Connection::open(&path)?;
        connection.pragma_update(None, "user_version", 2)?;
        drop(connection);

        let error = match Storage::open(&path) {
            Ok(_) => panic!("expected version-2 database to be rejected"),
            Err(error) => error,
        };
        assert!(
            error
                .to_string()
                .contains("unsupported database schema version 2")
        );
        Ok(())
    }

    #[test]
    fn completed_interval_and_metadata_are_persisted_in_order() -> Result<()> {
        let mut storage = Storage::open(Path::new(":memory:"))?;
        let started_at = timestamp(0);
        storage.record(&start(7, started_at, "first"))?;
        storage.record(&ActivityEvent::WindowMetadataChanged {
            interval_id: 7,
            window: window("second"),
            observed_at: timestamp(5),
        })?;
        storage.record(&ActivityEvent::WindowMetadataChanged {
            interval_id: 7,
            window: window("third"),
            observed_at: timestamp(10),
        })?;
        storage.record(&finish(7, started_at, timestamp(20), "first"))?;

        assert_eq!(
            storage
                .connection
                .query_row("SELECT COUNT(*) FROM activity_interval", [], |row| row
                    .get::<_, i64>(0))?,
            1
        );
        let titles: Vec<String> = storage
            .connection
            .prepare("SELECT title FROM window_metadata_change ORDER BY id")?
            .query_map([], |row| row.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        assert_eq!(titles, ["second", "third"]);
        Ok(())
    }

    #[test]
    fn unfinished_interval_is_not_persisted() -> Result<()> {
        let mut storage = Storage::open(Path::new(":memory:"))?;
        storage.record(&start(1, timestamp(0), "first"))?;
        assert_eq!(
            storage
                .connection
                .query_row("SELECT COUNT(*) FROM activity_interval", [], |row| row
                    .get::<_, i64>(0))?,
            0
        );
        Ok(())
    }

    #[test]
    fn title_segment_view_supports_app_and_title_totals() -> Result<()> {
        let mut storage = Storage::open(Path::new(":memory:"))?;
        let started_at = timestamp(0);
        storage.record(&start(1, started_at, "first"))?;
        storage.record(&ActivityEvent::WindowMetadataChanged {
            interval_id: 1,
            window: window("second"),
            observed_at: timestamp(10),
        })?;
        storage.record(&finish(1, started_at, timestamp(20), "first"))?;

        let app_total: f64 = storage.connection.query_row(
            "SELECT SUM(ended_at - started_at) FROM activity_interval WHERE state = 'active' GROUP BY app_id",
            [],
            |row| row.get(0),
        )?;
        assert_eq!(app_total, 20.0);
        let segments: Vec<(String, f64)> = storage
            .connection
            .prepare(
                "SELECT title, duration_seconds FROM activity_title_segment ORDER BY segment_start",
            )?
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?;
        assert_eq!(segments.len(), 2);
        assert_eq!(segments[0].0, "first");
        assert_eq!(segments[1].0, "second");
        assert!((segments[0].1 - 10.0).abs() < 0.001);
        assert!((segments[1].1 - 10.0).abs() < 0.001);
        Ok(())
    }
    #[test]
    fn report_connections_are_read_only_and_observe_wal_snapshots() -> Result<()> {
        let root = env::temp_dir().join(format!(
            "rxtt-read-only-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_nanos()
        ));
        let path = root.join("activity.sqlite3");
        assert!(open_database_read_only(&path).is_err());
        assert!(!root.exists());
        let writer = open_database(&path)?;
        writer.execute_batch(
            "CREATE TABLE snapshot_test (value INTEGER); INSERT INTO snapshot_test VALUES (1);",
        )?;
        let mut reader = open_database_read_only(&path)?;
        assert!(
            reader
                .execute("INSERT INTO snapshot_test VALUES (2)", [])
                .is_err()
        );
        let transaction = reader.transaction()?;
        let count = |connection: &Connection| {
            connection.query_row("SELECT COUNT(*) FROM snapshot_test", [], |row| {
                row.get::<_, i64>(0)
            })
        };
        assert_eq!(count(&transaction)?, 1);
        writer.execute("INSERT INTO snapshot_test VALUES (2)", [])?;
        assert_eq!(count(&transaction)?, 1);
        transaction.commit()?;
        assert_eq!(count(&reader)?, 2);
        drop(reader);
        for version in [0, 2] {
            writer.pragma_update(None, "user_version", version)?;
            assert!(open_database_read_only(&path).is_err());
            let actual: i64 = writer.pragma_query_value(None, "user_version", |row| row.get(0))?;
            assert_eq!(actual, version);
        }
        drop(writer);
        fs::remove_dir_all(root)?;
        Ok(())
    }
}
