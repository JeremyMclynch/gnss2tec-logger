use crate::args::NmeaLogFormat;
use std::collections::BTreeMap;
use std::time::{Duration, Instant};

const WATCHED_MESSAGE_IDS: [&str; 6] = ["GSA", "GSV", "GNS", "RMC", "GBS", "GST"];
const MAX_SENTENCE_LEN: usize = 160;

// Periodically emits the latest watched NMEA sentences found in the byte stream.
pub struct NmeaMonitor {
    collector: NmeaSentenceCollector,
    latest: BTreeMap<String, String>,
    updated_since_emit: BTreeMap<String, bool>,
    interval: Option<Duration>,
    format: NmeaLogFormat,
    last_emit: Instant,
}

impl NmeaMonitor {
    pub fn new(interval_secs: u64, format: NmeaLogFormat) -> Self {
        let interval = if interval_secs == 0 {
            None
        } else {
            Some(Duration::from_secs(interval_secs.max(1)))
        };

        Self {
            collector: NmeaSentenceCollector::new(),
            latest: BTreeMap::new(),
            updated_since_emit: BTreeMap::new(),
            interval,
            format,
            last_emit: Instant::now(),
        }
    }

    // Feed raw serial bytes; matching NMEA sentences are retained as latest snapshot by type.
    pub fn ingest(&mut self, bytes: &[u8]) {
        if self.interval.is_none() {
            return;
        }

        let mut sentences = Vec::new();
        self.collector.push_bytes(bytes, &mut sentences);

        for sentence in sentences {
            let Some(message_id) = parse_message_id(&sentence) else {
                continue;
            };
            if !is_watched_message(&message_id) {
                continue;
            }

            self.latest.insert(message_id.clone(), sentence);
            self.updated_since_emit.insert(message_id, true);
        }
    }

    // Emit periodic NMEA status lines for any watched sentences seen since last emission.
    pub fn maybe_emit_logs(&mut self) {
        let Some(interval) = self.interval else {
            return;
        };
        if self.last_emit.elapsed() < interval {
            return;
        }

        for message_id in WATCHED_MESSAGE_IDS {
            let Some(sentence) = self.latest.get(message_id).cloned() else {
                continue;
            };
            if !self
                .updated_since_emit
                .get(message_id)
                .copied()
                .unwrap_or(false)
            {
                continue;
            }

            self.emit_sentence_logs(message_id, &sentence);
            self.updated_since_emit
                .insert(message_id.to_string(), false);
        }

        self.last_emit = Instant::now();
    }

    fn emit_sentence_logs(&self, message_id: &str, sentence: &str) {
        match self.format {
            NmeaLogFormat::Raw => {
                eprintln!("[NMEA:{}:RAW] {}", message_id, sentence);
            }
            NmeaLogFormat::Plain => {
                let plain = summarize_nmea_plain(message_id, sentence)
                    .unwrap_or_else(|| "unable to parse sentence".to_string());
                eprintln!("[NMEA:{}:PLAIN] {}", message_id, plain);
            }
            NmeaLogFormat::Both => {
                eprintln!("[NMEA:{}:RAW] {}", message_id, sentence);
                let plain = summarize_nmea_plain(message_id, sentence)
                    .unwrap_or_else(|| "unable to parse sentence".to_string());
                eprintln!("[NMEA:{}:PLAIN] {}", message_id, plain);
            }
        }
    }
}

// Extract complete NMEA sentences from arbitrary serial bytes.
struct NmeaSentenceCollector {
    capturing: bool,
    buf: Vec<u8>,
}

impl NmeaSentenceCollector {
    fn new() -> Self {
        Self {
            capturing: false,
            buf: Vec::with_capacity(MAX_SENTENCE_LEN),
        }
    }

    fn push_bytes(&mut self, bytes: &[u8], out: &mut Vec<String>) {
        for &byte in bytes {
            if !self.capturing {
                if byte == b'$' {
                    self.capturing = true;
                    self.buf.clear();
                    self.buf.push(byte);
                }
                continue;
            }

            if byte == b'$' {
                // Restart capture on a nested '$' to recover from malformed data.
                self.buf.clear();
                self.buf.push(byte);
                continue;
            }

            if byte == b'\n' {
                if let Ok(raw) = std::str::from_utf8(&self.buf) {
                    let sentence = raw.trim_end_matches('\r').to_string();
                    if sentence.starts_with('$') {
                        out.push(sentence);
                    }
                }
                self.capturing = false;
                self.buf.clear();
                continue;
            }

            if !is_allowed_nmea_byte(byte) {
                self.capturing = false;
                self.buf.clear();
                continue;
            }

            if self.buf.len() >= MAX_SENTENCE_LEN {
                self.capturing = false;
                self.buf.clear();
                continue;
            }

            self.buf.push(byte);
        }
    }
}

fn summarize_nmea_plain(message_id: &str, sentence: &str) -> Option<String> {
    let fields = parse_nmea_fields(sentence)?;
    match message_id {
        "GSA" => summarize_gsa(&fields),
        "GSV" => summarize_gsv(&fields),
        "GNS" => summarize_gns(&fields),
        "RMC" => summarize_rmc(&fields),
        "GBS" => summarize_gbs(&fields),
        "GST" => summarize_gst(&fields),
        _ => None,
    }
}

fn summarize_gsa(fields: &[&str]) -> Option<String> {
    if fields.is_empty() {
        return None;
    }
    let mode = match field(fields, 1) {
        "A" => "automatic",
        "M" => "manual",
        _ => "unknown",
    };
    let fix = match field(fields, 2) {
        "1" => "no-fix",
        "2" => "2D",
        "3" => "3D",
        _ => "unknown",
    };
    let sats_used = fields
        .get(3..15)
        .map(|slice| slice.iter().filter(|value| !value.is_empty()).count())
        .unwrap_or(0);

    Some(format!(
        "mode={} fix={} sats_used={} pdop={} hdop={} vdop={}",
        mode,
        fix,
        sats_used,
        nz(field(fields, 15)),
        nz(field(fields, 16)),
        nz(field(fields, 17))
    ))
}

fn summarize_gsv(fields: &[&str]) -> Option<String> {
    if fields.is_empty() {
        return None;
    }
    Some(format!(
        "msg={}/{} sats_in_view={} talker={}",
        nz(field(fields, 2)),
        nz(field(fields, 1)),
        nz(field(fields, 3)),
        talker_id(field(fields, 0)).unwrap_or("-")
    ))
}

fn summarize_gns(fields: &[&str]) -> Option<String> {
    if fields.is_empty() {
        return None;
    }
    let lat = format_coord(parse_lat(field(fields, 2), field(fields, 3)));
    let lon = format_coord(parse_lon(field(fields, 4), field(fields, 5)));
    Some(format!(
        "time={} mode={} sats_used={} hdop={} lat={} lon={} alt_m={}",
        nz(field(fields, 1)),
        nz(field(fields, 6)),
        nz(field(fields, 7)),
        nz(field(fields, 8)),
        lat,
        lon,
        nz(field(fields, 9))
    ))
}

fn summarize_rmc(fields: &[&str]) -> Option<String> {
    if fields.is_empty() {
        return None;
    }
    let status = match field(fields, 2) {
        "A" => "valid",
        "V" => "warning",
        _ => "unknown",
    };
    let speed = parse_f64(field(fields, 7))
        .map(|knots| format!("{:.2} kn/{:.2} kmh", knots, knots * 1.852))
        .unwrap_or_else(|| "-".to_string());
    let lat = format_coord(parse_lat(field(fields, 3), field(fields, 4)));
    let lon = format_coord(parse_lon(field(fields, 5), field(fields, 6)));
    Some(format!(
        "status={} time={} date={} lat={} lon={} speed={} course_deg={}",
        status,
        nz(field(fields, 1)),
        nz(field(fields, 9)),
        lat,
        lon,
        speed,
        nz(field(fields, 8))
    ))
}

fn summarize_gbs(fields: &[&str]) -> Option<String> {
    if fields.is_empty() {
        return None;
    }
    Some(format!(
        "time={} err_lat_m={} err_lon_m={} err_alt_m={} failed_sat={} prob={} bias={} stddev={}",
        nz(field(fields, 1)),
        nz(field(fields, 2)),
        nz(field(fields, 3)),
        nz(field(fields, 4)),
        nz(field(fields, 5)),
        nz(field(fields, 6)),
        nz(field(fields, 7)),
        nz(field(fields, 8))
    ))
}

fn summarize_gst(fields: &[&str]) -> Option<String> {
    if fields.is_empty() {
        return None;
    }
    Some(format!(
        "time={} rms_m={} semi_major_m={} semi_minor_m={} orient_deg={} sigma_lat_m={} sigma_lon_m={} sigma_alt_m={}",
        nz(field(fields, 1)),
        nz(field(fields, 2)),
        nz(field(fields, 3)),
        nz(field(fields, 4)),
        nz(field(fields, 5)),
        nz(field(fields, 6)),
        nz(field(fields, 7)),
        nz(field(fields, 8))
    ))
}

fn parse_nmea_fields(sentence: &str) -> Option<Vec<&str>> {
    let core = sentence
        .strip_prefix('$')?
        .split('*')
        .next()
        .unwrap_or_default();
    Some(core.split(',').collect())
}

fn parse_f64(raw: &str) -> Option<f64> {
    if raw.is_empty() {
        return None;
    }
    raw.parse::<f64>().ok()
}

fn parse_lat(value: &str, hemi: &str) -> Option<f64> {
    parse_nmea_coord(value, hemi, 2)
}

fn parse_lon(value: &str, hemi: &str) -> Option<f64> {
    parse_nmea_coord(value, hemi, 3)
}

fn parse_nmea_coord(value: &str, hemi: &str, degree_digits: usize) -> Option<f64> {
    if value.len() <= degree_digits {
        return None;
    }

    let (deg_str, min_str) = value.split_at(degree_digits);
    let degrees = deg_str.parse::<f64>().ok()?;
    let minutes = min_str.parse::<f64>().ok()?;

    let mut decimal = degrees + (minutes / 60.0);
    if hemi == "S" || hemi == "W" {
        decimal = -decimal;
    }
    Some(decimal)
}

fn format_coord(coord: Option<f64>) -> String {
    coord
        .map(|value| format!("{value:.6}"))
        .unwrap_or_else(|| "-".to_string())
}

fn talker_id(head: &str) -> Option<&str> {
    if head.len() < 2 {
        return None;
    }
    Some(&head[..2])
}

fn field<'a>(fields: &'a [&'a str], idx: usize) -> &'a str {
    fields.get(idx).copied().unwrap_or("")
}

fn nz(raw: &str) -> &str {
    if raw.is_empty() { "-" } else { raw }
}

fn is_allowed_nmea_byte(byte: u8) -> bool {
    byte == b'\r' || (0x20..=0x7E).contains(&byte)
}

fn parse_message_id(sentence: &str) -> Option<String> {
    let core = sentence
        .strip_prefix('$')?
        .split('*')
        .next()
        .unwrap_or_default();
    let talker_and_id = core.split(',').next().unwrap_or_default();
    if talker_and_id.len() < 3 {
        return None;
    }
    Some(talker_and_id[talker_and_id.len() - 3..].to_string())
}

fn is_watched_message(message_id: &str) -> bool {
    WATCHED_MESSAGE_IDS.contains(&message_id)
}
