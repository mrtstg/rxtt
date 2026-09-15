use chrono::{Local, NaiveDate, TimeZone};

#[derive(Debug, Clone, Copy)]
pub struct TimeRange {
    pub start: i64,
    pub end: i64,
    pub since: NaiveDate,
    pub until: NaiveDate,
}

pub(crate) fn time_range(
    since: Option<NaiveDate>,
    until: Option<NaiveDate>,
) -> crate::Result<TimeRange> {
    let today = Local::now().date_naive();
    let since = since.unwrap_or(today);
    let until = until.unwrap_or(today);
    if until < since {
        return Err("--until must not be earlier than --since".into());
    }
    let end_date = until.succ_opt().ok_or("--until is too late to represent")?;
    Ok(TimeRange {
        start: local_midnight(since)?.timestamp(),
        end: local_midnight(end_date)?.timestamp(),
        since,
        until,
    })
}

fn local_midnight(date: NaiveDate) -> crate::Result<chrono::DateTime<Local>> {
    Local
        .from_local_datetime(&date.and_hms_opt(0, 0, 0).ok_or("invalid report date")?)
        .earliest()
        .ok_or_else(|| format!("cannot determine local midnight for {date}").into())
}

pub(crate) fn selected_elapsed_seconds(range: TimeRange, now: i64) -> i64 {
    range.end.min(now).saturating_sub(range.start).max(0)
}
