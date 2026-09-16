# rxtt

X11 activity tracker. Records focused window intervals to SQLite. Generates usage reports and workflow timelines.

## Platform

- Linux with X11 (Wayland not supported)
- Tested: Debian 13 + XFCE, single monitor
- Requires: X11 server, EWMH `_NET_ACTIVE_WINDOW`, XScreenSaver extension (for idle detection)

## Example

```
# rxtt report --no-tree --no-ansi
Usage report for 2026-07-17 (3h 53m 04s/12h 39m 26s)
Alacritty  1h 30m 52s
Chromium  47m 11s
firefox  39m 40s
Spotify  26m 41s
TelegramDesktop  20m 26s
Free Download Manager  6m 37s
Xfdesktop  26s
Throne  20s
Xfce4-clipman-history  11s

# rxtt report --no-ansi
Usage report for 2026-07-17 (3h 53m 04s/12h 39m 26s)
Alacritty  1h 32m 45s
  ├─ "rxtt"  37m 36s
  ├─ "ilya@ilya:~/projects/rust/rxtt"  19m 06s
  ├─ "π - rxtt"  9m 12s
  ├─ "nvim README.md"  3m 43s
  ├─ "nvim ~/.config/rxtt/config.toml"  3m 09s
# and so on...
```



## Prebuilt binaries

Grab the pre-compiled binaries from the latest [release page](https://github.com/mrtstg/rxtt/releases/latest).

## Build

**Prerequisites:**

- Rust toolchain (edition 2024)
- X11 development headers (`libx11-dev` on Debian/Ubuntu, `libX11-devel` on Fedora)

```sh
git clone <repo> && cd rxtt
cargo build --release
# or, build statically linked binaries using glibc or musl (make required)
make build-static-glibc
make build-static-musl # require musl-tools
```

Binary lands at `target/release/rxtt`. Install globally:

```sh
cargo install --path .
```

**Dependencies:**

| Crate | Purpose |
| --- | --- |
| `clap` (derive) | CLI parsing |
| `x11rb` (+screensaver) | X11 connection, EWMH atoms, idle queries |
| `rusqlite` (bundled) | SQLite storage (sqlite3 bundled, no system dep) |
| `chrono` | Date/time handling |
| `regex` | Title grouping patterns |
| `toml` + `serde` | Config file parsing |
| `unicode-normalization` | Title normalization (NFKC, format-mark removal) |
| `signal-hook` | SIGINT/SIGTERM handling |
| `anstyle` | ANSI terminal styling |
| `nix` (poll) | `poll()` for X11 event loop |

## Autostart

You can add `.desktop` file to `~/.config/autostart/` directory:

```
[Desktop Entry]
Type=Application
Name=RXTT
Exec=/home/<user>/.cargo/bin/rxtt daemon
Terminal=false
```

You can change path depending from where `rxtt` is installed.


## Commands

| Command | Description |
| --- | --- |
| `rxtt daemon` | Foreground tracker. Watches X11 focus/idle, writes intervals to SQLite |
| `rxtt report` | Usage totals grouped by application and title for a date range |
| `rxtt workflow` | Chronological active window spans and unlogged periods |
| `rxtt title-test` | Trace how a raw title normalizes through grouping rules |
| `rxtt probe` | Diagnostic: print X11/EWMH/XScreenSaver support and current active window |

### Global flags

| Flag | Description |
| --- | --- |
| `--config PATH` | TOML config path (default: `$XDG_CONFIG_HOME/rxtt/config.toml` or `~/.config/rxtt/config.toml`). Creates a missing file with default rules on first run, without overwriting an existing file |



## `rxtt daemon`

Run foreground. Watches `_NET_ACTIVE_WINDOW` property changes and XScreenSaver idle state. Writes completed intervals to SQLite. Streams status events to stdout. Stops on Ctrl+C (SIGINT) or SIGTERM.

Only **completed** intervals persist. If the process dies mid-interval, in-memory data for that interval is lost.

### Arguments

| Flag | Default | Description |
| --- | --- | --- |
| `--database PATH` | `$XDG_STATE_HOME/rxtt/activity.sqlite3` (or `~/.local/state/rxtt/activity.sqlite3`) | SQLite database path |
| `--idle-threshold SECONDS` | `300` (5 min) | Seconds without input before entering idle state. Must be finite, >= 0, and representable as a duration |
| `--sample-interval SECONDS` | `0.25` | Seconds between safety focus/idle polls. Must be finite, positive after nanosecond conversion, and fit the polling limit (2,147,483,647 ms) |
| `--title-interval SECONDS` | `5` | Minimum seconds between emitted title-change events for one active window (throttle); finite, >= 0, representable as a duration |
| `--no-idle` | off | Disable idle detection. Track focused windows only (no idle intervals) |

Invalid or overflowing duration arguments are rejected during CLI parsing with exit code 2. Fractional seconds remain supported.

### Usage

```sh
# Default: 5 min idle threshold, 5 s title throttle
rxtt daemon

# Custom database, no idle tracking
rxtt daemon --database /tmp/activity.sqlite3 --no-idle

# Faster polling, longer idle
rxtt daemon --idle-threshold 600 --sample-interval 0.5
```

### Tracker loop

1. Subscribe to `_NET_ACTIVE_WINDOW` property changes
2. Subscribe to current window's title change events
3. Reconcile current state (active/idle) → start interval if needed
4. Loop: drain X11 events → safety poll → flush pending title update → reconcile
5. On shutdown: finish current interval → dispatch completes → exit 0



## `rxtt report`

Print usage totals grouped by application. Shows title tree (collapsed equivalent titles) beneath each app.

Date range defaults to today. Heading shows `(active usage / elapsed selected range)`. When range includes today, elapsed ends at current time.

Below the heading, `Unlogged: <duration>` shows elapsed time with no completed active or idle record, including gaps before the first record and after the last. Recorded idle time is logged and does not count toward this total. Use `--verbose` to list each unlogged period with its local start/end times and duration. Future time is excluded; an initialized database with no records makes the entire elapsed selection unlogged. A missing database is an error (exit code 1); reporting does not create it or its parent directories.

### Arguments

| Flag | Default | Description |
| --- | --- | --- |
| `--database PATH` | same as daemon | SQLite database path |
| `--since YYYY-MM-DD` | today | First included calendar date (inclusive) |
| `--until YYYY-MM-DD` | today | Last included calendar date (inclusive) |
| `--verbose` | off | List each unlogged period beneath the unlogged total |
| `--no-tree` | off | Hide title branches below each application |
| `--no-group-titles` | off | Show exact stored titles instead of grouping equivalents |
| `--no-ansi` | off | Disable ANSI styling (use when redirecting to file) |

### Title grouping

Grouping merges titles that differ only in formatting or volatile UI state:

- **Universal normalization:** NFKC unicode, lowercase, remove format marks, normalize curly quotes, collapse whitespace
- **App-specific regex rules:** Strip browser tab badges (`(5) Inbox` → `inbox`), Telegram unread counters, terminal spinner prefixes, etc.

Rules run in file order. Every replacement in a rule runs in order. See [Config](#config) for details.

### Usage

```sh
# Today
rxtt report

# Show exactly when time was unlogged
rxtt report --verbose

# Two-week range with title tree
rxtt report --since 2026-07-01 --until 2026-07-16

# Flat output, exact titles, no ANSI (for piping)
rxtt report --since 2026-07-01 --until 2026-07-16 --no-tree --no-group-titles --no-ansi
```



## `rxtt workflow`

Print chronological active window spans and unlogged periods. Active spans show local start/end time, application, exact stored title, and duration. Unlogged periods show local start/end time, `Unlogged`, and duration. Dates appear for multi-day selections and spans crossing midnight.

Like `report`, `workflow` requires an existing, initialized database and exits with code 1 if it is missing.

Adjacent spans with identical app + raw title merge. Recorded idle time is omitted but does not count as unlogged. Gaps include the leading and trailing elapsed time in the selection, stopping at now. Title changes shown at daemon's `--title-interval` resolution.

### Arguments

| Flag | Default | Description |
| --- | --- | --- |
| `--database PATH` | same as daemon | SQLite database path |
| `--since YYYY-MM-DD` | today | First included calendar date (inclusive) |
| `--until YYYY-MM-DD` | today | Last included calendar date (inclusive) |
| `--no-ansi` | off | Disable ANSI styling |
| `--json` | off | Export as JSON array (suppresses heading/ANSI) |
| `--verbose` | off | Include unlogged entries and entry kinds in JSON; text already includes gaps |

### JSON output

Default JSON remains an array of active entries: `{ "app_id", "title" (or null), "started_at" (unix), "ended_at" (unix), "duration_seconds" }`.

With `--json --verbose`, the array includes unlogged periods in chronological order. Active entries gain `"kind": "active"`; unlogged entries have no application/title fields:

```json
{"kind": "unlogged", "started_at": 1789456500, "ended_at": 1789458300, "duration_seconds": 1800}
````

### Usage

```sh
# Chronological view for two weeks
rxtt workflow --since 2026-07-01 --until 2026-07-16

# JSON export with unlogged periods
rxtt workflow --json --verbose

# Active-only JSON export
rxtt workflow --since 2026-07-01 --json > workflow.json
```



### Unlogged output example

```text
# rxtt report --verbose --no-ansi
Usage report for 2026-09-15 (1h 00m 00s/2h 00m 00s)
Unlogged: 30m 00s
  01:00:00–01:30:00  Unlogged  30m 00s
Alacritty  1h 00m 00s

# rxtt workflow --no-ansi
Workflow for 2026-09-15
00:00:00–01:00:00  Alacritty  "rxtt"  1h 00m 00s
01:00:00–01:30:00  Unlogged  30m 00s
```

This example has 30 minutes of recorded idle time after the gap. “Unlogged” describes missing completed records, not proof that the computer was offline. The daemon keeps its current interval in memory until it finishes, so that unfinished interval temporarily appears unlogged. Shutdowns, tracker downtime, and lost unfinished intervals can also leave gaps. These commands infer gaps without changing stored data.

## `rxtt title-test`

Trace title normalization pipeline. Shows universal normalization step, then every app rule (matched/skipped) with per-replacement input/output.

No database needed. Uses same config and pipeline as `report`.

### Arguments

| Positional | Description |
| --- | --- |
| `APP_ID` | Application identifier (matched against rule `match` regex) |
| `TITLE` | Raw window title to normalize |

| Flag | Description |
| --- | --- |
| `--expect TITLE` | Expected final title. Prints `PASS`/`FAIL` and exits 0/1. Useful for shell regression |

### Usage

```sh
# Trace normalization
rxtt title-test Firefox '(33) Inbox'

# Regression check (exits nonzero on mismatch)
rxtt title-test Firefox '(33) Inbox' --expect inbox
```



## `rxtt probe`

One-shot diagnostic. Connects to X11, checks EWMH `_NET_ACTIVE_WINDOW` and XScreenSaver support, prints active window info and any warnings.

No arguments beyond `--config`.

### Usage

```sh
rxtt probe
```



## Config

First `rxtt` run creates `$XDG_CONFIG_HOME/rxtt/config.toml` (or `~/.config/rxtt/config.toml`). Use `--config PATH` with any command to select alternate file.

**Format:** TOML. Invalid TOML, wrong version, or bad regex → warning + built-in defaults for that run.

### Structure

```toml
version = 1

[[title_grouping.apps]]
match = "(?i)my-app"
replacements = [
  { pattern = '^\[\d+%\]\s*', replacement = "" },
]
```

| Field | Type | Description |
| --- | --- | --- |
| `version` | integer | Optional. Default - latest config version, now '1' |
| `title_grouping.apps` | array | Ordered list of app rules |
| `apps[].match` | regex | Rust `regex` pattern matched against application ID |
| `apps[].replacements` | array | Ordered list of `{ pattern, replacement }` |
| `replacements[].pattern` | regex | Pattern to match (Rust `regex` syntax) |
| `replacements[].replacement` | string | Replacement string. Supports `$1`, `${name}` capture refs |

### Built-in rules

| App match | What it strips |
| --- | --- |
| Firefox, Chromium, Chrome, Brave | Tab count `*`, `(N)`, `N · `, Telegram-style message counters |
| Telegram | Unread counters `(N)`, inline `- (N)` |
| Terminals (Alacritty, Kitty, WezTerm, GNOME Terminal, Konsole, Terminator, Tilix, Foot, Xterm) | Block-element spinner chars, `[!]` / `[·]` status prefixes |

Rules run in file order. Multiple rules can match same app (all replacements chain).



## Database

SQLite file at `~/.local/state/rxtt/activity.sqlite3` (or `--database` path).

**Settings:** The daemon initializes SQLite with WAL mode, foreign keys ON, a 5s busy timeout, and versioned migrations (`PRAGMA user_version = 1`).

`report` and `workflow` open existing databases read-only, validate schema version 1, and read each result within one snapshot transaction. They do not initialize or migrate databases or change journal mode. SQLite may still use WAL coordination files when reading a running daemon’s database. Configuration-file initialization remains independent of database access.

### Schema (v1)

**`activity_interval`** — completed intervals

| Column | Type | Notes |
| --- | --- | --- |
| `id` | INTEGER PK | Auto-increment |
| `tracker_interval_id` | INTEGER | Unique per daemon session |
| `state` | TEXT | `'active'` or `'idle'` |
| `started_at` | INTEGER | Unix epoch seconds |
| `ended_at` | INTEGER | Unix epoch seconds |
| `start_reason` | TEXT | e.g. `"startup"`, `"_NET_ACTIVE_WINDOW changed"` |
| `end_reason` | TEXT | e.g. `"shutdown"`, `"periodic sample; idle=305.2s"` |
| `initial_window_id` | INTEGER | X11 window ID |
| `initial_title` | TEXT | Window title at interval start |
| `initial_wm_instance` | TEXT | WM_INSTANCE_NAME |
| `initial_wm_class` | TEXT | WM_CLASS class |
| `initial_pid` | INTEGER | Process ID |
| `initial_executable` | TEXT | Executable path |
| `app_id` | TEXT | Derived application identifier |

**`window_metadata_change`** — title/metadata updates during interval

| Column | Type | Notes |
| --- | --- | --- |
| `id` | INTEGER PK | Auto-increment |
| `interval_id` | INTEGER FK | → `activity_interval.id` (CASCADE delete) |
| `observed_at` | INTEGER | Unix epoch seconds |
| `window_id` | INTEGER | X11 window ID |
| `title` | TEXT | Updated title |
| `wm_instance` | TEXT | WM_INSTANCE_NAME |
| `wm_class` | TEXT | WM_CLASS class |
| `pid` | INTEGER | Process ID |
| `executable` | TEXT | Executable path |
| `app_id` | TEXT | Derived application identifier |

**`activity_title_segment`** — view for title-level reporting

Splits each active interval into contiguous title segments. Starts from initial title, ends each segment at next metadata change or interval end. Exposes `duration_seconds` per segment.

## Exit codes

| Code | Meaning |
| --- | --- |
| 0 | Success |
| 1 | Runtime error (storage fail, dispatcher crash) |
| 2 | Configuration error (invalid path, bad dates, X11 init fail) |



## Notes

- **Terminal text:** Titles, application identifiers, diagnostic values, and other external text escape terminal control characters and Unicode bidi controls before styling. Newlines, carriage returns, and tabs appear as `\n`, `\r`, and `\t`; other controls appear as `\u{...}`. Backslashes and enclosing quotes are escaped consistently. Ordinary Unicode and emoji remain readable. Stored metadata and JSON values retain their raw text.
- **X11 metadata limits:** Each title, window-manager name, or `WM_CLASS` property is limited to 64 KiB. Oversized or incorrectly formatted properties are treated as unavailable, not stored as truncated prefixes. Existing fallback paths still apply, such as `WM_NAME` when `_NET_WM_NAME` is unavailable. Scalar window/PID properties read the first 32-bit value and ignore trailing values for window-manager compatibility; the supported-atom list is limited to 16,384 atoms, with incomplete lists treated as unavailable.

- **WAL mode:** Database readable while daemon runs (concurrent reads safe)
- **No recovery:** Unfinished intervals on crash are not recovered
- **Title throttle:** `--title-interval` controls resolution. Shorter = more granular title tracking, more DB rows

# 📜 License
Project is licensed under the BSD-3-Clause license.
