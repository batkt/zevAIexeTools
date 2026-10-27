//! Windows Event Log collector.
//!
//! Talks to wevtapi.dll directly (EvtQuery / EvtNext / EvtRender /
//! EvtFormatMessage) rather than shelling out to `wevtutil` or PowerShell —
//! a service that spawns a PowerShell every 60s is both slow and the kind of
//! thing endpoint protection flags.
//!
//! Reads any channel the account can open: System, Application, Security, and
//! the "Microsoft-Windows-…/Operational" channels. Running as LocalSystem (the
//! default for an installed service) is what grants Security channel access.

use crate::config::EventLogConfig;
use crate::event::{DetectorEvent, Severity, Source};
use std::collections::{HashSet, VecDeque};

/// Remembers which EventRecordIDs were already shipped so an overlapping poll
/// window (we deliberately query a few seconds wider than the interval, to
/// survive clock jitter) does not re-send the same event.
pub struct EventLogCollector {
    cfg: EventLogConfig,
    seen: HashSet<String>,
    seen_order: VecDeque<String>,
    last_poll: std::time::Instant,
    first_run: bool,
}

const SEEN_CAP: usize = 20_000;

impl EventLogCollector {
    pub fn new(cfg: EventLogConfig) -> Self {
        Self {
            cfg,
            seen: HashSet::new(),
            seen_order: VecDeque::new(),
            last_poll: std::time::Instant::now(),
            first_run: true,
        }
    }

    fn remember(&mut self, key: String) -> bool {
        if self.seen.contains(&key) {
            return false;
        }
        if self.seen_order.len() >= SEEN_CAP {
            if let Some(old) = self.seen_order.pop_front() {
                self.seen.remove(&old);
            }
        }
        self.seen.insert(key.clone());
        self.seen_order.push_back(key);
        true
    }

    /// Milliseconds of history to ask for. On the very first poll we only look
    /// back one interval — starting a fresh agent should not dump a week of
    /// backlog into the dashboard.
    fn window_ms(&mut self) -> u64 {
        let base = self.cfg.poll_interval_secs.max(1) * 1000;
        if self.first_run {
            self.first_run = false;
            self.last_poll = std::time::Instant::now();
            return base;
        }
        let elapsed = self.last_poll.elapsed().as_millis() as u64;
        self.last_poll = std::time::Instant::now();
        // +5s of overlap, capped at an hour so a long stall cannot cause a flood.
        (elapsed + 5_000).min(3_600_000).max(base)
    }

    pub fn poll(&mut self) -> Vec<DetectorEvent> {
        if !self.cfg.enabled {
            return Vec::new();
        }
        let window = self.window_ms();
        let query = format!(
            "*[System[(Level <= {}) and (Level > 0) and TimeCreated[timediff(@SystemTime) <= {}]]]",
            self.cfg.max_level, window
        );

        let mut out = Vec::new();
        for channel in self.cfg.channels.clone() {
            match platform::query_channel(&channel, &query, self.cfg.max_per_poll) {
                Ok(raws) => {
                    for raw in raws {
                        if let Some(ev) = self.convert(&channel, raw) {
                            out.push(ev);
                        }
                    }
                }
                Err(e) => {
                    log::warn!("Event log channel '{channel}' query failed: {e}");
                }
            }
        }
        out
    }

    fn convert(&mut self, channel: &str, raw: RawEvent) -> Option<DetectorEvent> {
        let key = format!("{channel}#{}", raw.record_id);
        if !self.remember(key) {
            return None;
        }
        if self.cfg.ignore_event_ids.contains(&raw.event_id) {
            return None;
        }
        if !self.cfg.providers.is_empty()
            && !self
                .cfg
                .providers
                .iter()
                .any(|p| p.eq_ignore_ascii_case(&raw.provider))
        {
            return None;
        }
        let lower = raw.message.to_lowercase();
        if self
            .cfg
            .ignore_contains
            .iter()
            .any(|s| !s.is_empty() && lower.contains(&s.to_lowercase()))
        {
            return None;
        }

        // Event Log Level is authoritative here — no keyword guessing needed.
        let severity = match raw.level {
            1 | 2 => Severity::Critical,
            3 => Severity::Warning,
            _ => Severity::Info,
        };

        let msg = if raw.message.trim().is_empty() {
            format!("{} Event ID {}", raw.provider, raw.event_id)
        } else {
            raw.message.clone()
        };

        Some(
            DetectorEvent::new(Source::EventLog, severity, "windows", channel, msg)
                .kod(format!("{}/{}", raw.provider, raw.event_id))
                .at(raw.time_created.clone())
                .meta("eventId", raw.event_id)
                .meta("provider", raw.provider.clone())
                .meta("level", raw.level)
                .meta("channel", channel.to_string())
                .meta("recordId", raw.record_id.clone())
                .meta("computer", raw.computer.clone())
                .refingerprint(),
        )
    }
}

#[derive(Debug, Default, Clone)]
pub struct RawEvent {
    pub record_id: String,
    pub event_id: u32,
    pub level: u32,
    pub provider: String,
    pub time_created: String,
    pub computer: String,
    pub message: String,
}

// ─── minimal Event XML reader ────────────────────────────────────────────────
// The Event XML schema is fixed and small; pulling in a full XML crate to read
// six known fields out of it would cost more than it buys.

pub fn attr(xml: &str, tag: &str, attr_name: &str) -> String {
    let Some(start) = xml.find(&format!("<{tag}")) else {
        return String::new();
    };
    let rest = &xml[start..];
    let Some(end) = rest.find('>') else {
        return String::new();
    };
    let open_tag = &rest[..end];
    let needle = format!("{attr_name}='");
    if let Some(p) = open_tag.find(&needle) {
        let after = &open_tag[p + needle.len()..];
        if let Some(q) = after.find('\'') {
            return unescape(&after[..q]);
        }
    }
    let needle = format!("{attr_name}=\"");
    if let Some(p) = open_tag.find(&needle) {
        let after = &open_tag[p + needle.len()..];
        if let Some(q) = after.find('"') {
            return unescape(&after[..q]);
        }
    }
    String::new()
}

pub fn tag_text(xml: &str, tag: &str) -> String {
    let open = format!("<{tag}");
    let close = format!("</{tag}>");
    let Some(start) = xml.find(&open) else {
        return String::new();
    };
    let rest = &xml[start..];
    let Some(gt) = rest.find('>') else {
        return String::new();
    };
    // Self-closing, e.g. <Level/>
    if rest[..gt].ends_with('/') {
        return String::new();
    }
    let body = &rest[gt + 1..];
    let Some(end) = body.find(&close) else {
        return String::new();
    };
    unescape(&body[..end])
}

/// Joins every `<Data …>value</Data>` in EventData — the fallback message when
/// the publisher has no message DLL registered.
pub fn event_data(xml: &str) -> String {
    let mut parts = Vec::new();
    let mut cursor = 0usize;
    while let Some(p) = xml[cursor..].find("<Data") {
        let abs = cursor + p;
        let rest = &xml[abs..];
        let Some(gt) = rest.find('>') else { break };
        if rest[..gt].ends_with('/') {
            cursor = abs + gt + 1;
            continue;
        }
        let body = &rest[gt + 1..];
        let Some(end) = body.find("</Data>") else { break };
        let value = unescape(&body[..end]);
        if !value.trim().is_empty() {
            parts.push(value.trim().to_string());
        }
        cursor = abs + gt + 1 + end + 7;
    }
    parts.join(" | ")
}

fn unescape(s: &str) -> String {
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&#13;", "")
        .replace("&#10;", " ")
        .replace("&amp;", "&")
}

pub fn parse_event_xml(xml: &str) -> RawEvent {
    RawEvent {
        record_id: tag_text(xml, "EventRecordID"),
        event_id: tag_text(xml, "EventID").trim().parse().unwrap_or(0),
        level: tag_text(xml, "Level").trim().parse().unwrap_or(4),
        provider: attr(xml, "Provider", "Name"),
        time_created: attr(xml, "TimeCreated", "SystemTime"),
        computer: tag_text(xml, "Computer"),
        message: String::new(),
    }
}

// ─── platform layer ──────────────────────────────────────────────────────────

#[cfg(windows)]
mod platform {
    use super::{event_data, parse_event_xml, RawEvent};
    use std::collections::HashMap;
    use std::ffi::c_void;

    #[link(name = "wevtapi")]
    extern "system" {
        fn EvtQuery(
            session: isize,
            path: *const u16,
            query: *const u16,
            flags: u32,
        ) -> isize;
        fn EvtNext(
            resultset: isize,
            events_size: u32,
            events: *mut isize,
            timeout: u32,
            flags: u32,
            returned: *mut u32,
        ) -> i32;
        fn EvtRender(
            context: isize,
            fragment: isize,
            flags: u32,
            buffer_size: u32,
            buffer: *mut c_void,
            buffer_used: *mut u32,
            property_count: *mut u32,
        ) -> i32;
        fn EvtOpenPublisherMetadata(
            session: isize,
            publisher_id: *const u16,
            log_file_path: *const u16,
            locale: u32,
            flags: u32,
        ) -> isize;
        fn EvtFormatMessage(
            publisher_metadata: isize,
            event: isize,
            message_id: u32,
            value_count: u32,
            values: *const c_void,
            flags: u32,
            buffer_size: u32,
            buffer: *mut u16,
            buffer_used: *mut u32,
        ) -> i32;
        fn EvtClose(object: isize) -> i32;
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn GetLastError() -> u32;
    }

    const EVT_QUERY_CHANNEL_PATH: u32 = 0x1;
    const EVT_QUERY_REVERSE_DIRECTION: u32 = 0x200;
    const EVT_QUERY_TOLERATE_QUERY_ERRORS: u32 = 0x1000;
    const EVT_RENDER_EVENT_XML: u32 = 1;
    const EVT_FORMAT_MESSAGE_EVENT: u32 = 1;
    const ERROR_INSUFFICIENT_BUFFER: u32 = 122;
    const ERROR_NO_MORE_ITEMS: u32 = 259;
    const ERROR_EVT_UNRESOLVED_VALUE_INSERT: u32 = 15029;
    const ERROR_EVT_UNRESOLVED_PARAMETER_INSERT: u32 = 15030;
    const ERROR_EVT_MAX_INSERTS_REACHED: u32 = 15031;

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    struct Handle(isize);
    impl Drop for Handle {
        fn drop(&mut self) {
            if self.0 != 0 {
                unsafe { EvtClose(self.0) };
            }
        }
    }

    pub fn query_channel(
        channel: &str,
        query: &str,
        max: usize,
    ) -> anyhow::Result<Vec<RawEvent>> {
        let wchannel = wide(channel);
        let wquery = wide(query);

        let h = unsafe {
            EvtQuery(
                0,
                wchannel.as_ptr(),
                wquery.as_ptr(),
                EVT_QUERY_CHANNEL_PATH
                    | EVT_QUERY_REVERSE_DIRECTION
                    | EVT_QUERY_TOLERATE_QUERY_ERRORS,
            )
        };
        if h == 0 {
            let code = unsafe { GetLastError() };
            anyhow::bail!("EvtQuery failed (win32 error {code})");
        }
        let result = Handle(h);

        let mut publishers: HashMap<String, isize> = HashMap::new();
        let mut out = Vec::new();
        let mut batch: [isize; 32] = [0; 32];

        'outer: loop {
            let mut returned: u32 = 0;
            let ok = unsafe {
                EvtNext(
                    result.0,
                    batch.len() as u32,
                    batch.as_mut_ptr(),
                    2_000,
                    0,
                    &mut returned,
                )
            };
            if ok == 0 {
                let code = unsafe { GetLastError() };
                if code != ERROR_NO_MORE_ITEMS {
                    log::debug!("EvtNext on '{channel}' stopped: win32 error {code}");
                }
                break;
            }

            for i in 0..returned as usize {
                let ev = Handle(batch[i]);
                if let Some(xml) = render_xml(ev.0) {
                    let mut raw = parse_event_xml(&xml);
                    raw.message = format_message(ev.0, &raw.provider, &mut publishers)
                        .unwrap_or_default();
                    if raw.message.trim().is_empty() {
                        raw.message = event_data(&xml);
                    }
                    out.push(raw);
                }
                if out.len() >= max {
                    // Remaining handles in this batch are closed by the loop's
                    // Handle drops above; the ones not yet wrapped leak only
                    // until the result set closes, which is immediate here.
                    for &h in &batch[(i + 1)..returned as usize] {
                        unsafe { EvtClose(h) };
                    }
                    break 'outer;
                }
            }

            if returned == 0 {
                break;
            }
        }

        for (_, h) in publishers {
            unsafe { EvtClose(h) };
        }
        Ok(out)
    }

    fn render_xml(event: isize) -> Option<String> {
        let mut used: u32 = 0;
        let mut count: u32 = 0;
        unsafe {
            EvtRender(
                0,
                event,
                EVT_RENDER_EVENT_XML,
                0,
                std::ptr::null_mut(),
                &mut used,
                &mut count,
            );
            if GetLastError() != ERROR_INSUFFICIENT_BUFFER || used == 0 {
                return None;
            }
            let mut buf: Vec<u8> = vec![0; used as usize + 2];
            let ok = EvtRender(
                0,
                event,
                EVT_RENDER_EVENT_XML,
                used,
                buf.as_mut_ptr() as *mut c_void,
                &mut used,
                &mut count,
            );
            if ok == 0 {
                return None;
            }
            Some(utf16_bytes_to_string(&buf[..used as usize]))
        }
    }

    fn format_message(
        event: isize,
        provider: &str,
        cache: &mut HashMap<String, isize>,
    ) -> Option<String> {
        if provider.is_empty() {
            return None;
        }
        let meta = match cache.get(provider) {
            Some(h) => *h,
            None => {
                let wp = wide(provider);
                let h = unsafe {
                    EvtOpenPublisherMetadata(0, wp.as_ptr(), std::ptr::null(), 0, 0)
                };
                cache.insert(provider.to_string(), h);
                h
            }
        };
        if meta == 0 {
            return None;
        }

        let mut used: u32 = 0;
        unsafe {
            EvtFormatMessage(
                meta,
                event,
                0,
                0,
                std::ptr::null(),
                EVT_FORMAT_MESSAGE_EVENT,
                0,
                std::ptr::null_mut(),
                &mut used,
            );
            let code = GetLastError();
            if code != ERROR_INSUFFICIENT_BUFFER || used == 0 {
                return None;
            }
            let mut buf: Vec<u16> = vec![0; used as usize + 1];
            let ok = EvtFormatMessage(
                meta,
                event,
                0,
                0,
                std::ptr::null(),
                EVT_FORMAT_MESSAGE_EVENT,
                used,
                buf.as_mut_ptr(),
                &mut used,
            );
            if ok == 0 {
                // These three mean "rendered, but some %1 inserts stayed raw" —
                // a partially-substituted message still beats no message.
                let code = GetLastError();
                if code != ERROR_EVT_UNRESOLVED_VALUE_INSERT
                    && code != ERROR_EVT_UNRESOLVED_PARAMETER_INSERT
                    && code != ERROR_EVT_MAX_INSERTS_REACHED
                {
                    return None;
                }
            }
            let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
            let text = String::from_utf16_lossy(&buf[..len]);
            let cleaned = text
                .replace(['\r', '\n'], " ")
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ");
            if cleaned.is_empty() {
                None
            } else {
                Some(cleaned)
            }
        }
    }

    fn utf16_bytes_to_string(bytes: &[u8]) -> String {
        let units: Vec<u16> = bytes
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        let len = units.iter().position(|&c| c == 0).unwrap_or(units.len());
        String::from_utf16_lossy(&units[..len])
    }
}

#[cfg(not(windows))]
mod platform {
    use super::RawEvent;

    pub fn query_channel(
        _channel: &str,
        _query: &str,
        _max: usize,
    ) -> anyhow::Result<Vec<RawEvent>> {
        anyhow::bail!("Windows Event Log is only available on Windows")
    }
}
