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

pub(crate) fn format_title(title: Option<&str>) -> String {
    format!("\"{}\"", title.unwrap_or("<untitled>").replace('"', "\\\""))
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
    fn title_output_keeps_unicode_format_characters_and_escapes_quotes() {
        assert_eq!(format_title(Some("\u{200e}example")), "\"\u{200e}example\"");
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
