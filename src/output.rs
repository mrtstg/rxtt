use chrono::{DateTime, Local, SecondsFormat};

use crate::model::{ActivityEvent, ActivityState, WindowInfo};
use crate::x11::ProbeReport;

pub fn print_probe(report: &ProbeReport) -> i32 {
    println!("X11 probe");
    println!("---------");
    println!("XDG_SESSION_TYPE : {}", report.session_type);
    println!("DISPLAY          : {}", report.display_name);
    println!("connected display: {}", report.display_name);
    println!("screen number    : {}", report.screen_number);
    println!("root window      : 0x{:x}", report.root_window);
    println!(
        "window manager   : {}",
        report.window_manager.as_deref().unwrap_or("<unknown>")
    );
    println!(
        "_NET_ACTIVE_WINDOW supported: {}",
        report.active_window_supported
    );
    println!("MIT-SCREEN-SAVER available  : {}", report.xss_available);
    if let Some(idle) = report.idle_duration {
        println!("current idle time: {:.3} s", idle.as_secs_f64());
    }

    println!("\nActive window");
    println!("-------------");
    if let Some(info) = report.active_window.as_ref() {
        print_window(info);
    } else {
        println!("<none or metadata unavailable>");
    }

    if report.warnings.is_empty() {
        println!("\nProbe result: OK");
        0
    } else {
        println!("\nProbe result: WARNING");
        for warning in &report.warnings {
            println!("- {warning}");
        }
        1
    }
}

pub fn render_event(event: &ActivityEvent) {
    match event {
        ActivityEvent::IntervalStarted {
            state,
            window,
            started_at,
            reason,
            ..
        } => print_interval_start(*state, window.as_ref(), started_at, reason),
        ActivityEvent::IntervalFinished {
            state,
            window,
            started_at,
            ended_at,
            duration,
            reason,
            ..
        } => print_interval_end(
            *state,
            window.as_ref(),
            started_at,
            ended_at,
            duration,
            reason,
        ),
        ActivityEvent::WindowMetadataChanged {
            window,
            observed_at,
            ..
        } => print_window_metadata_changed(window, observed_at),
        ActivityEvent::Status(message) => println!("{message}"),
    }
}

fn print_window_metadata_changed(window: &WindowInfo, observed_at: &DateTime<Local>) {
    println!(
        "WINDOW   {}  metadata_changed  app={} xid=0x{:x}  title={}",
        format_time(observed_at),
        quoted(&window.app_id()),
        window.window_id,
        quoted_option(window.title.as_deref()),
    );
}

fn print_interval_start(
    state: ActivityState,
    window: Option<&WindowInfo>,
    started_at: &DateTime<Local>,
    reason: &str,
) {
    let time = format_time(started_at);
    if state == ActivityState::Active
        && let Some(window) = window
    {
        println!(
            "START    {time}  active  app={} xid=0x{:x}  title={}  reason={reason}",
            quoted(&window.app_id()),
            window.window_id,
            quoted_option(window.title.as_deref()),
        );
        return;
    }

    let app = window.map(WindowInfo::app_id);
    println!(
        "START    {time}  {}  last_app={}  reason={reason}",
        state.as_str(),
        quoted_option(app.as_deref()),
    );
}

fn print_interval_end(
    state: ActivityState,
    window: Option<&WindowInfo>,
    started_at: &DateTime<Local>,
    ended_at: &DateTime<Local>,
    duration: &std::time::Duration,
    reason: &str,
) {
    let app = window.map(WindowInfo::app_id);
    let xid = window.map(|window| format!("0x{:x}", window.window_id));
    println!(
        "INTERVAL {} -> {}  duration={:8.3}s  state={}  app={} xid={}  reason={reason}",
        format_time(started_at),
        format_time(ended_at),
        duration.as_secs_f64(),
        state.as_str(),
        quoted_option(app.as_deref()),
        quoted_option(xid.as_deref()),
    );
}

pub fn print_window(info: &WindowInfo) {
    println!("window id : 0x{:x}", info.window_id);
    println!("app id    : {}", info.app_id());
    println!(
        "WM_CLASS  : instance={}, class={}",
        quoted_option(info.wm_instance.as_deref()),
        quoted_option(info.wm_class.as_deref())
    );
    println!(
        "PID       : {}",
        info.pid
            .map_or_else(|| "None".into(), |pid| pid.to_string())
    );
    println!("executable: {}", quoted_option(info.executable.as_deref()));
    println!("title     : {}", quoted_option(info.title.as_deref()));
}

fn format_time(time: &DateTime<Local>) -> String {
    time.to_rfc3339_opts(SecondsFormat::Secs, false)
}

fn quoted(value: &str) -> String {
    format!("'{}'", value.replace('\\', "\\\\").replace('\'', "\\'"))
}

fn quoted_option(value: Option<&str>) -> String {
    value.map(quoted).unwrap_or_else(|| "None".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quoted_output_matches_python_style_for_basic_values() {
        assert_eq!(quoted_option(None), "None");
        assert_eq!(quoted("O'Reilly"), "'O\\'Reilly'");
    }
}
