use std::collections::BTreeMap;
use std::path::Path;

use crate::time::{TimeRange, selected_elapsed_seconds};
use chrono::Local;
use rusqlite::{Connection, params};

use crate::Result;
use crate::config::TitleGroupingConfig;
use crate::presentation::{TextStyles, format_duration, format_title};
use crate::storage::open_database_read_only;
use crate::unlogged::{load_unlogged, render_summary};

#[derive(Debug, PartialEq, Eq)]
struct AppUsage {
    app_id: String,
    seconds: i64,
    titles: Vec<TitleUsage>,
}

#[derive(Debug, PartialEq, Eq)]
struct TitleUsage {
    title: Option<String>,
    seconds: i64,
}

pub struct ReportOptions {
    pub tree: bool,
    pub group_titles: bool,
    pub no_ansi: bool,
    pub verbose: bool,
}

pub fn print_report(
    path: &Path,
    range: TimeRange,
    options: ReportOptions,
    title_grouping_config: &TitleGroupingConfig,
) -> Result<()> {
    let ReportOptions {
        tree,
        group_titles,
        no_ansi,
        verbose,
    } = options;
    let now = Local::now().timestamp();
    let mut connection = open_database_read_only(path)?;
    let transaction = connection.transaction()?;
    let usage = load_usage(
        &transaction,
        range,
        tree,
        group_titles,
        title_grouping_config,
    )?;
    let gaps = load_unlogged(&transaction, range, now)?;
    transaction.commit()?;
    let total_usage = usage.iter().map(|app| app.seconds).sum();
    let selected_elapsed = selected_elapsed_seconds(range, now);
    let styles = TextStyles::new(!no_ansi);

    let heading = if range.since == range.until {
        format!(
            "Usage report for {} ({}/{})",
            range.since,
            format_duration(total_usage),
            format_duration(selected_elapsed)
        )
    } else {
        format!(
            "Usage report from {} through {} ({}/{})",
            range.since,
            range.until,
            format_duration(total_usage),
            format_duration(selected_elapsed)
        )
    };
    println!("{}", styles.header(&heading));
    println!("{}", render_summary(&gaps, range, verbose, &styles));

    if usage.is_empty() {
        println!("{}", styles.muted("No completed active intervals."));
        return Ok(());
    }

    for app in usage {
        println!(
            "{}  {}",
            styles.application(&crate::presentation::escape_terminal(&app.app_id)),
            styles.duration(&format_duration(app.seconds))
        );
        if tree {
            let title_count = app.titles.len();
            for (index, title) in app.titles.into_iter().enumerate() {
                let branch = if index + 1 == title_count {
                    "└─"
                } else {
                    "├─"
                };
                let title_label = format_title(title.title.as_deref());
                println!(
                    "  {} {}  {}",
                    styles.branch(branch),
                    styles.title(&title_label),
                    styles.duration(&format_duration(title.seconds))
                );
            }
        }
    }
    Ok(())
}

fn load_usage(
    connection: &Connection,
    range: TimeRange,
    tree: bool,
    group_titles: bool,
    title_grouping_config: &TitleGroupingConfig,
) -> Result<Vec<AppUsage>> {
    let mut statement = connection.prepare(
        "SELECT app_id,
                SUM(MIN(ended_at, ?2) - MAX(started_at, ?1)) AS seconds
         FROM activity_interval
         WHERE state = 'active' AND ended_at > ?1 AND started_at < ?2
         GROUP BY app_id
         ORDER BY seconds DESC, app_id",
    )?;
    let apps = statement
        .query_map(params![range.start, range.end], |row| {
            Ok(AppUsage {
                app_id: row.get(0)?,
                seconds: row.get(1)?,
                titles: Vec::new(),
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    drop(statement);

    if !tree {
        return Ok(apps);
    }

    apps.into_iter()
        .map(|mut app| {
            app.titles = load_title_usage(connection, range, &app.app_id)?;
            if group_titles {
                app.titles = group_title_usage(&app.app_id, app.titles, title_grouping_config);
            }
            Ok(app)
        })
        .collect()
}

fn group_title_usage(
    app_id: &str,
    titles: Vec<TitleUsage>,
    title_grouping_config: &TitleGroupingConfig,
) -> Vec<TitleUsage> {
    let mut groups: BTreeMap<Option<String>, TitleGroup> = BTreeMap::new();
    for title in titles {
        let key = normalized_title_key(app_id, title.title.as_deref(), title_grouping_config);
        let group = groups.entry(key).or_insert_with(|| TitleGroup {
            seconds: 0,
            label: title.title.clone(),
            label_seconds: title.seconds,
        });
        if title.seconds > group.label_seconds
            || (title.seconds == group.label_seconds && title.title < group.label)
        {
            group.label = title.title.clone();
            group.label_seconds = title.seconds;
        }
        group.seconds += title.seconds;
    }

    let mut grouped: Vec<_> = groups
        .into_values()
        .map(|group| TitleUsage {
            title: group.label,
            seconds: group.seconds,
        })
        .collect();
    grouped.sort_by(|left, right| {
        right
            .seconds
            .cmp(&left.seconds)
            .then_with(|| left.title.cmp(&right.title))
    });
    grouped
}

struct TitleGroup {
    seconds: i64,
    label: Option<String>,
    label_seconds: i64,
}

fn normalized_title_key(
    app_id: &str,
    title: Option<&str>,
    title_grouping_config: &TitleGroupingConfig,
) -> Option<String> {
    title.map(|title| title_grouping_config.normalize_title(app_id, title))
}

fn load_title_usage(
    connection: &Connection,
    range: TimeRange,
    app_id: &str,
) -> Result<Vec<TitleUsage>> {
    let mut statement = connection.prepare(
        "SELECT title,
                SUM(MIN(segment_end, ?2) - MAX(segment_start, ?1)) AS seconds
         FROM activity_title_segment
         WHERE app_id = ?3 AND segment_end > ?1 AND segment_start < ?2
         GROUP BY title
         ORDER BY seconds DESC, title",
    )?;
    Ok(statement
        .query_map(params![range.start, range.end, app_id], |row| {
            Ok(TitleUsage {
                title: row.get(0)?,
                seconds: row.get(1)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    #[test]
    fn report_clips_app_and_title_segments_to_the_requested_range() -> Result<()> {
        let connection = Connection::open_in_memory()?;
        connection.execute_batch(
            "CREATE TABLE activity_interval (
                 id INTEGER PRIMARY KEY, state TEXT NOT NULL, app_id TEXT NOT NULL,
                 started_at INTEGER NOT NULL, ended_at INTEGER NOT NULL
             );
             CREATE TABLE window_metadata_change (
                 id INTEGER PRIMARY KEY, interval_id INTEGER NOT NULL, observed_at INTEGER NOT NULL,
                 title TEXT, app_id TEXT NOT NULL
             );
             CREATE VIEW activity_title_segment AS
             WITH points AS (
                 SELECT id AS interval_id, app_id, 'first' AS title, started_at AS segment_start,
                        ended_at AS interval_end, 0 AS point_order FROM activity_interval
                 UNION ALL
                 SELECT m.interval_id, m.app_id, m.title, m.observed_at, i.ended_at, m.id
                 FROM window_metadata_change AS m JOIN activity_interval AS i ON i.id = m.interval_id
             )
             SELECT interval_id, app_id, title, segment_start,
                    LEAD(segment_start, 1, interval_end) OVER (
                        PARTITION BY interval_id ORDER BY segment_start, point_order
                    ) AS segment_end
             FROM points;
             INSERT INTO activity_interval VALUES (1, 'active', 'Example', 0, 20);
             INSERT INTO window_metadata_change VALUES (1, 1, 10, 'second', 'Example');",
        )?;
        let range = TimeRange {
            start: 5,
            end: 15,
            since: NaiveDate::from_ymd_opt(1970, 1, 1).unwrap(),
            until: NaiveDate::from_ymd_opt(1970, 1, 1).unwrap(),
        };

        let config = TitleGroupingConfig::built_in();
        let usage = load_usage(&connection, range, true, false, &config)?;
        assert_eq!(usage.len(), 1);
        assert_eq!(usage[0].seconds, 10);
        assert_eq!(
            usage[0].titles,
            vec![
                TitleUsage {
                    title: Some("first".into()),
                    seconds: 5
                },
                TitleUsage {
                    title: Some("second".into()),
                    seconds: 5
                },
            ]
        );
        Ok(())
    }

    #[test]
    fn selected_elapsed_time_stops_at_now() {
        let range = TimeRange {
            start: 1_000,
            end: 2_000,
            since: NaiveDate::from_ymd_opt(1970, 1, 1).unwrap(),
            until: NaiveDate::from_ymd_opt(1970, 1, 1).unwrap(),
        };
        assert_eq!(selected_elapsed_seconds(range, 1_250), 250);
        assert_eq!(selected_elapsed_seconds(range, 3_000), 1_000);
        assert_eq!(selected_elapsed_seconds(range, 500), 0);
    }

    #[test]
    fn grouping_merges_only_formatting_equivalent_titles() {
        let config = TitleGroupingConfig::built_in();
        let grouped = group_title_usage(
            "Example",
            vec![
                TitleUsage {
                    title: Some("\u{200e}Quarterly   “Review”".into()),
                    seconds: 5,
                },
                TitleUsage {
                    title: Some("quarterly \"review\"".into()),
                    seconds: 12,
                },
                TitleUsage {
                    title: Some("Quarterly Review 2025".into()),
                    seconds: 7,
                },
            ],
            &config,
        );
        assert_eq!(
            grouped,
            vec![
                TitleUsage {
                    title: Some("quarterly \"review\"".into()),
                    seconds: 17,
                },
                TitleUsage {
                    title: Some("Quarterly Review 2025".into()),
                    seconds: 7,
                },
            ]
        );
    }

    #[test]
    fn grouping_uses_a_stable_label_when_durations_tie() {
        let config = TitleGroupingConfig::built_in();
        let grouped = group_title_usage(
            "Example",
            vec![
                TitleUsage {
                    title: Some("alpha".into()),
                    seconds: 5,
                },
                TitleUsage {
                    title: Some("Alpha".into()),
                    seconds: 5,
                },
                TitleUsage {
                    title: None,
                    seconds: 3,
                },
            ],
            &config,
        );
        assert_eq!(
            grouped,
            vec![
                TitleUsage {
                    title: Some("Alpha".into()),
                    seconds: 10,
                },
                TitleUsage {
                    title: None,
                    seconds: 3,
                },
            ]
        );
    }

    #[test]
    fn browser_notification_counters_merge_without_affecting_other_apps() {
        let config = TitleGroupingConfig::built_in();
        let browser = group_title_usage(
            "Firefox",
            vec![
                TitleUsage {
                    title: Some("(33) Pinterest — Mozilla Firefox".into()),
                    seconds: 3,
                },
                TitleUsage {
                    title: Some("(32) Pinterest — Mozilla Firefox".into()),
                    seconds: 7,
                },
                TitleUsage {
                    title: Some("Pinterest — Mozilla Firefox".into()),
                    seconds: 2,
                },
                TitleUsage {
                    title: Some("Яндекс Мессенджер — 31 новое сообщение - Chromium".into()),
                    seconds: 4,
                },
                TitleUsage {
                    title: Some("Яндекс Мессенджер — 33 новых сообщения - Chromium".into()),
                    seconds: 5,
                },
            ],
            &config,
        );
        assert_eq!(browser[0].seconds, 12);
        assert_eq!(browser[1].seconds, 9);

        let spotify = group_title_usage(
            "Spotify",
            vec![
                TitleUsage {
                    title: Some("Track (1)".into()),
                    seconds: 3,
                },
                TitleUsage {
                    title: Some("Track (2)".into()),
                    seconds: 5,
                },
            ],
            &config,
        );
        assert_eq!(spotify.len(), 2);
    }

    #[test]
    fn telegram_and_terminal_volatile_state_is_grouped() {
        let config = TitleGroupingConfig::built_in();
        let telegram = group_title_usage(
            "TelegramDesktop",
            vec![
                TitleUsage {
                    title: Some("Жена – (946)".into()),
                    seconds: 4,
                },
                TitleUsage {
                    title: Some("(1) Жена – (947)".into()),
                    seconds: 6,
                },
                TitleUsage {
                    title: Some("Жена – (948)".into()),
                    seconds: 2,
                },
            ],
            &config,
        );
        assert_eq!(telegram.len(), 1);
        assert_eq!(telegram[0].seconds, 12);

        let terminal = group_title_usage(
            "Alacritty",
            vec![
                TitleUsage {
                    title: Some("⠸ rxtt".into()),
                    seconds: 4,
                },
                TitleUsage {
                    title: Some("⠋ rxtt".into()),
                    seconds: 6,
                },
                TitleUsage {
                    title: Some("rxtt".into()),
                    seconds: 2,
                },
                TitleUsage {
                    title: Some("[ ! ] Action Required | rxtt".into()),
                    seconds: 3,
                },
                TitleUsage {
                    title: Some("[ . ] Action Required | rxtt".into()),
                    seconds: 5,
                },
            ],
            &config,
        );
        assert_eq!(terminal.len(), 2);
        assert_eq!(terminal[0].seconds, 12);
        assert_eq!(terminal[1].seconds, 8);
    }
}
