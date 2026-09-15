use std::path::PathBuf;
use std::time::Duration;

use chrono::NaiveDate;
use clap::{Args, Parser, Subcommand};
use nix::poll::PollTimeout;

use crate::storage::default_database_path;
use crate::time::{TimeRange, time_range};
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
    #[arg(long, default_value = "300", value_parser = parse_duration)]
    idle_threshold: Duration,

    /// Seconds between safety focus and idle checks.
    #[arg(long, default_value = "0.25", value_parser = parse_sample_duration)]
    sample_interval: Duration,

    /// Minimum seconds between emitted title-change events for one active window.
    #[arg(long = "title-interval", default_value = "5", value_parser = parse_duration)]
    title_change_min_interval: Duration,

    /// Disable idle detection and track focused windows only.
    #[arg(long)]
    no_idle: bool,
}

#[derive(Debug, Args)]
pub struct SelectionArgs {
    /// SQLite database path (defaults to the XDG state directory).
    #[arg(long)]
    pub database: Option<PathBuf>,

    /// First included calendar date (YYYY-MM-DD; defaults to today).
    #[arg(long, value_name = "DATE")]
    since: Option<NaiveDate>,

    /// Last included calendar date (YYYY-MM-DD; defaults to today).
    #[arg(long, value_name = "DATE")]
    until: Option<NaiveDate>,
}

#[derive(Debug, Args)]
pub struct ReportArgs {
    #[command(flatten)]
    pub selection: SelectionArgs,

    /// Hide title totals below each application.
    #[arg(long)]
    pub no_tree: bool,

    /// Keep exact title totals instead of grouping equivalent titles.
    #[arg(long)]
    pub no_group_titles: bool,

    /// List the start, end, and duration of each unlogged period.
    #[arg(long)]
    pub verbose: bool,

    /// Disable ANSI styling in report output.
    #[arg(long)]
    pub no_ansi: bool,
}

#[derive(Debug, Args)]
pub struct WorkflowArgs {
    #[command(flatten)]
    pub selection: SelectionArgs,

    /// Disable ANSI styling in workflow output.
    #[arg(long)]
    pub no_ansi: bool,

    /// Include unlogged entries in JSON (text output always includes them).
    #[arg(long)]
    pub verbose: bool,

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
            idle_threshold: self.idle_threshold,
            sample_interval: self.sample_interval,
            title_change_min_interval: self.title_change_min_interval,
        }
    }
}

impl SelectionArgs {
    pub fn database_path(&self) -> crate::Result<PathBuf> {
        database_path(&self.database)
    }

    pub fn time_range(&self) -> crate::Result<TimeRange> {
        time_range(self.since, self.until)
    }
}

fn parse_duration(value: &str) -> Result<Duration, String> {
    let seconds: f64 = value.parse().map_err(|_| "expected seconds as a number")?;
    Duration::try_from_secs_f64(seconds)
        .map_err(|_| "seconds must be finite, nonnegative, and representable".into())
}

fn parse_sample_duration(value: &str) -> Result<Duration, String> {
    let duration = parse_duration(value)?;
    if duration.is_zero() {
        return Err("sample interval must be positive after conversion to nanoseconds".into());
    }
    PollTimeout::try_from(duration).map_err(|_| "sample interval exceeds the polling limit")?;
    Ok(duration)
}

fn database_path(database: &Option<PathBuf>) -> crate::Result<PathBuf> {
    database
        .clone()
        .map(Ok)
        .unwrap_or_else(default_database_path)
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
        assert_eq!(
            args.selection.database,
            Some(PathBuf::from("/tmp/activity.sqlite3"))
        );
        assert!(args.no_ansi);
        assert!(args.json);
        assert_eq!(
            args.selection.time_range().unwrap().since.to_string(),
            "2026-07-01"
        );
        assert_eq!(
            args.selection.time_range().unwrap().until.to_string(),
            "2026-07-02"
        );
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
        assert!(args.selection.time_range().is_err());
    }
    #[test]
    fn verbose_is_opt_in_for_report_and_workflow() {
        for verbose in [false, true] {
            let mut args = vec!["rxtt", "report"];
            if verbose {
                args.push("--verbose");
            }
            let Command::Report(report) = Cli::try_parse_from(args).unwrap().command else {
                panic!()
            };
            assert_eq!(report.verbose, verbose);
            let mut args = vec!["rxtt", "workflow", "--json"];
            if verbose {
                args.push("--verbose");
            }
            let Command::Workflow(workflow) = Cli::try_parse_from(args).unwrap().command else {
                panic!()
            };
            assert_eq!(workflow.verbose, verbose);
        }
    }
    #[test]
    fn durations_are_validated_during_parsing() {
        let Command::Daemon(args) = Cli::try_parse_from(["rxtt", "daemon"]).unwrap().command else {
            panic!()
        };
        let config = args.tracker_config();
        assert_eq!(config.idle_threshold, Duration::from_secs(300));
        assert_eq!(config.sample_interval, Duration::from_millis(250));
        assert_eq!(config.title_change_min_interval, Duration::from_secs(5));
        for flag in ["--idle-threshold", "--sample-interval", "--title-interval"] {
            for value in ["NaN", "inf", "-1", "1e300"] {
                assert!(
                    Cli::try_parse_from(["rxtt", "daemon", &format!("{flag}={value}")]).is_err()
                );
            }
            assert!(Cli::try_parse_from(["rxtt", "daemon", &format!("{flag}=0.125")]).is_ok());
        }
        assert!(parse_duration("0").unwrap().is_zero());
        assert!(parse_sample_duration("0").is_err());
        assert!(parse_sample_duration("1e-30").is_err());
        assert!(parse_sample_duration("2147484").is_err());
        assert!(parse_sample_duration("2147483").is_ok());
    }
}
