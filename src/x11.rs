use crate::presentation::escape_terminal;
use std::os::fd::AsFd;
use std::thread;
use std::time::Duration;

use nix::errno::Errno;
use nix::poll::{PollFd, PollFlags, PollTimeout, poll};
use x11rb::connection::{Connection, RequestConnection};
use x11rb::protocol::Event;
use x11rb::protocol::screensaver;
use x11rb::protocol::screensaver::ConnectionExt as ScreenSaverExt;
use x11rb::protocol::xproto::ConnectionExt as XprotoExt;
use x11rb::protocol::xproto::{
    Atom, AtomEnum, ChangeWindowAttributesAux, EventMask, GetPropertyReply, Window,
};
use x11rb::rust_connection::RustConnection;

use crate::Result;
use crate::model::WindowInfo;

// X11 property lengths are measured in four-byte units: 64 KiB maximum.
const MAX_PROPERTY_UNITS: u32 = 16_384;

fn valid_property(reply: GetPropertyReply, format: u8, units: u32) -> Option<GetPropertyReply> {
    (reply.format == format && reply.bytes_after == 0 && reply.value.len() <= units as usize * 4)
        .then_some(reply)
}

struct Atoms {
    active_window: Atom,
    supported: Atom,
    supporting_wm_check: Atom,
    net_wm_name: Atom,
    utf8_string: Atom,
    net_wm_pid: Atom,
}

pub struct X11Source {
    conn: RustConnection,
    screen_number: usize,
    root: Window,
    display_name: String,
    session_type: String,
    atoms: Atoms,
    xss_available: bool,
    track_idle: bool,
}

pub struct ProbeReport {
    pub session_type: String,
    pub display_name: String,
    pub screen_number: usize,
    pub root_window: Window,
    pub window_manager: Option<String>,
    pub active_window_supported: bool,
    pub xss_available: bool,
    pub idle_duration: Option<Duration>,
    pub active_window: Option<WindowInfo>,
    pub warnings: Vec<&'static str>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum X11Event {
    ActiveWindowChanged,
    WindowTitleChanged { window_id: Window },
}

impl X11Source {
    pub fn connect(track_idle: bool) -> Result<Self> {
        let display_name = std::env::var_os("DISPLAY")
            .ok_or("$DISPLAY is not set; this does not look like an accessible X11 session")?;
        let display_name = display_name.to_string_lossy().into_owned();
        let (conn, screen_number) = RustConnection::connect(Some(&display_name))?;
        let root = conn
            .setup()
            .roots
            .get(screen_number)
            .ok_or("X11 server did not provide the requested screen")?
            .root;
        let atoms = Atoms {
            active_window: intern_atom(&conn, b"_NET_ACTIVE_WINDOW")?,
            supported: intern_atom(&conn, b"_NET_SUPPORTED")?,
            supporting_wm_check: intern_atom(&conn, b"_NET_SUPPORTING_WM_CHECK")?,
            net_wm_name: intern_atom(&conn, b"_NET_WM_NAME")?,
            utf8_string: intern_atom(&conn, b"UTF8_STRING")?,
            net_wm_pid: intern_atom(&conn, b"_NET_WM_PID")?,
        };
        let xss_available = conn
            .extension_information(screensaver::X11_EXTENSION_NAME)?
            .is_some();
        if track_idle && !xss_available {
            eprintln!("WARNING: MIT-SCREEN-SAVER is unavailable; idle detection is disabled.");
        }

        Ok(Self {
            conn,
            screen_number,
            root,
            display_name,
            session_type: std::env::var("XDG_SESSION_TYPE").unwrap_or_else(|_| "<unset>".into()),
            atoms,
            xss_available,
            track_idle: track_idle && xss_available,
        })
    }

    pub fn subscribe_to_active_window_changes(&self) -> Result<()> {
        self.conn
            .change_window_attributes(
                self.root,
                &ChangeWindowAttributesAux::new().event_mask(EventMask::PROPERTY_CHANGE),
            )?
            .check()?;
        self.conn.flush()?;
        Ok(())
    }

    pub fn drain_events(&self) -> Result<Vec<X11Event>> {
        let mut events = Vec::new();
        while let Some(event) = self.conn.poll_for_event()? {
            let Event::PropertyNotify(event) = event else {
                continue;
            };
            if event.window == self.root && event.atom == self.atoms.active_window {
                events.push(X11Event::ActiveWindowChanged);
            } else if event.atom == self.atoms.net_wm_name
                || event.atom == u32::from(AtomEnum::WM_NAME)
            {
                events.push(X11Event::WindowTitleChanged {
                    window_id: event.window,
                });
            }
        }
        Ok(events)
    }

    pub fn subscribe_to_window_title_changes(&self, window: Window) -> Result<()> {
        self.conn
            .change_window_attributes(
                window,
                &ChangeWindowAttributesAux::new().event_mask(EventMask::PROPERTY_CHANGE),
            )?
            .check()?;
        self.conn.flush()?;
        Ok(())
    }

    pub fn wait_for_events(&self, timeout: Duration) -> Result<()> {
        let timeout = PollTimeout::try_from(timeout)?;
        let poll_result = {
            let mut fds = [PollFd::new(self.conn.stream().as_fd(), PollFlags::POLLIN)];
            poll(&mut fds, timeout)
        };
        if let Err(error) = poll_result
            && error != Errno::EINTR
        {
            return Err(error.into());
        }
        Ok(())
    }

    pub fn read_active_window(&self) -> Option<WindowInfo> {
        for attempt in 0..3 {
            let Some(window_id) = self.active_window_id() else {
                if attempt < 2 {
                    thread::sleep(Duration::from_millis(10));
                    continue;
                }
                return None;
            };

            return Some(self.read_window_info(window_id));
        }
        None
    }

    pub fn idle_duration(&mut self) -> Option<Duration> {
        if !self.track_idle {
            return None;
        }

        match self.query_idle_duration() {
            Ok(idle) => Some(idle),
            Err(error) => {
                self.disable_idle_tracking(&error);
                None
            }
        }
    }

    pub fn probe(&mut self) -> ProbeReport {
        let mut problems = Vec::new();
        if self.session_type.eq_ignore_ascii_case("wayland") {
            problems.push("native Wayland session detected; this tracker only sees X11/XWayland");
        }
        let active_window_supported = self.active_window_supported();
        if !active_window_supported {
            problems.push("window manager does not advertise _NET_ACTIVE_WINDOW");
        }
        ProbeReport {
            session_type: self.session_type.clone(),
            display_name: self.display_name.clone(),
            screen_number: self.screen_number,
            root_window: self.root,
            window_manager: self.wm_name(),
            active_window_supported,
            xss_available: self.xss_available,
            idle_duration: self.idle_duration(),
            active_window: self.read_active_window(),
            warnings: problems,
        }
    }

    fn property(
        &self,
        window: Window,
        property: Atom,
        property_type: Atom,
    ) -> Option<GetPropertyReply> {
        // Focused windows may disappear between the EWMH lookup and metadata
        // requests. Treat those X11 races as missing metadata.
        let (format, length) = if property_type == u32::from(AtomEnum::ATOM) {
            (32, MAX_PROPERTY_UNITS)
        } else if property_type == u32::from(AtomEnum::WINDOW)
            || property_type == u32::from(AtomEnum::CARDINAL)
        {
            (32, 1)
        } else {
            (8, MAX_PROPERTY_UNITS)
        };
        let reply = self
            .conn
            .get_property(false, window, property, property_type, 0, length)
            .ok()?
            .reply()
            .ok()?;
        valid_property(reply, format, length)
    }

    fn active_window_id(&self) -> Option<Window> {
        self.property(self.root, self.atoms.active_window, AtomEnum::WINDOW.into())?
            .value32()?
            .next()
            .filter(|window| *window != 0)
    }

    pub fn read_window_info(&self, window_id: Window) -> WindowInfo {
        let title = self
            .property(window_id, self.atoms.net_wm_name, self.atoms.utf8_string)
            .and_then(|reply| decode_text(&reply.value))
            .or_else(|| {
                self.property(window_id, AtomEnum::WM_NAME.into(), AtomEnum::ANY.into())
                    .and_then(|reply| decode_text(&reply.value))
            });
        let (wm_instance, wm_class) = self
            .property(
                window_id,
                AtomEnum::WM_CLASS.into(),
                AtomEnum::STRING.into(),
            )
            .map(|reply| decode_wm_class(&reply.value))
            .unwrap_or_default();
        let pid = self
            .property(window_id, self.atoms.net_wm_pid, AtomEnum::CARDINAL.into())
            .and_then(|reply| reply.value32()?.next())
            .filter(|pid| *pid > 0);
        let executable = pid.and_then(|pid| {
            std::fs::read_link(format!("/proc/{pid}/exe"))
                .ok()
                .map(|path| path.to_string_lossy().into_owned())
        });

        WindowInfo {
            window_id,
            title,
            wm_instance,
            wm_class,
            pid,
            executable,
        }
    }

    fn disable_idle_tracking(&mut self, error: &str) {
        let error = escape_terminal(error);
        eprintln!("WARNING: idle query failed; disabling idle detection: {error}");
        self.track_idle = false;
    }

    fn query_idle_duration(&self) -> std::result::Result<Duration, String> {
        let cookie = self
            .conn
            .screensaver_query_info(self.root)
            .map_err(|error| error.to_string())?;
        let reply = cookie.reply().map_err(|error| error.to_string())?;
        Ok(Duration::from_millis(u64::from(reply.ms_since_user_input)))
    }

    fn wm_name(&self) -> Option<String> {
        let wm_window = self
            .property(
                self.root,
                self.atoms.supporting_wm_check,
                AtomEnum::WINDOW.into(),
            )?
            .value32()?
            .next()?;
        self.property(wm_window, self.atoms.net_wm_name, self.atoms.utf8_string)
            .and_then(|reply| decode_text(&reply.value))
    }

    fn active_window_supported(&self) -> bool {
        let Some(reply) = self.property(self.root, self.atoms.supported, AtomEnum::ATOM.into())
        else {
            return false;
        };
        reply
            .value32()
            .is_some_and(|mut atoms| atoms.any(|atom| atom == self.atoms.active_window))
    }
}

fn intern_atom(conn: &RustConnection, name: &[u8]) -> Result<Atom> {
    Ok(conn.intern_atom(false, name)?.reply()?.atom)
}

fn decode_text(bytes: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(bytes)
        .trim_end_matches('\0')
        .to_owned();
    (!text.is_empty()).then_some(text)
}

fn decode_wm_class(bytes: &[u8]) -> (Option<String>, Option<String>) {
    let mut values = bytes.split(|byte| *byte == 0).filter_map(decode_text);
    (values.next(), values.next())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wm_class_is_two_nul_terminated_strings() {
        assert_eq!(
            decode_wm_class(b"navigator\0Firefox\0"),
            (Some("navigator".into()), Some("Firefox".into()))
        );
    }
    #[test]
    fn property_limits_reject_incomplete_or_wrong_format_values() {
        let reply = |format, bytes_after, size| GetPropertyReply {
            format,
            bytes_after,
            value: vec![b'x'; size],
            ..Default::default()
        };
        assert!(valid_property(reply(8, 0, 65_536), 8, MAX_PROPERTY_UNITS).is_some());
        assert!(valid_property(reply(8, 1, 65_536), 8, MAX_PROPERTY_UNITS).is_none());
        assert!(valid_property(reply(8, 0, 65_537), 8, MAX_PROPERTY_UNITS).is_none());
        assert!(valid_property(reply(32, 0, 4), 8, MAX_PROPERTY_UNITS).is_none());
        assert!(valid_property(reply(32, 0, 4), 32, 1).is_some());
        assert!(valid_property(reply(32, 4, 4), 32, 1).is_none());
        assert!(valid_property(reply(32, 0, 65_536), 32, MAX_PROPERTY_UNITS).is_some());
        let fallback = valid_property(reply(8, 4, 65_536), 8, MAX_PROPERTY_UNITS)
            .and_then(|reply| decode_text(&reply.value))
            .or_else(|| decode_text(b"fallback"));
        assert_eq!(fallback.as_deref(), Some("fallback"));
    }
}
