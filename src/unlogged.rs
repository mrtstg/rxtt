use rusqlite::{Connection, params};

use crate::Result;
use crate::presentation::{TextStyles, format_duration, format_period};
use crate::report::TimeRange;

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Gap {
    pub started_at: i64,
    pub ended_at: i64,
}

impl Gap {
    pub fn duration_seconds(&self) -> i64 {
        self.ended_at - self.started_at
    }

    pub fn render(&self, range: TimeRange, styles: &TextStyles) -> String {
        format!(
            "{}  {}  {}",
            styles.time(&format_period(
                self.started_at,
                self.ended_at,
                range.since != range.until
            )),
            styles.muted("Unlogged"),
            styles.duration(&format_duration(self.duration_seconds())),
        )
    }
}

pub(crate) fn load_unlogged(
    connection: &Connection,
    range: TimeRange,
    now: i64,
) -> Result<Vec<Gap>> {
    let end = range.end.min(now);
    if end <= range.start {
        return Ok(Vec::new());
    }
    let mut statement = connection.prepare(
        "SELECT MAX(started_at, ?1), MIN(ended_at, ?2)
         FROM activity_interval
         WHERE state IN ('active', 'idle') AND ended_at > started_at
           AND ended_at > ?1 AND started_at < ?2
         ORDER BY started_at, ended_at",
    )?;
    let intervals = statement.query_map(params![range.start, end], |row| {
        Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?))
    })?;
    let mut cursor = range.start;
    let mut gaps = Vec::new();
    for interval in intervals {
        let (start, end) = interval?;
        if start > cursor {
            gaps.push(Gap {
                started_at: cursor,
                ended_at: start,
            });
        }
        cursor = cursor.max(end);
    }
    if cursor < end {
        gaps.push(Gap {
            started_at: cursor,
            ended_at: end,
        });
    }
    Ok(gaps)
}

pub(crate) fn render_summary(
    gaps: &[Gap],
    range: TimeRange,
    verbose: bool,
    styles: &TextStyles,
) -> String {
    let total = gaps.iter().map(Gap::duration_seconds).sum();
    let mut output = styles.muted(&format!("Unlogged: {}", format_duration(total)));
    if verbose {
        for gap in gaps {
            output.push_str(&format!("\n  {}", gap.render(range, styles)));
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    fn range() -> TimeRange {
        TimeRange {
            start: 10,
            end: 100,
            since: NaiveDate::from_ymd_opt(2026, 1, 1).unwrap(),
            until: NaiveDate::from_ymd_opt(2026, 1, 1).unwrap(),
        }
    }

    fn database() -> Connection {
        let connection = Connection::open_in_memory().unwrap();
        connection.execute_batch("CREATE TABLE activity_interval (state TEXT, started_at INTEGER, ended_at INTEGER);").unwrap();
        connection
    }

    #[test]
    fn gaps_use_union_of_active_and_idle_coverage() -> Result<()> {
        let connection = database();
        connection.execute_batch(
            "INSERT INTO activity_interval VALUES
            ('active', 20, 30), ('idle', 30, 40), ('active', 35, 50),
            ('idle', 36, 38), ('active', 60, 70), ('active', 75, 75),
            ('idle', 90, 80);",
        )?;
        let gaps = load_unlogged(&connection, range(), 95)?;
        assert_eq!(
            gaps,
            vec![
                Gap {
                    started_at: 10,
                    ended_at: 20
                },
                Gap {
                    started_at: 50,
                    ended_at: 60
                },
                Gap {
                    started_at: 70,
                    ended_at: 95
                }
            ]
        );
        assert_eq!(gaps.iter().map(Gap::duration_seconds).sum::<i64>(), 45);
        Ok(())
    }

    #[test]
    fn coverage_clips_to_selection_and_now() -> Result<()> {
        let connection = database();
        connection.execute_batch(
            "INSERT INTO activity_interval VALUES ('active', 0, 20), ('idle', 80, 200);",
        )?;
        assert_eq!(
            load_unlogged(&connection, range(), 90)?,
            vec![Gap {
                started_at: 20,
                ended_at: 80
            }]
        );
        assert_eq!(
            load_unlogged(&connection, range(), 50)?,
            vec![Gap {
                started_at: 20,
                ended_at: 50
            }]
        );
        assert!(load_unlogged(&connection, range(), 15)?.is_empty());
        Ok(())
    }

    #[test]
    fn empty_database_covers_only_elapsed_time() -> Result<()> {
        let connection = database();
        for (now, end) in [(50, 50), (200, 100)] {
            assert_eq!(
                load_unlogged(&connection, range(), now)?,
                vec![Gap {
                    started_at: 10,
                    ended_at: end
                }]
            );
        }
        assert!(load_unlogged(&connection, range(), 5)?.is_empty());
        assert!(load_unlogged(&connection, range(), 10)?.is_empty());
        Ok(())
    }

    #[test]
    fn summary_only_lists_periods_in_verbose_mode() {
        let styles = TextStyles::new(false);
        let gaps = [Gap {
            started_at: 20,
            ended_at: 80,
        }];
        assert_eq!(
            render_summary(&gaps, range(), false, &styles),
            "Unlogged: 1m 00s"
        );
        let verbose = render_summary(&gaps, range(), true, &styles);
        assert_eq!(
            verbose,
            format!("Unlogged: 1m 00s\n  {}", gaps[0].render(range(), &styles))
        );
        assert!(!verbose.contains('\x1b'));
        assert_eq!(render_summary(&[], range(), true, &styles), "Unlogged: 0s");
    }
}
