use std::path::Path;

use chrono::Local;
use rusqlite::{Connection, params};
use serde::Serialize;

use crate::Result;
use crate::presentation::{TextStyles, format_duration, format_period, format_title};
use crate::report::TimeRange;
use crate::storage::open_database;
use crate::unlogged::{Gap, load_unlogged};

#[derive(Debug, PartialEq, Eq)]
struct WorkflowEntry {
    app_id: String,
    title: Option<String>,
    started_at: i64,
    ended_at: i64,
}

impl WorkflowEntry {
    fn duration_seconds(&self) -> i64 {
        self.ended_at - self.started_at
    }
}

pub fn print_workflow(
    path: &Path,
    range: TimeRange,
    no_ansi: bool,
    json: bool,
    verbose: bool,
) -> Result<()> {
    let now = Local::now().timestamp();
    let mut connection = open_database(path)?;
    let transaction = connection.transaction()?;
    let entries = load_workflow(&transaction, range)?;
    let gaps = load_unlogged(&transaction, range, now)?;
    transaction.commit()?;
    if json {
        println!(
            "{}",
            if verbose {
                render_verbose_json(&entries, &gaps)?
            } else {
                render_workflow_json(&entries)?
            }
        );
    } else {
        println!("{}", render_workflow(&entries, &gaps, range, !no_ansi));
    }
    Ok(())
}

fn render_verbose_json(entries: &[WorkflowEntry], gaps: &[Gap]) -> serde_json::Result<String> {
    let mut rows: Vec<serde_json::Value> = serde_json::from_str(&render_workflow_json(entries)?)?;
    for row in &mut rows {
        row["kind"] = "active".into();
    }
    rows.extend(gaps.iter().map(|gap| {
        serde_json::json!({
            "kind": "unlogged",
            "started_at": gap.started_at,
            "ended_at": gap.ended_at,
            "duration_seconds": gap.duration_seconds(),
        })
    }));
    rows.sort_by_key(|row| {
        (
            row["started_at"].as_i64().unwrap(),
            row["ended_at"].as_i64().unwrap(),
        )
    });
    serde_json::to_string_pretty(&rows)
}

#[derive(Serialize)]
struct WorkflowJsonEntry<'a> {
    app_id: &'a str,
    title: Option<&'a str>,
    started_at: i64,
    ended_at: i64,
    duration_seconds: i64,
}

fn render_workflow_json(entries: &[WorkflowEntry]) -> serde_json::Result<String> {
    let entries = entries
        .iter()
        .map(|entry| WorkflowJsonEntry {
            app_id: &entry.app_id,
            title: entry.title.as_deref(),
            started_at: entry.started_at,
            ended_at: entry.ended_at,
            duration_seconds: entry.duration_seconds(),
        })
        .collect::<Vec<_>>();
    serde_json::to_string_pretty(&entries)
}

fn load_workflow(connection: &Connection, range: TimeRange) -> Result<Vec<WorkflowEntry>> {
    let mut statement = connection.prepare(
        "SELECT app_id,
                title,
                MAX(segment_start, ?1) AS started_at,
                MIN(segment_end, ?2) AS ended_at
         FROM activity_title_segment
         WHERE segment_end > ?1
           AND segment_start < ?2
           AND segment_end > segment_start
         ORDER BY started_at, ended_at, interval_id",
    )?;
    let entries = statement
        .query_map(params![range.start, range.end], |row| {
            Ok(WorkflowEntry {
                app_id: row.get(0)?,
                title: row.get(1)?,
                started_at: row.get(2)?,
                ended_at: row.get(3)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(merge_adjacent(entries))
}

fn merge_adjacent(entries: Vec<WorkflowEntry>) -> Vec<WorkflowEntry> {
    let mut merged: Vec<WorkflowEntry> = Vec::new();
    for entry in entries {
        if let Some(previous) = merged.last_mut()
            && previous.ended_at == entry.started_at
            && previous.app_id == entry.app_id
            && previous.title == entry.title
        {
            previous.ended_at = entry.ended_at;
        } else {
            merged.push(entry);
        }
    }
    merged
}

fn render_workflow(
    entries: &[WorkflowEntry],
    gaps: &[Gap],
    range: TimeRange,
    ansi: bool,
) -> String {
    let styles = TextStyles::new(ansi);
    let heading = if range.since == range.until {
        format!("Workflow for {}", range.since)
    } else {
        format!("Workflow from {} through {}", range.since, range.until)
    };
    let mut output = styles.header(&heading);
    if entries.is_empty() {
        output.push('\n');
        output.push_str(&styles.muted("No completed active intervals."));
    }

    let mut rows = Vec::new();
    for entry in entries {
        rows.push((
            entry.started_at,
            entry.ended_at,
            format!(
                "{}  {}  {}  {}",
                styles.time(&format_period(
                    entry.started_at,
                    entry.ended_at,
                    range.since != range.until
                )),
                styles.application(&entry.app_id),
                styles.title(&format_title(entry.title.as_deref())),
                styles.duration(&format_duration(entry.duration_seconds())),
            ),
        ));
    }
    rows.extend(
        gaps.iter()
            .map(|gap| (gap.started_at, gap.ended_at, gap.render(range, &styles))),
    );
    rows.sort_by_key(|(start, end, _)| (*start, *end));
    for (_, _, row) in rows {
        output.push('\n');
        output.push_str(&row);
    }
    output
}

#[cfg(test)]
mod tests {
    use crate::presentation::format_timestamp;
    use chrono::NaiveDate;

    use super::*;

    fn range(start: i64, end: i64, since: &str, until: &str) -> TimeRange {
        TimeRange {
            start,
            end,
            since: NaiveDate::parse_from_str(since, "%Y-%m-%d").unwrap(),
            until: NaiveDate::parse_from_str(until, "%Y-%m-%d").unwrap(),
        }
    }

    #[test]
    fn workflow_clips_orders_and_merges_exact_adjacent_segments() -> Result<()> {
        let connection = Connection::open_in_memory()?;
        connection.execute_batch(
            "CREATE TABLE activity_title_segment (
                 interval_id INTEGER NOT NULL,
                 app_id TEXT NOT NULL,
                 title TEXT,
                 segment_start INTEGER NOT NULL,
                 segment_end INTEGER NOT NULL
             );
             INSERT INTO activity_title_segment VALUES
                 (1, 'Alpha', 'same', 0, 10),
                 (2, 'Alpha', 'same', 10, 15),
                 (3, 'Alpha', 'other', 15, 20),
                 (4, 'Beta', NULL, 22, 30),
                 (5, 'Ignored', 'zero', 18, 18);",
        )?;

        let entries = load_workflow(&connection, range(5, 25, "1970-01-01", "1970-01-01"))?;
        assert_eq!(
            entries,
            vec![
                WorkflowEntry {
                    app_id: "Alpha".into(),
                    title: Some("same".into()),
                    started_at: 5,
                    ended_at: 15,
                },
                WorkflowEntry {
                    app_id: "Alpha".into(),
                    title: Some("other".into()),
                    started_at: 15,
                    ended_at: 20,
                },
                WorkflowEntry {
                    app_id: "Beta".into(),
                    title: None,
                    started_at: 22,
                    ended_at: 25,
                },
            ]
        );
        Ok(())
    }

    #[test]
    fn workflow_does_not_merge_gaps_or_different_raw_titles() {
        let entries = merge_adjacent(vec![
            WorkflowEntry {
                app_id: "Example".into(),
                title: Some("first".into()),
                started_at: 0,
                ended_at: 5,
            },
            WorkflowEntry {
                app_id: "Example".into(),
                title: Some("second".into()),
                started_at: 5,
                ended_at: 10,
            },
            WorkflowEntry {
                app_id: "Example".into(),
                title: Some("second".into()),
                started_at: 11,
                ended_at: 15,
            },
        ]);
        assert_eq!(entries.len(), 3);
    }

    #[test]
    fn workflow_rendering_supports_plain_text_and_multiple_dates() {
        let entries = vec![WorkflowEntry {
            app_id: "Example".into(),
            title: None,
            started_at: 0,
            ended_at: 65,
        }];
        let rendered = render_workflow(
            &entries,
            &[],
            range(0, 86_400, "1970-01-01", "1970-01-02"),
            false,
        );
        assert!(rendered.starts_with("Workflow from 1970-01-01 through 1970-01-02"));
        assert!(rendered.contains(&format!(
            "{}–{}",
            format_timestamp(0, true),
            format_timestamp(65, true)
        )));
        assert!(rendered.contains("Example  \"<untitled>\"  1m 05s"));
        assert!(!rendered.contains("\u{1b}["));
    }

    #[test]
    fn empty_workflow_has_a_clear_message() {
        let rendered = render_workflow(&[], &[], range(0, 1, "1970-01-01", "1970-01-01"), false);
        assert!(rendered.contains("No completed active intervals."));
    }

    #[test]
    fn workflow_json_exports_clipped_entry_facts() {
        let json = render_workflow_json(&[WorkflowEntry {
            app_id: "Example".into(),
            title: None,
            started_at: 10,
            ended_at: 75,
        }])
        .unwrap();
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(
            value,
            serde_json::json!([{
                "app_id": "Example",
                "title": null,
                "started_at": 10,
                "ended_at": 75,
                "duration_seconds": 65,
            }])
        );
    }
    #[test]
    fn unlogged_rows_are_chronological_and_verbose_json_is_typed() -> Result<()> {
        let entries = [WorkflowEntry {
            app_id: "Example".into(),
            title: None,
            started_at: 10,
            ended_at: 20,
        }];
        let gaps = [
            Gap {
                started_at: 0,
                ended_at: 10,
            },
            Gap {
                started_at: 20,
                ended_at: 30,
            },
        ];
        let selected = range(0, 30, "1970-01-01", "1970-01-01");
        let rendered = render_workflow(&entries, &gaps, selected, false);
        let lines: Vec<_> = rendered.lines().collect();
        assert!(lines[1].contains("Unlogged"));
        assert!(lines[2].contains("Example"));
        assert!(lines[3].contains("Unlogged"));
        assert!(!rendered.contains('\x1b'));
        let json: serde_json::Value = serde_json::from_str(&render_verbose_json(&entries, &gaps)?)?;
        assert_eq!(
            json[0],
            serde_json::json!({"kind": "unlogged", "started_at": 0, "ended_at": 10, "duration_seconds": 10})
        );
        assert_eq!(json[1]["kind"], "active");
        assert_eq!(json[1]["title"], serde_json::Value::Null);
        assert_eq!(json[2]["started_at"], 20);
        let empty = render_workflow(&[], &gaps, selected, false);
        assert!(empty.contains("No completed active intervals."));
        assert_eq!(empty.matches("Unlogged").count(), 2);
        Ok(())
    }
}
