use std::path::PathBuf;
use std::time::Duration;

use chrono::{Local, NaiveDate, TimeZone};
use clap::{Args, CommandFactory, Parser, Subcommand, error::ErrorKind};

use crate::report::TimeRange;
use crate::storage::default_database_path;
use crate::tracker::TrackerConfig;

#[derive(Debug, Parser)]
#[command(about = "Track active X11 windows as activity intervals")]
#[command(subcommand_required = true, arg_required_else_help = true)]
pub struct Cli {
    /// TOML configuration path (defaults to the XDG config directory).
    #[arg(long, global = true)]
    config: Option<PathBuf>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Run the foreground activity tracker.
    Daemon(DaemonArgs),
    /// Print usage totals from the local activity database.
    Report(ReportArgs),
    /// Print a chronological sequence of active window titles.
    Workflow(WorkflowArgs),
    /// Trace title-grouping rules for one window title.
    TitleTest(TitleTestArgs),
    /// Check X11/EWMH/XScreenSaver support and print the active window.
    Probe,
}

#[derive(Debug, Args)]
pub struct DaemonArgs {
    /// SQLite database path (defaults to the XDG state directory).
    #[arg(long)]
    pub database: Option<PathBuf>,

    /// Seconds without input before entering idle state.
    #[arg(long, default_value_t = 300.0)]
    idle_threshold: f64,

    /// Seconds between safety focus and idle checks.
    #[arg(long, default_value_t = 0.25)]
    sample_interval: f64,

    /// Minimum seconds between emitted title-change events for one active window.
    #[arg(long = "title-interval", default_value_t = 5.0)]
    title_change_min_interval: f64,

    /// Disable idle detection and track focused windows only.
    #[arg(long)]
    no_idle: bool,
}

#[derive(Debug, Args)]
pub struct ReportArgs {
    /// SQLite database path (defaults to the XDG state directory).
    #[arg(long)]
    pub database: Option<PathBuf>,

    /// First included calendar date (YYYY-MM-DD; defaults to today).
    #[arg(long, value_name = "DATE")]
    since: Option<NaiveDate>,

    /// Last included calendar date (YYYY-MM-DD; defaults to today).
    #[arg(long, value_name = "DATE")]
    until: Option<NaiveDate>,

    /// Hide title totals below each application.
    #[arg(long)]
    pub no_tree: bool,

    /// Keep exact title totals instead of grouping equivalent titles.
    #[arg(long)]
    pub no_group_titles: bool,

    /// Disable ANSI styling in report output.
    #[arg(long)]
    pub no_ansi: bool,
}

#[derive(Debug, Args)]
pub struct WorkflowArgs {
    /// SQLite database path (defaults to the XDG state directory).
    #[arg(long)]
    pub database: Option<PathBuf>,

    /// First included calendar date (YYYY-MM-DD; defaults to today).
    #[arg(long, value_name = "DATE")]
    since: Option<NaiveDate>,

    /// Last included calendar date (YYYY-MM-DD; defaults to today).
    #[arg(long, value_name = "DATE")]
    until: Option<NaiveDate>,

    /// Disable ANSI styling in workflow output.
    #[arg(long)]
    pub no_ansi: bool,

    /// Export workflow entries as JSON.
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Args)]
pub struct TitleTestArgs {
    /// Application identifier to match against title-grouping rules.
    pub app_id: String,

    /// Raw window title to normalize.
    pub title: String,

    /// Expected final title; exits nonzero when it differs.
    #[arg(long)]
    pub expect: Option<String>,
}

impl Cli {
    pub fn parse_and_validate() -> Self {
        let cli = Self::parse();
        if let Command::Daemon(args) = &cli.command {
            args.validate();
        }
        cli
    }

    pub fn into_parts(self) -> (Command, Option<PathBuf>) {
        (self.command, self.config)
    }
}

impl DaemonArgs {
    pub fn database_path(&self) -> crate::Result<PathBuf> {
        database_path(&self.database)
    }

    pub fn track_idle(&self) -> bool {
        !self.no_idle
    }

    pub fn tracker_config(&self) -> TrackerConfig {
        TrackerConfig {
            idle_threshold: Duration::from_secs_f64(self.idle_threshold),
            sample_interval: Duration::from_secs_f64(self.sample_interval),
            title_change_min_interval: Duration::from_secs_f64(self.title_change_min_interval),
        }
    }

    fn validate(&self) {
        if !self.idle_threshold.is_finite() || self.idle_threshold < 0.0 {
            Cli::command()
                .error(
                    ErrorKind::ValueValidation,
                    "--idle-threshold must be a finite value >= 0",
                )
                .exit();
        }
        if !self.sample_interval.is_finite() || self.sample_interval <= 0.0 {
            Cli::command()
                .error(
                    ErrorKind::ValueValidation,
                    "--sample-interval must be a finite value > 0",
                )
                .exit();
        }
        if !self.title_change_min_interval.is_finite() || self.title_change_min_interval < 0.0 {
            Cli::command()
                .error(
                    ErrorKind::ValueValidation,
                    "--title-interval must be a finite value >= 0",
                )
                .exit();
        }
    }
}

impl ReportArgs {
    pub fn database_path(&self) -> crate::Result<PathBuf> {
        database_path(&self.database)
    }

    pub fn time_range(&self) -> crate::Result<TimeRange> {
        time_range(self.since, self.until)
    }
}

impl WorkflowArgs {
    pub fn database_path(&self) -> crate::Result<PathBuf> {
        database_path(&self.database)
    }

    pub fn time_range(&self) -> crate::Result<TimeRange> {
        time_range(self.since, self.until)
    }
}

fn database_path(database: &Option<PathBuf>) -> crate::Result<PathBuf> {
    database
        .clone()
        .map(Ok)
        .unwrap_or_else(default_database_path)
}

fn time_range(since: Option<NaiveDate>, until: Option<NaiveDate>) -> crate::Result<TimeRange> {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn report_tree_and_title_grouping_are_enabled_by_default() {
        let cli = Cli::try_parse_from(["rxtt", "report"]).unwrap();
        let Command::Report(args) = cli.command else {
            panic!("expected report command");
        };
        assert!(!args.no_tree);
        assert!(!args.no_group_titles);
    }

    #[test]
    fn report_negative_flags_disable_tree_and_grouping() {
        let cli =
            Cli::try_parse_from(["rxtt", "report", "--no-tree", "--no-group-titles"]).unwrap();
        let Command::Report(args) = cli.command else {
            panic!("expected report command");
        };
        assert!(args.no_tree);
        assert!(args.no_group_titles);
    }

    #[test]
    fn config_path_is_accepted_by_every_subcommand() {
        for command in [
            ["rxtt", "--config", "/tmp/rxtt.toml", "daemon"].as_slice(),
            ["rxtt", "probe", "--config", "/tmp/rxtt.toml"].as_slice(),
            ["rxtt", "report", "--config", "/tmp/rxtt.toml"].as_slice(),
            ["rxtt", "workflow", "--config", "/tmp/rxtt.toml"].as_slice(),
            [
                "rxtt",
                "title-test",
                "app",
                "title",
                "--config",
                "/tmp/rxtt.toml",
            ]
            .as_slice(),
        ] {
            assert!(Cli::try_parse_from(command).is_ok());
        }
    }

    #[test]
    fn title_test_accepts_an_optional_expected_title() {
        let cli = Cli::try_parse_from([
            "rxtt",
            "title-test",
            "Firefox",
            "(5) Inbox",
            "--expect",
            "inbox",
        ])
        .unwrap();
        let Command::TitleTest(args) = cli.command else {
            panic!("expected title-test command");
        };
        assert_eq!(args.app_id, "Firefox");
        assert_eq!(args.title, "(5) Inbox");
        assert_eq!(args.expect.as_deref(), Some("inbox"));
    }

    #[test]
    fn workflow_accepts_report_compatible_range_and_output_flags() {
        let cli = Cli::try_parse_from([
            "rxtt",
            "workflow",
            "--database",
            "/tmp/activity.sqlite3",
            "--since",
            "2026-07-01",
            "--until",
            "2026-07-02",
            "--no-ansi",
            "--json",
        ])
        .unwrap();
        let Command::Workflow(args) = cli.command else {
            panic!("expected workflow command");
        };
        assert_eq!(args.database, Some(PathBuf::from("/tmp/activity.sqlite3")));
        assert!(args.no_ansi);
        assert!(args.json);
        assert_eq!(args.time_range().unwrap().since.to_string(), "2026-07-01");
        assert_eq!(args.time_range().unwrap().until.to_string(), "2026-07-02");
    }

    #[test]
    fn workflow_rejects_an_inverted_date_range() {
        let cli = Cli::try_parse_from([
            "rxtt",
            "workflow",
            "--since",
            "2026-07-02",
            "--until",
            "2026-07-01",
        ])
        .unwrap();
        let Command::Workflow(args) = cli.command else {
            panic!("expected workflow command");
        };
        assert!(args.time_range().is_err());
    }
}
