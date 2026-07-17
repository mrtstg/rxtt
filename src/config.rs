use std::env;
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};

use regex::Regex;
use serde::Deserialize;
use unicode_general_category::{GeneralCategory, get_general_category};
use unicode_normalization::UnicodeNormalization;

pub const DEFAULT_CONFIG: &str = r#"# rxtt title-grouping cleanup rules
# Every matching app block runs in file order. Its replacements also run in
# order. Add, remove, or change blocks to tailor title grouping to your apps.
version = 1

[[title_grouping.apps]]
match = "(?i)(firefox|chromium|chrome|brave)"
replacements = [
  { pattern = '^\*\s+', replacement = "" },
  { pattern = '^\(\d+\)\s+', replacement = "" },
  { pattern = '^\d+\s+·\s+', replacement = "" },
  { pattern = '\s+[—-]\s+\d+\s+(?:new messages?|новое сообщение|новых сообщения|новых сообщений)(\s+[—-]\s+.*)?$', replacement = "$1" },
]

[[title_grouping.apps]]
match = "(?i)telegram"
replacements = [
  { pattern = '^\(\d+\)\s+', replacement = "" },
  { pattern = '\s*[–—-]\s+\(\d+\)$', replacement = "" },
  { pattern = '\s+\(\d+\)$', replacement = "" },
]

[[title_grouping.apps]]
match = "(?i)(alacritty|kitty|wezterm|gnome-terminal|konsole|terminator|tilix|^foot$|^xterm$)"
replacements = [
  { pattern = '^[⠀-⣿]+\s*', replacement = "" },
  { pattern = '^\[\s*[!.⠀-⣿]+\s*\]\s*', replacement = "" },
]
"#;

pub struct TitleGroupingConfig {
    apps: Vec<AppRule>,
}

struct AppRule {
    app_match_source: String,
    app_match: Regex,
    replacements: Vec<TitleReplacement>,
}

struct TitleReplacement {
    pattern_source: String,
    pattern: Regex,
    replacement: String,
}

#[derive(Debug, PartialEq, Eq)]
pub struct TitleGroupingTrace {
    pub input: String,
    pub universally_normalized: String,
    pub rules: Vec<AppRuleTrace>,
    pub result: String,
}

#[derive(Debug, PartialEq, Eq)]
pub struct AppRuleTrace {
    pub app_match: String,
    pub matched: bool,
    pub replacements: Vec<ReplacementTrace>,
}

#[derive(Debug, PartialEq, Eq)]
pub struct ReplacementTrace {
    pub pattern: String,
    pub replacement: String,
    pub input: String,
    pub output: String,
}

pub fn format_title_trace(app_id: &str, trace: &TitleGroupingTrace) -> String {
    let mut output = format!(
        "Title grouping trace\nApplication: {}\nInput: {}\nUniversal normalization: {} -> {}\n",
        display_title(app_id),
        display_title(&trace.input),
        display_title(&trace.input),
        display_title(&trace.universally_normalized),
    );
    for (rule_index, rule) in trace.rules.iter().enumerate() {
        if !rule.matched {
            output.push_str(&format!(
                "Rule {}: skipped (match: {})\n",
                rule_index + 1,
                display_title(&rule.app_match),
            ));
            continue;
        }
        output.push_str(&format!(
            "Rule {}: matched (match: {})\n",
            rule_index + 1,
            display_title(&rule.app_match),
        ));
        for (replacement_index, replacement) in rule.replacements.iter().enumerate() {
            let status = if replacement.input == replacement.output {
                "unchanged"
            } else {
                "changed"
            };
            output.push_str(&format!(
                "  Replacement {}: pattern {} -> {}\n    {} -> {} ({status})\n",
                replacement_index + 1,
                display_title(&replacement.pattern),
                display_title(&replacement.replacement),
                display_title(&replacement.input),
                display_title(&replacement.output),
            ));
        }
    }
    output.push_str(&format!("Final title: {}", display_title(&trace.result)));
    output
}

fn display_title(value: &str) -> String {
    format!("\"{value}\"")
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FileConfig {
    version: Option<u32>,
    title_grouping: FileTitleGrouping,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FileTitleGrouping {
    apps: Vec<FileAppRule>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FileAppRule {
    #[serde(rename = "match")]
    app_match: String,
    replacements: Vec<FileTitleReplacement>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FileTitleReplacement {
    pattern: String,
    replacement: String,
}

pub fn load(path: Option<&Path>) -> TitleGroupingConfig {
    let path = match path
        .map(PathBuf::from)
        .or_else(|| default_config_path().ok())
    {
        Some(path) => path,
        None => {
            eprintln!(
                "WARNING: cannot determine rxtt config path; using built-in title-grouping filters"
            );
            return TitleGroupingConfig::built_in();
        }
    };
    match load_from_path(&path) {
        Ok(config) => config,
        Err(error) => {
            eprintln!(
                "WARNING: cannot load rxtt config {}: {error}; using built-in title-grouping filters",
                path.display()
            );
            TitleGroupingConfig::built_in()
        }
    }
}

pub fn default_config_path() -> Result<PathBuf, String> {
    config_path(
        env::var_os("XDG_CONFIG_HOME").as_deref(),
        env::var_os("HOME").as_deref(),
    )
}

fn config_path(xdg_config_home: Option<&OsStr>, home: Option<&OsStr>) -> Result<PathBuf, String> {
    let config_home = match xdg_config_home.filter(|value| !value.is_empty()) {
        Some(path) => PathBuf::from(path),
        None => PathBuf::from(home.ok_or("HOME is unset")?).join(".config"),
    };
    Ok(config_home.join("rxtt/config.toml"))
}

impl TitleGroupingConfig {
    pub fn built_in() -> Self {
        parse(DEFAULT_CONFIG).expect("the built-in rxtt config must be valid")
    }

    pub fn normalize_title(&self, app_id: &str, title: &str) -> String {
        self.trace_title(app_id, title).result
    }

    pub fn trace_title(&self, app_id: &str, title: &str) -> TitleGroupingTrace {
        let universally_normalized = normalize_universal_title(title);
        let mut current = universally_normalized.clone();
        let rules = self
            .apps
            .iter()
            .map(|app| {
                let matched = app.app_match.is_match(app_id);
                let replacements = if matched {
                    app.replacements
                        .iter()
                        .map(|replacement| {
                            let input = current.clone();
                            current = replacement
                                .pattern
                                .replace_all(&current, replacement.replacement.as_str())
                                .into_owned();
                            ReplacementTrace {
                                pattern: replacement.pattern_source.clone(),
                                replacement: replacement.replacement.clone(),
                                input,
                                output: current.clone(),
                            }
                        })
                        .collect()
                } else {
                    Vec::new()
                };
                AppRuleTrace {
                    app_match: app.app_match_source.clone(),
                    matched,
                    replacements,
                }
            })
            .collect();
        TitleGroupingTrace {
            input: title.to_owned(),
            universally_normalized,
            rules,
            result: current,
        }
    }
}

fn normalize_universal_title(title: &str) -> String {
    title
        .nfkc()
        .flat_map(char::to_lowercase)
        .filter(|character| get_general_category(*character) != GeneralCategory::Format)
        .map(normalize_quote)
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn normalize_quote(character: char) -> char {
    match character {
        '\u{2018}' | '\u{2019}' | '\u{201A}' | '\u{201B}' => '\'',
        '\u{201C}' | '\u{201D}' | '\u{201E}' | '\u{201F}' => '"',
        _ => character,
    }
}

fn load_from_path(path: &Path) -> Result<TitleGroupingConfig, String> {
    if !path.exists() {
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        fs::write(path, DEFAULT_CONFIG).map_err(|error| error.to_string())?;
    }
    let contents = fs::read_to_string(path).map_err(|error| error.to_string())?;
    parse(&contents)
}

fn parse(contents: &str) -> Result<TitleGroupingConfig, String> {
    let config: FileConfig = toml::from_str(contents).map_err(|error| error.to_string())?;
    let version = config.version.unwrap_or(1);
    if version != 1 {
        return Err(format!("unsupported config version {version}"));
    }
    let apps = config
        .title_grouping
        .apps
        .into_iter()
        .map(|app| {
            let app_match = Regex::new(&app.app_match)
                .map_err(|error| format!("invalid app match {:?}: {error}", app.app_match))?;
            let replacements = app
                .replacements
                .into_iter()
                .map(|replacement| {
                    let pattern = Regex::new(&replacement.pattern).map_err(|error| {
                        format!(
                            "invalid title replacement pattern {:?}: {error}",
                            replacement.pattern
                        )
                    })?;
                    Ok(TitleReplacement {
                        pattern_source: replacement.pattern,
                        pattern,
                        replacement: replacement.replacement,
                    })
                })
                .collect::<Result<Vec<_>, String>>()?;
            Ok(AppRule {
                app_match_source: app.app_match,
                app_match,
                replacements,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok(TitleGroupingConfig { apps })
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;

    static NEXT_TEST_ID: AtomicU64 = AtomicU64::new(0);

    #[test]
    fn config_path_prefers_xdg_and_falls_back_to_home() {
        assert_eq!(
            config_path(Some(OsStr::new("/config")), Some(OsStr::new("/home/a"))),
            Ok(PathBuf::from("/config/rxtt/config.toml"))
        );
        assert_eq!(
            config_path(None, Some(OsStr::new("/home/a"))),
            Ok(PathBuf::from("/home/a/.config/rxtt/config.toml"))
        );
    }

    #[test]
    fn missing_custom_config_is_created_with_defaults() {
        let path = env::temp_dir()
            .join(format!(
                "rxtt-config-test-{}-{}",
                std::process::id(),
                NEXT_TEST_ID.fetch_add(1, Ordering::Relaxed)
            ))
            .join("nested/config.toml");
        let config = load(Some(&path));
        assert_eq!(config.normalize_title("Firefox", "(5) Inbox"), "inbox");
        assert_eq!(fs::read_to_string(path).unwrap(), DEFAULT_CONFIG);
    }

    #[test]
    fn custom_app_rules_can_remove_and_add_cleanup_rules() {
        let config = parse(
            "version = 1
             [title_grouping]
             apps = [
                 { match = '(?i)vivaldi', replacements = [
                     { pattern = '^\\[\\d+%\\] ', replacement = '' }
                 ] }
             ]",
        )
        .unwrap();
        assert_eq!(
            config.normalize_title("Firefox", "[42%] Build"),
            "[42%] build"
        );
        assert_eq!(config.normalize_title("Vivaldi", "[42%] Build"), "build");
    }

    #[test]
    fn matching_app_rules_compose_in_file_order() {
        let config = parse(
            "version = 1
             [title_grouping]
             apps = [
                 { match = 'example', replacements = [
                     { pattern = '^working: ', replacement = '' }
                 ] },
                 { match = 'example', replacements = [
                     { pattern = ' \\[done\\]$', replacement = '' }
                 ] }
             ]",
        )
        .unwrap();
        assert_eq!(
            config.normalize_title("example", "working: report [done]"),
            "report"
        );
    }

    #[test]
    fn title_trace_uses_the_full_report_normalization_pipeline() {
        let config = parse(
            "version = 1
             [title_grouping]
             apps = [
                 { match = '(?i)browser', replacements = [
                     { pattern = '^\\(\\d+\\) ', replacement = '' },
                     { pattern = '^inbox$', replacement = 'mail' }
                 ] },
                 { match = 'terminal', replacements = [
                     { pattern = 'never', replacement = '' }
                 ] }
             ]",
        )
        .unwrap();

        let trace = config.trace_title("Browser", "(5)  INBOX");
        assert_eq!(trace.universally_normalized, "(5) inbox");
        assert_eq!(trace.result, "mail");
        assert_eq!(
            config.normalize_title("Browser", "(5)  INBOX"),
            trace.result
        );
        assert_eq!(trace.rules.len(), 2);
        assert!(trace.rules[0].matched);
        assert_eq!(trace.rules[0].replacements[0].input, "(5) inbox");
        assert_eq!(trace.rules[0].replacements[0].output, "inbox");
        assert_eq!(trace.rules[0].replacements[1].output, "mail");
        assert!(!trace.rules[1].matched);
        assert!(trace.rules[1].replacements.is_empty());
    }

    #[test]
    fn title_trace_formatter_keeps_unicode_readable_and_marks_noop_rules() {
        let config = TitleGroupingConfig::built_in();
        let trace = config.trace_title("Firefox", "\u{200e}\u{2068}(5) Inbox");
        let rendered = format_title_trace("Firefox", &trace);

        assert!(rendered.contains("(5) Inbox"));
        assert!(!rendered.contains("\\u{200e}"));
        assert!(rendered.contains("(5) inbox\" -> \"inbox\" (changed)"));
        assert!(rendered.contains("inbox\" -> \"inbox\" (unchanged)"));
        assert!(rendered.contains("Rule 2: skipped"));
    }

    #[test]
    fn invalid_config_is_rejected_for_fallback_handling() {
        assert!(parse("version = 1\nunknown = true").is_err());
        assert!(parse("version = 1\n[title_grouping]\napps = []").is_ok());
        assert!(parse("[title_grouping]\napps = []").is_ok());
        assert!(parse("version = 2\n[title_grouping]\napps = []").is_err());
        assert!(
            parse("version = 1\n[title_grouping]\napps = [{ match = '(', replacements = [] }]")
                .is_err()
        );
        assert!(parse("version = 1\n[title_grouping]\napps = [{ match = 'x', replacements = [{ pattern = '(', replacement = '' }] }]").is_err());
    }
}
