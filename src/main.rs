mod cli;
mod config;
mod dispatcher;
mod model;
mod output;
mod presentation;
mod report;
mod storage;
mod tracker;
mod unlogged;
mod workflow;
mod x11;

use std::error::Error;
use std::sync::{Arc, atomic::AtomicBool};

use cli::{Cli, Command, DaemonArgs, ReportArgs, TitleTestArgs, WorkflowArgs};
use dispatcher::spawn_activity_dispatcher;
use output::print_probe;
use signal_hook::consts::signal::{SIGINT, SIGTERM};
use storage::Storage;
use tracker::Tracker;
use x11::X11Source;

pub type Result<T> = std::result::Result<T, Box<dyn Error>>;

fn main() {
    let (command, config_path) = Cli::parse_and_validate().into_parts();
    let title_grouping_config = config::load(config_path.as_deref());
    let exit_code = match command {
        Command::Daemon(args) => run_daemon(args),
        Command::Report(args) => run_report(args, &title_grouping_config),
        Command::Workflow(args) => run_workflow(args),
        Command::TitleTest(args) => run_title_test(args, &title_grouping_config),
        Command::Probe => run_probe(),
    };
    std::process::exit(exit_code);
}

fn run_probe() -> i32 {
    let mut source = connect_or_exit(true);
    print_probe(&source.probe())
}

fn run_report(args: ReportArgs, title_grouping_config: &config::TitleGroupingConfig) -> i32 {
    let database_path = match args.database_path() {
        Ok(path) => path,
        Err(error) => {
            eprintln!("ERROR: cannot determine activity database path: {error}");
            return 2;
        }
    };
    let range = match args.time_range() {
        Ok(range) => range,
        Err(error) => {
            eprintln!("ERROR: invalid report date range: {error}");
            return 2;
        }
    };
    match report::print_report(
        &database_path,
        range,
        !args.no_tree,
        !args.no_group_titles,
        args.no_ansi,
        args.verbose,
        title_grouping_config,
    ) {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("ERROR: cannot create usage report: {error}");
            1
        }
    }
}

fn run_title_test(args: TitleTestArgs, title_grouping_config: &config::TitleGroupingConfig) -> i32 {
    let trace = title_grouping_config.trace_title(&args.app_id, &args.title);
    println!("{}", config::format_title_trace(&args.app_id, &trace));
    match args.expect {
        Some(expected) if expected == trace.result => {
            println!("Expectation: PASS");
            0
        }
        Some(expected) => {
            println!("Expectation: FAIL (expected \"{expected}\")");
            1
        }
        None => 0,
    }
}

fn run_workflow(args: WorkflowArgs) -> i32 {
    let database_path = match args.database_path() {
        Ok(path) => path,
        Err(error) => {
            eprintln!("ERROR: cannot determine activity database path: {error}");
            return 2;
        }
    };
    let range = match args.time_range() {
        Ok(range) => range,
        Err(error) => {
            eprintln!("ERROR: invalid workflow date range: {error}");
            return 2;
        }
    };
    match workflow::print_workflow(&database_path, range, args.no_ansi, args.json, args.verbose) {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("ERROR: cannot create workflow: {error}");
            1
        }
    }
}

fn run_daemon(args: DaemonArgs) -> i32 {
    let source = connect_or_exit(args.track_idle());
    let database_path = match args.database_path() {
        Ok(path) => path,
        Err(error) => {
            eprintln!("ERROR: cannot determine activity database path: {error}");
            return 2;
        }
    };
    let storage = match Storage::open(&database_path) {
        Ok(storage) => storage,
        Err(error) => {
            eprintln!(
                "ERROR: cannot initialize activity database {}: {error}",
                database_path.display()
            );
            return 2;
        }
    };
    let shutdown_requested = Arc::new(AtomicBool::new(false));
    for signal in [SIGINT, SIGTERM] {
        if let Err(error) = signal_hook::flag::register(signal, Arc::clone(&shutdown_requested)) {
            eprintln!("ERROR: cannot install signal handler: {error}");
            return 2;
        }
    }

    let dispatcher = spawn_activity_dispatcher(storage);
    let mut tracker = Tracker::new(
        source,
        args.tracker_config(),
        dispatcher.sender(),
        dispatcher.failure(),
    );
    let run_result = tracker.run(&shutdown_requested);
    if let Err(error) = &run_result {
        eprintln!("ERROR: activity tracker stopped unexpectedly: {error}");
        if let Err(finish_error) = tracker.finish_interval("tracker error") {
            eprintln!("ERROR: cannot finish activity interval: {finish_error}");
        }
    }
    drop(tracker);
    let dispatcher_result = dispatcher.join();
    if let Err(error) = dispatcher_result {
        eprintln!("ERROR: activity event dispatcher stopped unexpectedly: {error}");
        return 1;
    }
    if run_result.is_err() { 1 } else { 0 }
}

fn connect_or_exit(track_idle: bool) -> X11Source {
    match X11Source::connect(track_idle) {
        Ok(source) => source,
        Err(error) => {
            eprintln!("ERROR: cannot initialize X11 tracker: {error}");
            std::process::exit(2);
        }
    }
}
