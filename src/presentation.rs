use chrono::{Local, TimeZone};

use anstyle::{AnsiColor, Effects, Style};

pub(crate) struct TextStyles {
    ansi: bool,
}

impl TextStyles {
    pub(crate) fn new(ansi: bool) -> Self {
        Self { ansi }
    }

    pub(crate) fn header(&self, text: &str) -> String {
        self.paint(
            text,
            Style::new()
                .fg_color(Some(AnsiColor::BrightCyan.into()))
                .effects(Effects::BOLD),
        )
    }

    pub(crate) fn time(&self, text: &str) -> String {
        self.paint(text, Style::new().effects(Effects::DIMMED))
    }

    pub(crate) fn application(&self, text: &str) -> String {
        self.paint(
            text,
            Style::new()
                .fg_color(Some(AnsiColor::BrightGreen.into()))
                .effects(Effects::BOLD),
        )
    }

    pub(crate) fn branch(&self, text: &str) -> String {
        self.paint(
            text,
            Style::new().fg_color(Some(AnsiColor::BrightBlack.into())),
        )
    }

    pub(crate) fn title(&self, text: &str) -> String {
        self.paint(
            text,
            Style::new().fg_color(Some(AnsiColor::BrightYellow.into())),
        )
    }

    pub(crate) fn duration(&self, text: &str) -> String {
        self.paint(text, Style::new().effects(Effects::DIMMED))
    }

    pub(crate) fn muted(&self, text: &str) -> String {
        self.paint(
            text,
            Style::new().fg_color(Some(AnsiColor::BrightBlack.into())),
        )
    }

    fn paint(&self, text: &str, style: Style) -> String {
        if self.ansi {
            format!("{style}{text}{style:#}")
        } else {
            text.to_owned()
        }
    }
}

pub(crate) fn format_duration(seconds: i64) -> String {
    let hours = seconds / 3_600;
    let minutes = (seconds % 3_600) / 60;
    let seconds = seconds % 60;
    match (hours, minutes) {
        (0, 0) => format!("{seconds}s"),
        (0, _) => format!("{minutes}m {seconds:02}s"),
        _ => format!("{hours}h {minutes:02}m {seconds:02}s"),
    }
}

pub(crate) fn escape_terminal(value: &str) -> String {
    let mut output = String::new();
    for character in value.chars() {
        match character {
            '\\' => output.push_str("\\\\"),
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            '\t' => output.push_str("\\t"),
            c if c.is_control()
                || matches!(c, '\u{061c}' | '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}') =>
            {
                use std::fmt::Write;
                write!(output, "\\u{{{:x}}}", c as u32).expect("writing to String cannot fail");
            }
            c => output.push(c),
        }
    }
    output
}

pub(crate) fn quote_terminal(value: &str, quote: char) -> String {
    let escaped = escape_terminal(value).replace(quote, &format!("\\{quote}"));
    format!("{quote}{escaped}{quote}")
}

pub(crate) fn format_title(title: Option<&str>) -> String {
    quote_terminal(title.unwrap_or("<untitled>"), '"')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duration_format_is_compact() {
        assert_eq!(format_duration(5), "5s");
        assert_eq!(format_duration(65), "1m 05s");
        assert_eq!(format_duration(3_665), "1h 01m 05s");
    }

    #[test]
    fn title_output_escapes_bidi_controls_and_quotes() {
        assert_eq!(
            format_title(Some("\u{200e}example")),
            "\"\\u{200e}example\""
        );
        assert_eq!(format_title(Some("a \"quote\"")), "\"a \\\"quote\\\"\"");
    }

    #[test]
    fn ansi_can_be_disabled_or_enabled() {
        let plain = TextStyles::new(false);
        assert_eq!(plain.header("Report"), "Report");
        assert_eq!(plain.application("Example"), "Example");

        let rendered = TextStyles::new(true).header("Report");
        assert!(rendered.starts_with("\x1b["));
        assert!(rendered.ends_with("\x1b[0m"));
    }
}

/// Show dates for multi-day selections and spans crossing local midnight.
pub(crate) fn format_period(start: i64, end: i64, multi_day: bool) -> String {
    let date = |ts| {
        Local
            .timestamp_opt(ts, 0)
            .single()
            .map(|dt| dt.date_naive())
    };
    let include_date = multi_day || date(start) != date(end);
    format!(
        "{}–{}",
        format_timestamp(start, include_date),
        format_timestamp(end, include_date)
    )
}

pub(crate) fn format_timestamp(timestamp: i64, include_date: bool) -> String {
    let format = if include_date {
        "%Y-%m-%d %H:%M:%S"
    } else {
        "%H:%M:%S"
    };
    Local
        .timestamp_opt(timestamp, 0)
        .single()
        .map(|timestamp| timestamp.format(format).to_string())
        .unwrap_or_else(|| format!("<invalid timestamp {timestamp}>"))
}

#[cfg(test)]
mod period_tests {
    use super::*;

    #[test]
    fn crossing_midnight_includes_both_dates() {
        let start = Local
            .with_ymd_and_hms(2026, 1, 1, 23, 59, 0)
            .earliest()
            .unwrap()
            .timestamp();
        let end = Local
            .with_ymd_and_hms(2026, 1, 2, 0, 0, 0)
            .earliest()
            .unwrap()
            .timestamp();
        assert_eq!(
            format_period(start, end, false),
            "2026-01-01 23:59:00–2026-01-02 00:00:00"
        );
        assert_eq!(format_period(start, start + 30, false), "23:59:00–23:59:30");
    }
    #[test]
    fn terminal_values_cannot_inject_controls_and_keep_normal_unicode() {
        assert_eq!(
            escape_terminal("\x1b[31m\n\r\t\x07\x7f\u{009b}\u{202e}"),
            "\\u{1b}[31m\\n\\r\\t\\u{7}\\u{7f}\\u{9b}\\u{202e}"
        );
        assert_eq!(
            escape_terminal("Привет 👩\u{200d}💻"),
            "Привет 👩\u{200d}💻"
        );
        assert_eq!(quote_terminal("a\\b\"\n", '"'), "\"a\\\\b\\\"\\n\"");
        assert_eq!(quote_terminal("a'b", '\''), "'a\\'b'");
    }
}
