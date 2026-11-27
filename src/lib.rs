//! Core protocol logic for dibs, the expiring claim lease protocol.
//!
//! A backlog lives in one registry file. The file holds one JSON
//! document under the format name dibs-registry/1, and the document
//! holds the items, their standing leases, and the full claim history,
//! so a state change and its history event publish in the same rename
//! and can never disagree. Every mutating operation holds an exclusive
//! flock on the lock file beside the registry across its whole read
//! and evaluate and modify and write cycle, and publishes by renaming
//! a complete temporary file over the registry. Reads take a shared
//! lock and open the registry fresh each time, and a reader never
//! writes.
//!
//! Expiry is lazy. There is no daemon and no timer anywhere in this
//! module. Every operation that evaluates lease state takes the
//! current instant as an argument and evaluates expiry against it,
//! and a lease is live at instants strictly before its expiry instant
//! and expired at the expiry instant itself. History returns the
//! recorded events and evaluates nothing.
//!
//! Possession is proven by a token, not by a name. A successful claim
//! returns a fresh token drawn from the system random source, the
//! registry stores only the SHA-256 hash of the token, and renew and
//! release and complete must present the token together with the
//! standing epoch.
//!
//! This crate is the Rust port of the Round 1 Python implementation.
//! The port amendment in CONTRACT.md names the equivalence standard
//! and the reshapings, and the error messages and the stored bytes
//! reproduce the Python behaviour they name.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Component, Path, PathBuf};

use sha2::{Digest, Sha256};

pub const FORMAT_NAME: &str = "dibs-registry/1";
pub const FORMAT_NAME_V2: &str = "dibs-registry/2";
pub const LOCK_SUFFIX: &str = ".lock";
pub const NAME_MAX: usize = 64;
pub const NOTE_MAX: usize = 200;
pub const TOKEN_BYTES: usize = 32;
pub const TTL_MAX: i64 = 31_536_000;
pub const PRIORITY_DEFAULT: u8 = 2;
pub const PRIORITY_MAX: u8 = 3;
pub const TAGS_MAX: usize = 16;
pub const DEPENDS_ON_MAX: usize = 64;
pub const MAX_TAKEOVERS_MAX: u64 = 1_000;

const DOCUMENT_KEYS: [&str; 3] = ["format", "items", "history"];
const DOCUMENT_KEYS_V2: [&str; 4] = ["format", "compaction", "items", "history"];
const COMPACTION_KEYS: [&str; 4] = [
    "through_seq",
    "archived_events_sha256",
    "compacted_at",
    "baseline",
];
const BASELINE_KEYS: [&str; 5] = ["id", "status", "holder", "epoch", "takeover_count"];
const ITEM_KEYS: [&str; 7] = [
    "id",
    "note",
    "status",
    "holder",
    "token_hash",
    "epoch",
    "expires_at",
];
const ITEM_KEYS_V2: [&str; 12] = [
    "id",
    "note",
    "status",
    "priority",
    "tags",
    "depends_on",
    "max_takeovers",
    "takeover_count",
    "holder",
    "token_hash",
    "epoch",
    "expires_at",
];
const EVENT_KEYS: [&str; 7] = [
    "seq",
    "kind",
    "item",
    "holder",
    "at",
    "epoch",
    "previous_holder",
];
const EVENT_KINDS: [&str; 6] = [
    "offered",
    "claimed",
    "renewed",
    "released",
    "completed",
    "taken_over",
];
const EVENT_KINDS_V2: [&str; 7] = [
    "offered",
    "claimed",
    "renewed",
    "released",
    "completed",
    "taken_over",
    "reset",
];
const ITEM_STATUSES: [&str; 3] = ["open", "claimed", "completed"];

/// An error the command line reports with a nonzero exit and no
/// traceback. The first six variants are the error classes of the
/// protocol, each carrying the exit code the Python class carried.
/// The last variant carries the failures the Python implementation
/// let escape as tracebacks, operating system and random source
/// failures, held to the same one line report at exit 1.
#[derive(Debug, Clone, PartialEq)]
pub enum Error {
    InvalidName(String),
    InvalidTtl(String),
    InvalidRequest(String),
    NotFound(String),
    Refusal(String),
    Corrupt(String),
    Failure(String),
}

impl Error {
    pub fn message(&self) -> &str {
        match self {
            Error::InvalidName(message)
            | Error::InvalidTtl(message)
            | Error::InvalidRequest(message)
            | Error::NotFound(message)
            | Error::Refusal(message)
            | Error::Corrupt(message)
            | Error::Failure(message) => message,
        }
    }

    pub fn exit_code(&self) -> i32 {
        match self {
            Error::InvalidName(_) | Error::InvalidTtl(_) | Error::InvalidRequest(_) => 2,
            Error::NotFound(_) | Error::Refusal(_) | Error::Corrupt(_) | Error::Failure(_) => 1,
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.message())
    }
}

impl std::error::Error for Error {}

pub type Result<T> = std::result::Result<T, Error>;

fn corrupt(message: String) -> Error {
    Error::Corrupt(format!("the registry is corrupt, {message}"))
}

/// A value at the untyped boundary of the protocol. The strict
/// registry reader produces these, and the value level validators
/// consume them, so a wrong shape is refused by the same code at
/// every surface where a value arrives untyped, the registry file,
/// the command line converters, and a caller of the library. An
/// integer wider than 128 bits is held as its canonical decimal
/// text, so a refusal message prints the value whole.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Int(i128),
    BigInt(String),
    Float(f64),
    Str(String),
    Array(Vec<Value>),
    Object(Vec<(String, Value)>),
}

/// Render a string the way Python's repr renders it, for the error
/// messages that interpolate a value with repr formatting. The
/// quote is a double quote when the text carries a single quote
/// and no double quote, and a single quote otherwise. Printable
/// text passes through, the quote and the backslash and the common
/// control characters are escaped, and any other character Python
/// counts as not printable is rendered as a hex escape.
pub fn py_repr(text: &str) -> String {
    let quote = if text.contains('\'') && !text.contains('"') {
        '"'
    } else {
        '\''
    };
    let mut out = String::new();
    out.push(quote);
    for ch in text.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c == quote => {
                out.push('\\');
                out.push(c);
            }
            c if !py_printable(c) => {
                let n = c as u32;
                if n <= 0xff {
                    out.push_str(&format!("\\x{n:02x}"));
                } else if n <= 0xffff {
                    out.push_str(&format!("\\u{n:04x}"));
                } else {
                    out.push_str(&format!("\\U{n:08x}"));
                }
            }
            c => out.push(c),
        }
    }
    out.push(quote);
    out
}

/// Whether Python's str.isprintable holds for one character, over
/// the ranges that can reach an error message. Letters, digits,
/// marks, punctuation, symbols, and the plain space are printable.
/// Control and format and separator characters other than the
/// plain space are not, and neither are the private use ranges,
/// which Python counts as not printable in every Unicode version.
fn py_printable(c: char) -> bool {
    let n = c as u32;
    if n < 0x20 || n == 0x7f {
        return false;
    }
    if (0x80..=0x9f).contains(&n) {
        return false;
    }
    !matches!(n,
        0x00a0 | 0x00ad | 0x070f | 0x1680 | 0x180e
        | 0x2000..=0x200f | 0x2028 | 0x2029 | 0x202a..=0x202e | 0x202f
        | 0x205f | 0x2060..=0x206f | 0x3000 | 0xfeff | 0xfff9..=0xfffb
        | 0x0600..=0x0605 | 0x061c | 0x06dd | 0x0890..=0x0891 | 0x08e2
        | 0x180f | 0x1944..=0x1945 | 0x1b4d | 0x1b7d
        | 0xe000..=0xf8ff | 0xf0000..=0xffffd | 0x100000..=0x10fffd
    )
}

/// Render any value the way Python's repr renders it, for the
/// error messages that interpolate a stored or presented value.
pub fn py_repr_value(value: &Value) -> String {
    match value {
        Value::Null => "None".to_string(),
        Value::Bool(true) => "True".to_string(),
        Value::Bool(false) => "False".to_string(),
        Value::Int(number) => number.to_string(),
        Value::BigInt(digits) => digits.clone(),
        Value::Float(number) => py_float_repr(*number),
        Value::Str(text) => py_repr(text),
        Value::Array(items) => {
            let parts: Vec<String> = items.iter().map(py_repr_value).collect();
            format!("[{}]", parts.join(", "))
        }
        Value::Object(pairs) => {
            let parts: Vec<String> = pairs
                .iter()
                .map(|(key, value)| format!("{}: {}", py_repr(key), py_repr_value(value)))
                .collect();
            format!("{{{}}}", parts.join(", "))
        }
    }
}

/// Render a float the way Python's repr renders it, the shortest
/// decimal that round trips, in fixed notation inside the range
/// Python keeps fixed and in scientific notation outside it, with
/// a trailing point zero on an integral fixed value.
fn py_float_repr(number: f64) -> String {
    if number.is_nan() {
        return "nan".to_string();
    }
    if number.is_infinite() {
        return if number < 0.0 { "-inf" } else { "inf" }.to_string();
    }
    if number == 0.0 {
        return if number.is_sign_negative() {
            "-0.0".to_string()
        } else {
            "0.0".to_string()
        };
    }
    let sign = if number < 0.0 { "-" } else { "" };
    // Rust's lower exponential form gives the shortest round trip
    // digits as d.ddddeN, the same digits Python's repr chooses.
    let raw = format!("{:e}", number.abs());
    let (mantissa, exponent) = raw.split_once('e').unwrap();
    let exponent: i32 = exponent.parse().unwrap();
    let digits: String = mantissa.chars().filter(|c| *c != '.').collect();
    if (-4..16).contains(&exponent) {
        let point = exponent + 1;
        let mut out = String::new();
        if point <= 0 {
            out.push_str("0.");
            for _ in 0..(-point) {
                out.push('0');
            }
            out.push_str(&digits);
        } else if (point as usize) >= digits.len() {
            out.push_str(&digits);
            for _ in 0..(point as usize - digits.len()) {
                out.push('0');
            }
            out.push_str(".0");
        } else {
            out.push_str(&digits[..point as usize]);
            out.push('.');
            out.push_str(&digits[point as usize..]);
        }
        format!("{sign}{out}")
    } else {
        let mut mantissa_out = String::new();
        mantissa_out.push_str(&digits[..1]);
        if digits.len() > 1 {
            mantissa_out.push('.');
            mantissa_out.push_str(&digits[1..]);
        }
        format!("{sign}{mantissa_out}e{exponent:+03}")
    }
}

/// The hash of a token, the only form of a token the registry
/// ever stores.
pub fn token_hash(token: &str) -> String {
    let digest = Sha256::digest(token.as_bytes());
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn name_shape_sound(text: &str) -> bool {
    let count = text.chars().count();
    if count == 0 || count > NAME_MAX {
        return false;
    }
    let edge = |c: char| c.is_ascii_lowercase() || c.is_ascii_digit();
    text.chars().all(|c| edge(c) || c == '-' || c == '_')
        && text.chars().next().is_some_and(edge)
        && text.chars().last().is_some_and(edge)
}

fn invalid_name_message(kind: &str, value: &Value) -> Error {
    Error::InvalidName(format!(
        "invalid {kind} id {}, use one component of lowercase letters and digits \
         and hyphens and underscores, starting and ending with a letter or a \
         digit, at most {NAME_MAX} characters long",
        py_repr_value(value)
    ))
}

/// Refuse any item id or holder id outside the protocol name
/// rule, and return the name when it stands. The kind names the
/// role the message names, item or holder.
pub fn validate_name(value: &Value, kind: &str) -> Result<String> {
    match value {
        Value::Str(text) if name_shape_sound(text) => Ok(text.clone()),
        _ => Err(invalid_name_message(kind, value)),
    }
}

fn validate_name_str(text: &str, kind: &str) -> Result<()> {
    if name_shape_sound(text) {
        Ok(())
    } else {
        Err(invalid_name_message(kind, &Value::Str(text.to_string())))
    }
}

/// Refuse a time to live that is not an exact integer in the
/// protocol bound, and return it as a plain integer of seconds.
pub fn validate_ttl(value: &Value) -> Result<i64> {
    match value {
        Value::Int(number) if *number >= 1 && *number <= TTL_MAX as i128 => Ok(*number as i64),
        _ => Err(Error::InvalidTtl(format!(
            "invalid time to live {}, use a positive integer number of \
             seconds, at most {TTL_MAX}",
            py_repr_value(value)
        ))),
    }
}

fn validate_ttl_i64(ttl: i64) -> Result<i64> {
    validate_ttl(&Value::Int(ttl as i128))
}

/// Refuse a note that is not a string of at most the protocol
/// bound in characters, and return it when it stands.
pub fn validate_note(value: &Value) -> Result<String> {
    match value {
        Value::Str(text) if text.chars().count() <= NOTE_MAX => Ok(text.clone()),
        _ => Err(Error::InvalidRequest(format!(
            "invalid note, use a string of at most {NOTE_MAX} characters"
        ))),
    }
}

fn validate_note_str(note: &str) -> Result<()> {
    if note.chars().count() <= NOTE_MAX {
        Ok(())
    } else {
        Err(Error::InvalidRequest(format!(
            "invalid note, use a string of at most {NOTE_MAX} characters"
        )))
    }
}

/// Refuse a priority outside 0..=3 (DB-23).
pub fn validate_priority(value: &Value) -> Result<u8> {
    match value {
        Value::Int(number) if *number >= 0 && *number <= PRIORITY_MAX as i128 => Ok(*number as u8),
        _ => Err(Error::InvalidRequest(format!(
            "invalid priority {}, use an integer from 0 to {PRIORITY_MAX}",
            py_repr_value(value)
        ))),
    }
}

/// Refuse a max_takeovers value that is neither null nor an integer in 1..=1000 (DB-23).
pub fn validate_max_takeovers(value: &Value) -> Result<Option<u64>> {
    match value {
        Value::Null => Ok(None),
        Value::Int(number) if *number >= 1 && *number <= MAX_TAKEOVERS_MAX as i128 => {
            Ok(Some(*number as u64))
        }
        _ => Err(Error::InvalidRequest(format!(
            "invalid max_takeovers {}, use an integer from 1 to {MAX_TAKEOVERS_MAX}",
            py_repr_value(value)
        ))),
    }
}

/// Refuse a compaction through_seq that is not a positive 64-bit integer (DB-24).
pub fn validate_through_seq(value: &Value) -> Result<u64> {
    match value {
        Value::Int(number) if *number >= 1 && *number <= u64::MAX as i128 => Ok(*number as u64),
        _ => Err(Error::InvalidRequest(format!(
            "invalid through_seq {}, use a positive integer at most {}",
            py_repr_value(value),
            u64::MAX
        ))),
    }
}

fn normalize_tags(raw: &[String]) -> Result<Vec<String>> {
    for tag in raw {
        validate_name_str(tag, "tag")?;
    }
    let mut sorted = raw.to_vec();
    sorted.sort();
    sorted.dedup();
    if sorted.len() > TAGS_MAX {
        return Err(Error::InvalidRequest(format!(
            "too many tags, use at most {TAGS_MAX} tags per item"
        )));
    }
    Ok(sorted)
}

fn normalize_depends_on(item_id: &str, raw: &[String]) -> Result<Vec<String>> {
    for dep in raw {
        validate_name_str(dep, "item")?;
    }
    if raw.iter().any(|dep| dep == item_id) {
        return Err(Error::InvalidRequest(format!(
            "item {} cannot depend on itself",
            py_repr(item_id)
        )));
    }
    let mut sorted = raw.to_vec();
    sorted.sort();
    sorted.dedup();
    if sorted.len() > DEPENDS_ON_MAX {
        return Err(Error::InvalidRequest(format!(
            "too many dependencies, use at most {DEPENDS_ON_MAX} dependencies per item"
        )));
    }
    Ok(sorted)
}

/// Days from the civil date, the Howard Hinnant algorithm, with
/// the epoch at 1970-01-01.
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let shifted = if month <= 2 { year - 1 } else { year };
    let era = shifted.div_euclid(400);
    let yoe = shifted - era * 400;
    let mp = if month > 2 { month - 3 } else { month + 9 };
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

/// The civil date of a day count, the inverse of days_from_civil.
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    (if month <= 2 { year + 1 } else { year }, month, day)
}

fn is_leap_year(year: i64) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

fn days_in_month(year: i64, month: i64) -> i64 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if is_leap_year(year) {
                29
            } else {
                28
            }
        }
        _ => 0,
    }
}

/// The ISO weekday of a day count, Monday as 1. The epoch day
/// 1970-01-01 was a Thursday, ISO weekday 4.
fn iso_weekday(days: i64) -> i64 {
    (days + 3).rem_euclid(7) + 1
}

/// The number of ISO weeks in a year, 53 when the year starts on
/// a Thursday, or on a Wednesday in a leap year, and 52 otherwise.
fn iso_weeks_in_year(year: i64) -> i64 {
    let jan1 = iso_weekday(days_from_civil(year, 1, 1));
    if jan1 == 4 || (jan1 == 3 && is_leap_year(year)) {
        53
    } else {
        52
    }
}

/// One instant of time at microsecond precision, wall clock
/// fields with an offset from UTC. An instant with no offset is
/// naive, and the protocol refuses it wherever an instant is
/// taken. Comparisons between aware instants compare the instant
/// itself, never the wall clock fields, exactly as comparisons
/// between the aware datetimes of the Python implementation did.
#[derive(Debug, Clone, Copy, Eq)]
pub struct Instant {
    days: i64,
    secs: i64,
    micros: i64,
    offset_micros: Option<i64>,
}

impl PartialEq for Instant {
    /// Two aware instants are equal when they name the same
    /// instant, whatever offsets they carry, as the aware
    /// datetimes of the Python implementation compared. Two
    /// naive instants are equal when their wall clock fields
    /// agree, and a naive instant never equals an aware one.
    /// Equality therefore agrees with the Ord implementation
    /// wherever that implementation compares the instants
    /// themselves.
    fn eq(&self, other: &Self) -> bool {
        match (self.epoch_micros(), other.epoch_micros()) {
            (Some(left), Some(right)) => left == right,
            (None, None) => {
                self.days == other.days && self.secs == other.secs && self.micros == other.micros
            }
            _ => false,
        }
    }
}

const DAY_MICROS: i128 = 86_400_000_000;

impl Instant {
    /// Build an instant from civil fields and an offset in
    /// microseconds, or None when the fields do not stand. A None
    /// offset builds a naive instant.
    #[allow(clippy::too_many_arguments)]
    pub fn from_civil(
        year: i64,
        month: i64,
        day: i64,
        hour: i64,
        minute: i64,
        second: i64,
        micros: i64,
        offset_micros: Option<i64>,
    ) -> Option<Instant> {
        if !(1..=9999).contains(&year)
            || !(1..=12).contains(&month)
            || day < 1
            || day > days_in_month(year, month)
            || !(0..24).contains(&hour)
            || !(0..60).contains(&minute)
            || !(0..60).contains(&second)
            || !(0..1_000_000).contains(&micros)
        {
            return None;
        }
        if let Some(offset) = offset_micros {
            if offset.unsigned_abs() >= 86_400_000_000 {
                return None;
            }
        }
        Some(Instant {
            days: days_from_civil(year, month, day),
            secs: hour * 3600 + minute * 60 + second,
            micros,
            offset_micros,
        })
    }

    /// The instant the system clock reports now, in UTC. Used by
    /// the command line and by the Python binding when now is
    /// omitted.
    pub fn now_utc() -> Result<Instant> {
        Instant::from_clock_read(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH))
    }

    /// The instant a clock read names. A read that fails stands
    /// before the epoch, and no command may run on an instant the
    /// clock never gave (perfection finding DB-P13): the read
    /// answers a failure naming the case, at the failure exit,
    /// instead of the epoch the standing form fabricated.
    fn from_clock_read(
        read: std::result::Result<std::time::Duration, std::time::SystemTimeError>,
    ) -> Result<Instant> {
        let elapsed = read
            .map_err(|_| Error::Failure("the system clock stands before the epoch".to_string()))?;
        let total_micros = elapsed.as_secs() as i128 * 1_000_000 + elapsed.subsec_micros() as i128;
        let days = total_micros.div_euclid(DAY_MICROS);
        let rest = total_micros.rem_euclid(DAY_MICROS);
        Ok(Instant {
            days: days as i64,
            secs: (rest / 1_000_000) as i64,
            micros: (rest % 1_000_000) as i64,
            offset_micros: Some(0),
        })
    }

    pub fn is_naive(&self) -> bool {
        self.offset_micros.is_none()
    }

    /// The aware UTC instant at a count of microseconds since
    /// the epoch, or None when the count names a day count the
    /// type cannot hold, so a count this constructor accepts is
    /// a count it reads back whole and never a wrapped one.
    pub fn from_epoch_micros(micros: i128) -> Option<Instant> {
        let days = micros.div_euclid(DAY_MICROS);
        if days < i64::MIN as i128 || days > i64::MAX as i128 {
            return None;
        }
        Some(Instant {
            days: days as i64,
            secs: (micros.rem_euclid(DAY_MICROS) / 1_000_000) as i64,
            micros: (micros.rem_euclid(DAY_MICROS) % 1_000_000) as i64,
            offset_micros: Some(0),
        })
    }

    /// The instant itself as microseconds since the epoch, or
    /// None for a naive instant, which names no instant.
    pub fn epoch_micros(&self) -> Option<i128> {
        self.offset_micros.map(|offset| {
            self.days as i128 * DAY_MICROS + self.secs as i128 * 1_000_000 + self.micros as i128
                - offset as i128
        })
    }

    /// This instant shifted by a count of seconds, keeping the
    /// offset, the arithmetic the Python timedelta performed. At
    /// the magnitudes the protocol allows the shift cannot leave
    /// the representable range, and the one caller that checks a
    /// shift against the range uses the checked form below.
    pub fn plus_seconds(&self, seconds: i64) -> Instant {
        self.shift_micros(seconds as i128 * 1_000_000)
    }

    fn shift_micros(&self, delta: i128) -> Instant {
        let wall = self.days as i128 * DAY_MICROS
            + self.secs as i128 * 1_000_000
            + self.micros as i128
            + delta;
        Instant {
            days: wall.div_euclid(DAY_MICROS) as i64,
            secs: (wall.rem_euclid(DAY_MICROS) / 1_000_000) as i64,
            micros: (wall.rem_euclid(DAY_MICROS) % 1_000_000) as i64,
            offset_micros: self.offset_micros,
        }
    }

    /// The instant a time to live added to this one names, or
    /// None when the addition runs past the last representable
    /// instant, the refusal the protocol raises as an invalid time
    /// to live. The check runs on the UTC instant the stored
    /// rendering will carry, not on the wall clock fields the
    /// offset preserves, because a wall clock year inside the
    /// range can still name a UTC year outside it, and the stored
    /// form is the UTC rendering.
    fn expiry_after(&self, ttl: i64) -> Option<Instant> {
        let shifted = self.shift_micros(ttl as i128 * 1_000_000);
        let epoch = shifted.epoch_micros()?;
        if days_representable(epoch.div_euclid(DAY_MICROS)) {
            Some(shifted)
        } else {
            None
        }
    }
}

/// Whether a count of days since the epoch names a date inside
/// the representable years, 1 to 9999.
fn days_representable(days: i128) -> bool {
    days >= days_from_civil(1, 1, 1) as i128 && days <= days_from_civil(9999, 12, 31) as i128
}

impl PartialOrd for Instant {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Instant {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        match (self.epoch_micros(), other.epoch_micros()) {
            (Some(left), Some(right)) => left.cmp(&right),
            _ => {
                let left = self.days as i128 * DAY_MICROS
                    + self.secs as i128 * 1_000_000
                    + self.micros as i128;
                let right = other.days as i128 * DAY_MICROS
                    + other.secs as i128 * 1_000_000
                    + other.micros as i128;
                left.cmp(&right)
            }
        }
    }
}

fn validate_now(now: &Instant) -> Result<()> {
    let epoch = match now.epoch_micros() {
        Some(epoch) => epoch,
        None => {
            return Err(Error::InvalidRequest(
                "the current instant must be a timezone aware datetime".to_string(),
            ))
        }
    };
    // The instant is recorded in its UTC rendering, so it must
    // stand inside the representable years in UTC, whatever the
    // wall clock year its offset carries.
    if !days_representable(epoch.div_euclid(DAY_MICROS)) {
        return Err(Error::InvalidRequest(
            "the current instant falls outside the representable range in UTC".to_string(),
        ));
    }
    Ok(())
}

fn expiry_after(moment: &Instant, ttl: i64) -> Result<Instant> {
    moment.expiry_after(ttl).ok_or_else(|| {
        Error::InvalidTtl(format!(
            "a time to live of {ttl} seconds runs past the last representable instant"
        ))
    })
}

/// Render an instant the way the Python isoformat rendered it,
/// an aware instant converted to UTC with the UTC offset spelled
/// out, and a naive instant as its own wall clock fields with no
/// offset suffix. The separator is a T, and the fraction carries
/// six digits when the microsecond count is nonzero and none
/// when it is zero.
pub fn format_instant(moment: &Instant) -> String {
    let (days, secs, micros, suffix) = match moment.epoch_micros() {
        Some(epoch) => {
            let rest = epoch.rem_euclid(DAY_MICROS);
            (
                epoch.div_euclid(DAY_MICROS) as i64,
                (rest / 1_000_000) as i64,
                (rest % 1_000_000) as i64,
                "+00:00",
            )
        }
        None => (moment.days, moment.secs, moment.micros, ""),
    };
    let (year, month, day) = civil_from_days(days);
    let mut out = format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}",
        secs / 3600,
        (secs / 60) % 60,
        secs % 60
    );
    if micros != 0 {
        out.push_str(&format!(".{micros:06}"));
    }
    out.push_str(suffix);
    out
}

/// A cursor over the characters of a candidate instant.
struct Cursor {
    chars: Vec<char>,
    pos: usize,
}

impl Cursor {
    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn take(&mut self) -> Option<char> {
        let current = self.peek();
        if current.is_some() {
            self.pos += 1;
        }
        current
    }

    fn at_end(&self) -> bool {
        self.pos >= self.chars.len()
    }

    /// Take exactly count digits as a number, or fail.
    fn digits(&mut self, count: usize) -> Option<i64> {
        let mut value: i64 = 0;
        for _ in 0..count {
            let c = self.take()?;
            if !c.is_ascii_digit() {
                return None;
            }
            value = value * 10 + (c as i64 - '0' as i64);
        }
        Some(value)
    }

    fn expect(&mut self, wanted: char) -> Option<()> {
        if self.take() == Some(wanted) {
            Some(())
        } else {
            None
        }
    }

    /// Take a fraction separator and its digits, and return the
    /// fraction of a second in microseconds, truncated past six
    /// digits, with zero digits after the separator counting as
    /// no fraction where a zone designator follows it, and
    /// refusing the reading where the separator stands at the
    /// end of the text, the two cases the Python grammar
    /// separates. The fraction in the Python grammar always
    /// lands on the seconds, whatever unit it follows.
    fn fraction_micros(&mut self) -> Option<i64> {
        match self.peek() {
            Some('.') | Some(',') => {
                self.take();
                let mut micros: i64 = 0;
                let mut place: i64 = 100_000;
                let mut digits = 0;
                while let Some(c) = self.peek() {
                    if !c.is_ascii_digit() {
                        break;
                    }
                    self.take();
                    digits += 1;
                    if place > 0 {
                        micros += (c as i64 - '0' as i64) * place;
                        place /= 10;
                    }
                }
                if digits == 0 && self.at_end() {
                    None
                } else {
                    Some(micros)
                }
            }
            _ => Some(0),
        }
    }
}

/// The day count an ISO week date names, the Monday of its
/// first week stepped by whole weeks and weekdays, or None when
/// the week or the weekday stands outside the year.
fn week_date_days(year: i64, week: i64, weekday: i64) -> Option<i64> {
    if week < 1 || week > iso_weeks_in_year(year) || !(1..=7).contains(&weekday) {
        return None;
    }
    let jan4 = days_from_civil(year, 1, 4);
    let week_one_monday = jan4 - (iso_weekday(jan4) - 1);
    Some(week_one_monday + (week - 1) * 7 + (weekday - 1))
}

/// The day count a calendar date names, or None when the
/// year stands outside 1 to 9999, the month outside 1 to
/// 12, or the day outside the leap aware length of the
/// month. Both date families in parse_date close on this
/// one rule, so a grammar repair cannot correct one
/// spelling family and leave the other.
fn calendar_days(year: i64, month: i64, day: i64) -> Option<i64> {
    if !(1..=9999).contains(&year)
        || !(1..=12).contains(&month)
        || day < 1
        || day > days_in_month(year, month)
    {
        return None;
    }
    Some(days_from_civil(year, month, day))
}

fn parse_date(cursor: &mut Cursor) -> Option<i64> {
    // Extended form, YYYY-MM-DD and the week forms, or the basic
    // forms YYYYMMDD and YYYYWWW and YYYYWWWD. The year is
    // always four digits.
    if cursor.chars.get(4) == Some(&'-') {
        let year = cursor.digits(4)?;
        cursor.expect('-')?;
        if cursor.peek() == Some('W') {
            cursor.take();
            let week = cursor.digits(2)?;
            let weekday = if cursor.peek() == Some('-') {
                cursor.take();
                cursor.digits(1)?
            } else {
                1
            };
            return week_date_days(year, week, weekday);
        }
        let month = cursor.digits(2)?;
        cursor.expect('-')?;
        let day = cursor.digits(2)?;
        calendar_days(year, month, day)
    } else {
        let year = cursor.digits(4)?;
        if cursor.peek() == Some('W') {
            cursor.take();
            let week = cursor.digits(2)?;
            let weekday = if cursor.peek().is_some_and(|c| c.is_ascii_digit()) {
                cursor.digits(1)?
            } else {
                1
            };
            return week_date_days(year, week, weekday);
        }
        let month = cursor.digits(2)?;
        let day = cursor.digits(2)?;
        calendar_days(year, month, day)
    }
}

/// Read the minute and second components that may follow an
/// hour, the second and third components of a time and of an
/// offset alike. Each component stands in colon form after its
/// colon or in packed form where a digit stands, and a
/// component that does not stand reads as zero. The time after
/// a date and the offset after its sign carry their trailing
/// components in this one shape, so the shape stands here once
/// and both readers call it.
fn minute_second_components(cursor: &mut Cursor) -> Option<(i64, i64)> {
    let mut minute = 0i64;
    let mut second = 0i64;
    if cursor.peek() == Some(':') {
        cursor.take();
        minute = cursor.digits(2)?;
        if cursor.peek() == Some(':') {
            cursor.take();
            second = cursor.digits(2)?;
        }
    } else if cursor.peek().is_some_and(|c| c.is_ascii_digit()) {
        minute = cursor.digits(2)?;
        if cursor.peek().is_some_and(|c| c.is_ascii_digit()) {
            second = cursor.digits(2)?;
        }
    }
    Some((minute, second))
}

/// Parse the time and offset after a date and its separator, or
/// the bare end of the string, and build the instant. The
/// separator has already been consumed by the caller.
fn parse_time_and_offset(cursor: &mut Cursor, days: i64) -> Option<Instant> {
    let hour = cursor.digits(2)?;
    let (minute, second) = minute_second_components(cursor)?;
    let micros = cursor.fraction_micros()?;
    if hour > 23 || minute > 59 || second > 59 {
        return None;
    }
    if cursor.at_end() {
        return Instant::from_civil_fields(days, hour, minute, second, micros, None);
    }
    let offset_micros = parse_offset(cursor)?;
    if !cursor.at_end() {
        return None;
    }
    Instant::from_civil_fields(days, hour, minute, second, micros, Some(offset_micros))
}

/// Parse a zone designator, Z or a signed offset whose components
/// normalize inside a day, and return the offset in microseconds.
fn parse_offset(cursor: &mut Cursor) -> Option<i64> {
    match cursor.take()? {
        'Z' => Some(0),
        sign @ ('+' | '-') => {
            let hours = cursor.digits(2)?;
            let (minutes, seconds) = minute_second_components(cursor)?;
            let micros = cursor.fraction_micros()?;
            let total_seconds = hours * 3600 + minutes * 60 + seconds;
            let total = (total_seconds as i128) * 1_000_000 + micros as i128;
            if total >= 86_400_000_000 {
                return None;
            }
            let signed = if sign == '-' { -total } else { total };
            Some(signed as i64)
        }
        _ => None,
    }
}

impl Instant {
    fn from_civil_fields(
        days: i64,
        hour: i64,
        minute: i64,
        second: i64,
        micros: i64,
        offset_micros: Option<i64>,
    ) -> Option<Instant> {
        if let Some(offset) = offset_micros {
            if offset.unsigned_abs() >= 86_400_000_000 {
                return None;
            }
        }
        let (year, _, _) = civil_from_days(days);
        if !(1..=9999).contains(&year) {
            return None;
        }
        Some(Instant {
            days,
            secs: hour * 3600 + minute * 60 + second,
            micros,
            offset_micros,
        })
    }
}

/// Parse stored instant text against the family the Python
/// fromisoformat accepted, as probed against that runtime before
/// the port was written. Extended and basic and week dates, any
/// single character as the separator between the date and the
/// time (a digit separator only after an extended date, where
/// the Python parser took one), times with or without colons,
/// fractions on a dot or a comma truncated past six digits, and
/// offsets under a Z or signed components that normalize inside
/// a day. An Err is the unparseable case, and an Ok instant with
/// no offset is the naive case, and the two stay distinct
/// because the corruption messages name them distinctly. The
/// parser is public so the Python binding can hand it the
/// instant text a Python caller passes, and the one parser
/// stays the only parser. The unit error is deliberate, because
/// the parse carries exactly two outcomes the callers must not
/// blur, a refusal and a naive instant, and that distinction
/// rides on the Ok value rather than on an error payload.
#[allow(clippy::result_unit_err)]
pub fn parse_instant(text: &str) -> std::result::Result<Instant, ()> {
    let mut cursor = Cursor {
        chars: text.chars().collect(),
        pos: 0,
    };
    let days = parse_date(&mut cursor).ok_or(())?;
    if cursor.at_end() {
        return Instant::from_civil_fields(days, 0, 0, 0, 0, None).ok_or(());
    }
    // One separator character stands between the date and the
    // time. After a basic date the Python parser refused a digit
    // separator outright, so one is refused here too. After an
    // extended or week date any single character separates.
    let basic = cursor.chars.get(4) != Some(&'-');
    let separator = cursor.take().ok_or(())?;
    if basic && separator.is_ascii_digit() {
        return Err(());
    }
    parse_time_and_offset(&mut cursor, days).ok_or(())
}

/// Parse a stored instant field, raising the corruption error of
/// the case that fails, exactly as the Python reader raised it.
fn parse_stored_instant(value: &Value) -> Result<Instant> {
    let text = match value {
        Value::Str(text) => text,
        other => {
            return Err(corrupt(format!(
                "an instant field holds {} instead of text",
                py_repr_value(other)
            )))
        }
    };
    match parse_instant(text) {
        Err(()) => Err(corrupt(format!(
            "an instant field holds unparseable text {}",
            py_repr(text)
        ))),
        Ok(instant) if instant.is_naive() => Err(corrupt(format!(
            "an instant field holds a naive instant {}",
            py_repr(text)
        ))),
        Ok(instant) => Ok(instant),
    }
}

/// The strict JSON reader. A hand written Visitor over serde_json
/// whose visit_map sees every key as it arrives and refuses a
/// repeat at any level, because serde_json alone keeps the last
/// value of a repeated key and says nothing, where the Python
/// reader's object pairs hook refused it. Numbers keep their
/// integer value at 128 bit width when they arrive as integers; a
/// number with a fraction or an exponent lands in the Float
/// variant and fails every integer check, as it did under the
/// Python type checks. A duplicate key is reported through a
/// marker in the error text that the reader maps back to the
/// corruption message.
impl<'de> serde::Deserialize<'de> for Value {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Value, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct StrictVisitor;

        impl<'de> serde::de::Visitor<'de> for StrictVisitor {
            type Value = Value;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("a JSON value")
            }

            fn visit_bool<E>(self, value: bool) -> std::result::Result<Value, E> {
                Ok(Value::Bool(value))
            }

            fn visit_i64<E>(self, value: i64) -> std::result::Result<Value, E> {
                Ok(Value::Int(value as i128))
            }

            fn visit_u64<E>(self, value: u64) -> std::result::Result<Value, E> {
                Ok(Value::Int(value as i128))
            }

            fn visit_i128<E>(self, value: i128) -> std::result::Result<Value, E>
            where
                E: serde::de::Error,
            {
                Ok(Value::Int(value))
            }

            fn visit_f64<E>(self, value: f64) -> std::result::Result<Value, E> {
                Ok(Value::Float(value))
            }

            fn visit_str<E>(self, value: &str) -> std::result::Result<Value, E>
            where
                E: serde::de::Error,
            {
                Ok(Value::Str(value.to_string()))
            }

            fn visit_string<E>(self, value: String) -> std::result::Result<Value, E> {
                Ok(Value::Str(value))
            }

            fn visit_none<E>(self) -> std::result::Result<Value, E> {
                Ok(Value::Null)
            }

            fn visit_unit<E>(self) -> std::result::Result<Value, E> {
                Ok(Value::Null)
            }

            fn visit_some<D>(self, deserializer: D) -> std::result::Result<Value, D::Error>
            where
                D: serde::Deserializer<'de>,
            {
                <Value as serde::Deserialize>::deserialize(deserializer)
            }

            fn visit_seq<A>(self, mut seq: A) -> std::result::Result<Value, A::Error>
            where
                A: serde::de::SeqAccess<'de>,
            {
                let mut items = Vec::new();
                while let Some(item) = seq.next_element()? {
                    items.push(item);
                }
                Ok(Value::Array(items))
            }

            fn visit_map<A>(self, mut map: A) -> std::result::Result<Value, A::Error>
            where
                A: serde::de::MapAccess<'de>,
            {
                let mut entries: Vec<(String, Value)> = Vec::new();
                let mut seen: HashSet<String> = HashSet::new();
                while let Some(key) = map.next_key::<String>()? {
                    if !seen.insert(key.clone()) {
                        return Err(serde::de::Error::custom(format!(
                            "dibs-duplicate-key\x1f{key}"
                        )));
                    }
                    let value = map.next_value()?;
                    entries.push((key, value));
                }
                Ok(Value::Object(entries))
            }
        }

        deserializer.deserialize_any(StrictVisitor)
    }
}

fn duplicate_key_of(error_text: &str) -> Option<String> {
    let marker = "dibs-duplicate-key\x1f";
    let start = error_text.find(marker)? + marker.len();
    let rest = &error_text[start..];
    // The position suffix serde_json appends stands last in the
    // text, so the cut takes the last occurrence of the separator.
    // A key that itself carries the separator is recovered whole,
    // where cutting at the first occurrence named only the part
    // of the key standing before it.
    let end = rest.rfind(" at line ").unwrap_or(rest.len());
    Some(rest[..end].to_string())
}

/// The recovered duplicate key as the corruption message names it.
/// A key carrying a character that does not print renders through
/// py_repr, the escaped rendering the rest of the reader uses, so
/// the refusal stays one line and names the character visibly. A
/// key that prints whole stands raw, as the standing pins assert.
fn duplicate_key_display(key: &str) -> String {
    if key.chars().all(py_printable) {
        key.to_string()
    } else {
        py_repr(key)
    }
}

/// One item of the backlog, in the shape the registry stores it.
#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    pub id: String,
    pub note: String,
    pub status: String,
    pub holder: Option<String>,
    pub token_hash: Option<String>,
    pub epoch: i128,
    pub expires_at: Option<String>,
}

/// One recorded event of the claim history.
#[derive(Debug, Clone, PartialEq)]
pub struct Event {
    pub seq: i128,
    pub kind: String,
    pub item: String,
    pub holder: Option<String>,
    pub at: String,
    pub epoch: i128,
    pub previous_holder: Option<String>,
}

/// The registry document, items and history together.
#[derive(Debug, Clone, PartialEq)]
pub struct Document {
    pub items: Vec<Item>,
    pub history: Vec<Event>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct BaselineEntry {
    id: String,
    status: String,
    holder: Option<String>,
    epoch: i128,
    takeover_count: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Compaction {
    through_seq: u64,
    archived_events_sha256: String,
    compacted_at: String,
    baseline: Vec<BaselineEntry>,
}

#[derive(Debug, Clone, PartialEq)]
struct StoredItem {
    core: Item,
    priority: u8,
    tags: Vec<String>,
    depends_on: Vec<String>,
    max_takeovers: Option<u64>,
    takeover_count: u64,
}

#[derive(Debug, Clone, PartialEq)]
struct RegistryDoc {
    is_v2: bool,
    compaction: Option<Compaction>,
    items: Vec<StoredItem>,
    history: Vec<Event>,
}

fn empty_registry_doc() -> RegistryDoc {
    RegistryDoc {
        is_v2: false,
        compaction: None,
        items: Vec::new(),
        history: Vec::new(),
    }
}

/// One row of the board, an item with its lease state evaluated
/// at the read instant.
#[derive(Debug, Clone, PartialEq)]
pub struct BoardRow {
    pub id: String,
    pub note: String,
    pub status: String,
    pub holder: Option<String>,
    pub expires_at: Option<String>,
    pub epoch: i128,
}

/// The one return of a claim, and the only place a token appears.
#[derive(Debug, Clone, PartialEq)]
pub struct ClaimGranted {
    pub item: String,
    pub holder: String,
    pub token: String,
    pub epoch: i128,
    pub expires_at: Instant,
}

/// Specification for one item offered individually or in a batch (DB-18, DB-22, DB-23).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct OfferSpec {
    pub id: String,
    pub note: String,
    pub priority: u8,
    pub tags: Vec<String>,
    pub depends_on: Vec<String>,
    pub max_takeovers: Option<u64>,
}

/// Alias for single-item `/2` offer options (`Registry::offer_with`).
pub type OfferOptions = OfferSpec;

impl OfferSpec {
    pub fn new(id: impl Into<String>, note: impl Into<String>) -> OfferSpec {
        OfferSpec {
            id: id.into(),
            note: note.into(),
            priority: PRIORITY_DEFAULT,
            tags: Vec::new(),
            depends_on: Vec::new(),
            max_takeovers: None,
        }
    }

    pub fn with_priority(mut self, priority: u8) -> OfferSpec {
        self.priority = priority;
        self
    }

    pub fn with_tags<I, S>(mut self, tags: I) -> OfferSpec
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.tags = tags.into_iter().map(Into::into).collect();
        self
    }

    pub fn with_depends_on<I, S>(mut self, depends_on: I) -> OfferSpec
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.depends_on = depends_on.into_iter().map(Into::into).collect();
        self
    }

    pub fn with_max_takeovers(mut self, max_takeovers: Option<u64>) -> OfferSpec {
        self.max_takeovers = max_takeovers;
        self
    }
}

/// Detailed single item view with lease state and takeover count
/// evaluated at the read instant (DB-19, DB-22, DB-23).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemDetail {
    pub id: String,
    pub note: String,
    pub status: String,
    pub effective_status: String,
    pub priority: u8,
    pub tags: Vec<String>,
    pub depends_on: Vec<String>,
    pub holder: Option<String>,
    pub expires_at: Option<String>,
    pub epoch: i128,
    pub takeover_count: u64,
    pub max_takeovers: Option<u64>,
}

/// Summary of a history compaction run (DB-24).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompactSummary {
    pub through_seq: u64,
    pub archived_count: u64,
    pub remaining_count: u64,
    pub archived_events_sha256: String,
    pub archive_path: PathBuf,
}

/// Parse JSON Lines input for `offer-batch` into validated item
/// specifications before touching disk (DB-18, DB-22, DB-23).
pub fn parse_offer_batch_jsonl(text: &str) -> Result<Vec<OfferSpec>> {
    const BATCH_KEYS: [&str; 6] = [
        "id",
        "note",
        "priority",
        "tags",
        "depends_on",
        "max_takeovers",
    ];
    let mut specs = Vec::new();
    for (index, raw_line) in text.lines().enumerate() {
        let line_no = index + 1;
        if raw_line.trim().is_empty() {
            continue;
        }
        let value: Value = serde_json::from_str(raw_line).map_err(|err| {
            let err_text = err.to_string();
            if let Some(key) = duplicate_key_of(&err_text) {
                Error::InvalidRequest(format!(
                    "offer batch line {line_no} carries duplicate key {}",
                    duplicate_key_display(&key)
                ))
            } else {
                Error::InvalidRequest(format!(
                    "offer batch line {line_no} is not valid JSON, {err_text}"
                ))
            }
        })?;
        let Value::Object(pairs) = value else {
            return Err(Error::InvalidRequest(format!(
                "offer batch line {line_no} is not a JSON object"
            )));
        };
        for (key, _) in &pairs {
            if !BATCH_KEYS.contains(&key.as_str()) {
                return Err(Error::InvalidRequest(format!(
                    "offer batch line {line_no} carries unknown field {}",
                    py_repr(key)
                )));
            }
        }
        let Some(id_val) = object_get(&pairs, "id") else {
            return Err(Error::InvalidRequest(format!(
                "offer batch line {line_no} is missing required field 'id'"
            )));
        };
        let id = validate_name(id_val, "item")?;
        let note = match object_get(&pairs, "note") {
            Some(note_val) => validate_note(note_val)?,
            None => String::new(),
        };
        let priority = match object_get(&pairs, "priority") {
            Some(prio_val) => validate_priority(prio_val)?,
            None => PRIORITY_DEFAULT,
        };
        let tags = match object_get(&pairs, "tags") {
            None => Vec::new(),
            Some(Value::Array(arr)) => {
                let mut raw_tags = Vec::with_capacity(arr.len());
                for elem in arr {
                    raw_tags.push(validate_name(elem, "tag")?);
                }
                normalize_tags(&raw_tags)?
            }
            Some(other) => {
                return Err(Error::InvalidRequest(format!(
                    "offer batch line {line_no} field 'tags' holds {}, use an array of tag names",
                    py_repr_value(other)
                )));
            }
        };
        let depends_on = match object_get(&pairs, "depends_on") {
            None => Vec::new(),
            Some(Value::Array(arr)) => {
                let mut raw_deps = Vec::with_capacity(arr.len());
                for elem in arr {
                    raw_deps.push(validate_name(elem, "item")?);
                }
                normalize_depends_on(&id, &raw_deps)?
            }
            Some(other) => {
                return Err(Error::InvalidRequest(format!(
                    "offer batch line {line_no} field 'depends_on' holds {}, use an array of item ids",
                    py_repr_value(other)
                )));
            }
        };
        let max_takeovers = match object_get(&pairs, "max_takeovers") {
            None => None,
            Some(cap_val) => validate_max_takeovers(cap_val)?,
        };
        specs.push(OfferSpec {
            id,
            note,
            priority,
            tags,
            depends_on,
            max_takeovers,
        });
    }
    if specs.is_empty() {
        return Err(Error::InvalidRequest(
            "offer batch requires at least one item".to_string(),
        ));
    }
    Ok(specs)
}

fn object_get<'a>(pairs: &'a [(String, Value)], key: &str) -> Option<&'a Value> {
    pairs
        .iter()
        .find(|(seen, _)| seen == key)
        .map(|(_, value)| value)
}

fn keys_exact(pairs: &[(String, Value)], wanted: &[&str]) -> bool {
    pairs.len() == wanted.len()
        && wanted
            .iter()
            .all(|key| pairs.iter().any(|(seen, _)| seen == key))
}

fn validate_stored_name(value: &Value, field: &str, subject: &str) -> Result<()> {
    match value {
        Value::Str(text) if name_shape_sound(text) => Ok(()),
        _ => Err(corrupt(format!(
            "{subject} holds an invalid {field} {}",
            py_repr_value(value)
        ))),
    }
}

/// The text of a stored string field a check immediately
/// before has proven to be a string, validate_stored_name
/// for the name fields and parse_stored_instant for the
/// instant fields. The proof obligation stands on the
/// callers, and the unreachable arm of the extraction
/// stands here once instead of at every field, so a check
/// weakened later cannot turn one field's extraction into
/// a panic on a stored document the reader is refusing as
/// corrupt.
fn stored_string(value: &Value) -> String {
    match value {
        Value::Str(text) => text.clone(),
        _ => unreachable!(),
    }
}

fn validate_item_core_fields(pairs: &[(String, Value)]) -> Result<Item> {
    let id_value = object_get(pairs, "id").unwrap();
    validate_stored_name(id_value, "item id", "an item")?;
    let id = stored_string(id_value);
    let note = match object_get(pairs, "note").unwrap() {
        Value::Str(text) if text.chars().count() <= NOTE_MAX => text.clone(),
        _ => {
            return Err(corrupt(format!(
                "item {} holds an invalid note",
                py_repr(&id)
            )))
        }
    };
    let status = match object_get(pairs, "status").unwrap() {
        Value::Str(text) if ITEM_STATUSES.contains(&text.as_str()) => text.clone(),
        other => {
            return Err(corrupt(format!(
                "item {} holds an unknown status {}",
                py_repr(&id),
                py_repr_value(other)
            )))
        }
    };
    let holder = match object_get(pairs, "holder").unwrap() {
        Value::Null => None,
        other => {
            validate_stored_name(other, "holder id", "an item")?;
            Some(stored_string(other))
        }
    };
    let token_hash = match object_get(pairs, "token_hash").unwrap() {
        Value::Null => None,
        Value::Str(text) if is_token_hash(text) => Some(text.clone()),
        _ => {
            return Err(corrupt(format!(
                "item {} holds an invalid token hash",
                py_repr(&id)
            )))
        }
    };
    let epoch = match object_get(pairs, "epoch").unwrap() {
        Value::Int(number) if *number >= 0 => *number,
        _ => {
            return Err(corrupt(format!(
                "item {} holds an invalid epoch",
                py_repr(&id)
            )))
        }
    };
    let expires_value = object_get(pairs, "expires_at").unwrap();
    let expires_at = match expires_value {
        Value::Null => None,
        other => {
            parse_stored_instant(other)?;
            Some(stored_string(other))
        }
    };
    let lease_fields = (holder.is_some(), token_hash.is_some(), expires_at.is_some());
    if status == "claimed" {
        if !lease_fields.0 || !lease_fields.1 || !lease_fields.2 {
            return Err(corrupt(format!(
                "item {} stands claimed without a full lease",
                py_repr(&id)
            )));
        }
        if epoch < 1 {
            return Err(corrupt(format!(
                "item {} stands claimed with epoch 0",
                py_repr(&id)
            )));
        }
    } else if lease_fields.0 || lease_fields.1 || lease_fields.2 {
        return Err(corrupt(format!(
            "item {} stands {} while holding a lease",
            py_repr(&id),
            status
        )));
    }
    Ok(Item {
        id,
        note,
        status,
        holder,
        token_hash,
        epoch,
        expires_at,
    })
}

fn validate_item(value: &Value) -> Result<Item> {
    let pairs = match value {
        Value::Object(pairs) if keys_exact(pairs, &ITEM_KEYS) => pairs,
        _ => {
            return Err(corrupt(
                "an item does not carry exactly the recorded fields".to_string(),
            ))
        }
    };
    validate_item_core_fields(pairs)
}

fn validate_item_v2(value: &Value) -> Result<StoredItem> {
    let pairs = match value {
        Value::Object(pairs) if keys_exact(pairs, &ITEM_KEYS_V2) => pairs,
        _ => {
            return Err(corrupt(
                "an item does not carry exactly the recorded fields".to_string(),
            ))
        }
    };
    let core = validate_item_core_fields(pairs)?;
    let id = &core.id;
    let priority = match object_get(pairs, "priority").unwrap() {
        Value::Int(number) if *number >= 0 && *number <= PRIORITY_MAX as i128 => *number as u8,
        _ => {
            return Err(corrupt(format!(
                "item {} holds an invalid priority",
                py_repr(id)
            )))
        }
    };
    let tags = match object_get(pairs, "tags").unwrap() {
        Value::Array(arr) if arr.len() <= TAGS_MAX => {
            let mut tags: Vec<String> = Vec::with_capacity(arr.len());
            for elem in arr {
                validate_stored_name(elem, "tag id", "an item")?;
                let tag = stored_string(elem);
                if let Some(prev) = tags.last() {
                    if prev.as_str() >= tag.as_str() {
                        return Err(corrupt(format!(
                            "item {} holds unsorted or duplicate tags",
                            py_repr(id)
                        )));
                    }
                }
                tags.push(tag);
            }
            tags
        }
        _ => {
            return Err(corrupt(format!(
                "item {} holds an invalid tags list",
                py_repr(id)
            )))
        }
    };
    let depends_on = match object_get(pairs, "depends_on").unwrap() {
        Value::Array(arr) if arr.len() <= DEPENDS_ON_MAX => {
            let mut deps: Vec<String> = Vec::with_capacity(arr.len());
            for elem in arr {
                validate_stored_name(elem, "dependency id", "an item")?;
                let dep = stored_string(elem);
                if &dep == id {
                    return Err(corrupt(format!("item {} depends on itself", py_repr(id))));
                }
                if let Some(prev) = deps.last() {
                    if prev.as_str() >= dep.as_str() {
                        return Err(corrupt(format!(
                            "item {} holds unsorted or duplicate dependencies",
                            py_repr(id)
                        )));
                    }
                }
                deps.push(dep);
            }
            deps
        }
        _ => {
            return Err(corrupt(format!(
                "item {} holds an invalid depends_on list",
                py_repr(id)
            )))
        }
    };
    let max_takeovers = match object_get(pairs, "max_takeovers").unwrap() {
        Value::Null => None,
        Value::Int(number) if *number >= 1 && *number <= MAX_TAKEOVERS_MAX as i128 => {
            Some(*number as u64)
        }
        _ => {
            return Err(corrupt(format!(
                "item {} holds an invalid max_takeovers",
                py_repr(id)
            )))
        }
    };
    let takeover_count = match object_get(pairs, "takeover_count").unwrap() {
        Value::Int(number) if *number >= 0 && *number <= u64::MAX as i128 => *number as u64,
        _ => {
            return Err(corrupt(format!(
                "item {} holds an invalid takeover_count",
                py_repr(id)
            )))
        }
    };
    if core.epoch == 0 && takeover_count != 0 {
        return Err(corrupt(format!(
            "item {} holds takeover_count {} at epoch 0",
            py_repr(id),
            takeover_count
        )));
    }
    if core.epoch >= 1 && (takeover_count as i128) >= core.epoch {
        return Err(corrupt(format!(
            "item {} holds takeover_count {} at epoch {}",
            py_repr(id),
            takeover_count,
            core.epoch
        )));
    }
    if let Some(cap) = max_takeovers {
        if takeover_count > cap {
            return Err(corrupt(format!(
                "item {} holds takeover_count {} past max_takeovers {cap}",
                py_repr(id),
                takeover_count
            )));
        }
    }
    Ok(StoredItem {
        core,
        priority,
        tags,
        depends_on,
        max_takeovers,
        takeover_count,
    })
}

fn is_token_hash(text: &str) -> bool {
    text.len() == 64
        && text
            .chars()
            .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c))
}

fn validate_compaction(value: &Value) -> Result<Option<Compaction>> {
    if value == &Value::Null {
        return Ok(None);
    }
    let pairs = match value {
        Value::Object(pairs) if keys_exact(pairs, &COMPACTION_KEYS) => pairs,
        _ => {
            return Err(corrupt(
                "the compaction field does not carry exactly the recorded fields".to_string(),
            ))
        }
    };
    let through_seq = match object_get(pairs, "through_seq").unwrap() {
        Value::Int(number) if *number >= 1 && *number <= u64::MAX as i128 => *number as u64,
        _ => {
            return Err(corrupt(
                "compaction holds an invalid through_seq".to_string(),
            ))
        }
    };
    let archived_events_sha256 = match object_get(pairs, "archived_events_sha256").unwrap() {
        Value::Str(text) if is_token_hash(text) => text.clone(),
        _ => {
            return Err(corrupt(
                "compaction holds an invalid archived_events_sha256".to_string(),
            ))
        }
    };
    let compacted_raw = object_get(pairs, "compacted_at").unwrap();
    parse_stored_instant(compacted_raw)?;
    let compacted_at = stored_string(compacted_raw);
    let baseline_values = match object_get(pairs, "baseline").unwrap() {
        Value::Array(entries) => entries,
        _ => {
            return Err(corrupt(
                "compaction baseline field is not a list".to_string(),
            ))
        }
    };
    let mut baseline: Vec<BaselineEntry> = Vec::with_capacity(baseline_values.len());
    for entry_val in baseline_values {
        let entry_pairs = match entry_val {
            Value::Object(pairs) if keys_exact(pairs, &BASELINE_KEYS) => pairs,
            _ => {
                return Err(corrupt(
                    "a compaction baseline entry does not carry exactly the recorded fields"
                        .to_string(),
                ))
            }
        };
        let id_val = object_get(entry_pairs, "id").unwrap();
        validate_stored_name(id_val, "item id", "a compaction baseline entry")?;
        let id = stored_string(id_val);
        if let Some(prev) = baseline.last() {
            if prev.id.as_str() >= id.as_str() {
                return Err(corrupt(
                    "the compaction baseline is not sorted without duplicates".to_string(),
                ));
            }
        }
        let status = match object_get(entry_pairs, "status").unwrap() {
            Value::Str(text) if ITEM_STATUSES.contains(&text.as_str()) => text.clone(),
            other => {
                return Err(corrupt(format!(
                    "compaction baseline entry {} holds an unknown status {}",
                    py_repr(&id),
                    py_repr_value(other)
                )))
            }
        };
        let holder = match object_get(entry_pairs, "holder").unwrap() {
            Value::Null => None,
            other => {
                validate_stored_name(other, "holder id", "a compaction baseline entry")?;
                Some(stored_string(other))
            }
        };
        let epoch = match object_get(entry_pairs, "epoch").unwrap() {
            Value::Int(number) if *number >= 0 && *number <= u64::MAX as i128 => *number,
            _ => {
                return Err(corrupt(format!(
                    "compaction baseline entry {} holds an invalid epoch",
                    py_repr(&id)
                )))
            }
        };
        let takeover_count = match object_get(entry_pairs, "takeover_count").unwrap() {
            Value::Int(number) if *number >= 0 && *number <= u64::MAX as i128 => *number as u64,
            _ => {
                return Err(corrupt(format!(
                    "compaction baseline entry {} holds an invalid takeover_count",
                    py_repr(&id)
                )))
            }
        };
        if status == "claimed" {
            if holder.is_none() {
                return Err(corrupt(format!(
                    "compaction baseline entry {} stands claimed without a holder",
                    py_repr(&id)
                )));
            }
            if epoch < 1 {
                return Err(corrupt(format!(
                    "compaction baseline entry {} stands claimed with epoch 0",
                    py_repr(&id)
                )));
            }
        } else if holder.is_some() {
            return Err(corrupt(format!(
                "compaction baseline entry {} stands {} while naming a holder",
                py_repr(&id),
                status
            )));
        }
        if epoch == 0 && takeover_count != 0 {
            return Err(corrupt(format!(
                "compaction baseline entry {} holds takeover_count {} at epoch 0",
                py_repr(&id),
                takeover_count
            )));
        }
        if epoch >= 1 && (takeover_count as i128) >= epoch {
            return Err(corrupt(format!(
                "compaction baseline entry {} holds takeover_count {} at epoch {}",
                py_repr(&id),
                takeover_count,
                epoch
            )));
        }
        baseline.push(BaselineEntry {
            id,
            status,
            holder,
            epoch,
            takeover_count,
        });
    }
    Ok(Some(Compaction {
        through_seq,
        archived_events_sha256,
        compacted_at,
        baseline,
    }))
}

fn validate_event_with(value: &Value, expected_seq: i128, allow_v2: bool) -> Result<Event> {
    let pairs = match value {
        Value::Object(pairs) if keys_exact(pairs, &EVENT_KEYS) => pairs,
        _ => {
            return Err(corrupt(
                "a history event does not carry exactly the recorded fields".to_string(),
            ))
        }
    };
    let seq = match object_get(pairs, "seq").unwrap() {
        Value::Int(number) if *number == expected_seq && *number <= u64::MAX as i128 => *number,
        _ => {
            return Err(corrupt(
                "the history sequence does not grow by 1 from 1 without gaps".to_string(),
            ))
        }
    };
    let allowed_kinds: &[&str] = if allow_v2 {
        &EVENT_KINDS_V2
    } else {
        &EVENT_KINDS
    };
    let kind = match object_get(pairs, "kind").unwrap() {
        Value::Str(text) if allowed_kinds.contains(&text.as_str()) => text.clone(),
        other => {
            return Err(corrupt(format!(
                "a history event holds an unknown kind {}",
                py_repr_value(other)
            )))
        }
    };
    let item_value = object_get(pairs, "item").unwrap();
    validate_stored_name(item_value, "item id", "a history event")?;
    let item = stored_string(item_value);
    let holder_value = object_get(pairs, "holder").unwrap();
    let holder = if kind == "offered" || kind == "reset" {
        if holder_value != &Value::Null {
            return Err(corrupt(format!("a {kind} event names a holder")));
        }
        None
    } else {
        if holder_value == &Value::Null {
            return Err(corrupt(format!("a {kind} event names no holder")));
        }
        validate_stored_name(holder_value, "holder id", "a history event")?;
        Some(stored_string(holder_value))
    };
    let previous_value = object_get(pairs, "previous_holder").unwrap();
    let previous_holder = if kind == "taken_over" {
        if previous_value == &Value::Null {
            return Err(corrupt(
                "a taken over event names no previous holder".to_string(),
            ));
        }
        validate_stored_name(previous_value, "holder id", "a history event")?;
        Some(stored_string(previous_value))
    } else if kind == "reset" {
        match previous_value {
            Value::Null => None,
            other => {
                validate_stored_name(other, "holder id", "a history event")?;
                Some(stored_string(other))
            }
        }
    } else {
        if previous_value != &Value::Null {
            return Err(corrupt(
                "an event that is not a takeover names a previous holder".to_string(),
            ));
        }
        None
    };
    let at_raw = object_get(pairs, "at").unwrap();
    parse_stored_instant(at_raw)?;
    let at = stored_string(at_raw);
    let epoch = match object_get(pairs, "epoch").unwrap() {
        Value::Int(number) if *number >= 0 => *number,
        _ => {
            return Err(corrupt(
                "a history event holds an invalid epoch".to_string(),
            ))
        }
    };
    if kind == "offered" && epoch != 0 {
        return Err(corrupt(
            "an offered event carries an epoch other than 0".to_string(),
        ));
    }
    if kind != "offered" && epoch < 1 {
        return Err(corrupt(format!("a {kind} event carries epoch 0")));
    }
    if kind == "reset" && epoch < 2 {
        return Err(corrupt(
            "a reset event carries an epoch below 2".to_string(),
        ));
    }
    Ok(Event {
        seq,
        kind,
        item,
        holder,
        at,
        epoch,
        previous_holder,
    })
}

fn validate_event(value: &Value, position: usize) -> Result<Event> {
    validate_event_with(value, position as i128 + 1, false)
}

/// Iterative heap-stack three-color DFS cycle check (DB-22).
fn has_dependency_cycle(adj: &[Vec<usize>]) -> bool {
    let n = adj.len();
    // 0 = unvisited, 1 = visiting (on active DFS stack), 2 = visited (acyclic)
    let mut color = vec![0u8; n];
    let mut stack: Vec<(usize, usize)> = Vec::new();
    for start in 0..n {
        if color[start] != 0 {
            continue;
        }
        color[start] = 1;
        stack.push((start, 0));
        while let Some((node, edge_idx)) = stack.last_mut() {
            let u = *node;
            if *edge_idx < adj[u].len() {
                let v = adj[u][*edge_idx];
                *edge_idx += 1;
                match color[v] {
                    1 => return true,
                    0 => {
                        color[v] = 1;
                        stack.push((v, 0));
                    }
                    _ => {}
                }
            } else {
                color[u] = 2;
                stack.pop();
            }
        }
    }
    false
}

fn validate_stored_dag(items: &[StoredItem]) -> Result<()> {
    let mut index_by_id: HashMap<&str, usize> = HashMap::with_capacity(items.len());
    for (idx, item) in items.iter().enumerate() {
        index_by_id.insert(item.core.id.as_str(), idx);
    }
    let mut adj: Vec<Vec<usize>> = Vec::with_capacity(items.len());
    for item in items {
        let mut edges = Vec::with_capacity(item.depends_on.len());
        for dep in &item.depends_on {
            let Some(&dep_idx) = index_by_id.get(dep.as_str()) else {
                return Err(corrupt(format!(
                    "item {} depends on {}, which does not stand in the backlog",
                    py_repr(&item.core.id),
                    py_repr(dep)
                )));
            };
            edges.push(dep_idx);
        }
        adj.push(edges);
    }
    if has_dependency_cycle(&adj) {
        return Err(corrupt(
            "the item dependency graph contains a cycle".to_string(),
        ));
    }
    Ok(())
}

fn validate_registry_doc(value: &Value) -> Result<RegistryDoc> {
    let Value::Object(pairs) = value else {
        return Err(corrupt(
            "the document does not carry exactly the recorded fields".to_string(),
        ));
    };
    let Some(format_value) = object_get(pairs, "format") else {
        return Err(corrupt(
            "the document does not carry exactly the recorded fields".to_string(),
        ));
    };
    if format_value == &Value::Str(FORMAT_NAME.to_string()) {
        if !keys_exact(pairs, &DOCUMENT_KEYS) {
            return Err(corrupt(
                "the document does not carry exactly the recorded fields".to_string(),
            ));
        }
        let item_values = match object_get(pairs, "items").unwrap() {
            Value::Array(items) => items,
            _ => return Err(corrupt("the items field is not a list".to_string())),
        };
        let event_values = match object_get(pairs, "history").unwrap() {
            Value::Array(events) => events,
            _ => return Err(corrupt("the history field is not a list".to_string())),
        };
        let mut core_items = Vec::with_capacity(item_values.len());
        let mut seen: HashSet<String> = HashSet::with_capacity(item_values.len());
        for item_value in item_values {
            let item = validate_item(item_value)?;
            if !seen.insert(item.id.clone()) {
                return Err(corrupt(format!(
                    "item {} stands in the backlog twice",
                    py_repr(&item.id)
                )));
            }
            core_items.push(item);
        }
        let mut history = Vec::with_capacity(event_values.len());
        for (position, event_value) in event_values.iter().enumerate() {
            history.push(validate_event(event_value, position)?);
        }
        validate_history_against_items(&core_items, &history)?;
        let mut takeovers: HashMap<&str, u64> = HashMap::new();
        for event in &history {
            if event.kind == "taken_over" {
                *takeovers.entry(event.item.as_str()).or_insert(0) += 1;
            }
        }
        let items = core_items
            .into_iter()
            .map(|core| {
                let takeover_count = if takeovers.is_empty() {
                    0
                } else {
                    takeovers.get(core.id.as_str()).copied().unwrap_or(0)
                };
                StoredItem {
                    core,
                    priority: PRIORITY_DEFAULT,
                    tags: Vec::new(),
                    depends_on: Vec::new(),
                    max_takeovers: None,
                    takeover_count,
                }
            })
            .collect();
        return Ok(RegistryDoc {
            is_v2: false,
            compaction: None,
            items,
            history,
        });
    }
    if format_value == &Value::Str(FORMAT_NAME_V2.to_string()) {
        if !keys_exact(pairs, &DOCUMENT_KEYS_V2) {
            return Err(corrupt(
                "the document does not carry exactly the recorded fields".to_string(),
            ));
        }
        let compaction = validate_compaction(object_get(pairs, "compaction").unwrap())?;
        let item_values = match object_get(pairs, "items").unwrap() {
            Value::Array(items) => items,
            _ => return Err(corrupt("the items field is not a list".to_string())),
        };
        let event_values = match object_get(pairs, "history").unwrap() {
            Value::Array(events) => events,
            _ => return Err(corrupt("the history field is not a list".to_string())),
        };
        let mut items = Vec::with_capacity(item_values.len());
        let mut seen: HashSet<String> = HashSet::with_capacity(item_values.len());
        for item_value in item_values {
            let item = validate_item_v2(item_value)?;
            if !seen.insert(item.core.id.clone()) {
                return Err(corrupt(format!(
                    "item {} stands in the backlog twice",
                    py_repr(&item.core.id)
                )));
            }
            items.push(item);
        }
        validate_stored_dag(&items)?;
        let seq_base = compaction.as_ref().map_or(0i128, |c| c.through_seq as i128);
        let mut history = Vec::with_capacity(event_values.len());
        for (position, event_value) in event_values.iter().enumerate() {
            let expected_seq = seq_base + position as i128 + 1;
            history.push(validate_event_with(event_value, expected_seq, true)?);
        }
        validate_history_against_items_v2(&items, compaction.as_ref(), &history)?;
        return Ok(RegistryDoc {
            is_v2: true,
            compaction,
            items,
            history,
        });
    }
    Err(corrupt(format!(
        "the format name is {}, not {} or {}",
        py_repr_value(format_value),
        py_repr(FORMAT_NAME),
        py_repr(FORMAT_NAME_V2)
    )))
}

/// The standing of one item history as the cross validation
/// replays it.
struct Chain {
    phase: &'static str,
    holder: Option<String>,
    epoch: i128,
    saw_offer: bool,
}

struct ChainV2 {
    phase: &'static str,
    holder: Option<String>,
    epoch: i128,
    min_takeovers: u64,
    max_takeovers: u64,
    saw_offer: bool,
}

fn phase_str(status: &str) -> &'static str {
    match status {
        "open" => "open",
        "claimed" => "claimed",
        _ => "completed",
    }
}

/// Join the two halves of a dibs-registry/1 document (DB-7).
fn validate_history_against_items(items: &[Item], history: &[Event]) -> Result<()> {
    let mut standing: HashSet<&str> = HashSet::with_capacity(items.len());
    for item in items {
        standing.insert(item.id.as_str());
    }
    let mut chains: HashMap<&str, Chain> = HashMap::with_capacity(items.len());
    for event in history {
        if !standing.contains(event.item.as_str()) {
            return Err(corrupt(format!(
                "a history event names item {}, which does not stand in the backlog",
                py_repr(&event.item)
            )));
        }
        let chain = match chains.get_mut(event.item.as_str()) {
            Some(chain) => chain,
            None => {
                if event.kind == "taken_over" && event.epoch < 2 {
                    return Err(corrupt(format!(
                        "a taken over event on item {} carries epoch {}, and no \
                         takeover stands below epoch 2",
                        py_repr(&event.item),
                        event.epoch
                    )));
                }
                let chain = match event.kind.as_str() {
                    "offered" => Chain {
                        phase: "open",
                        holder: None,
                        epoch: event.epoch,
                        saw_offer: true,
                    },
                    "claimed" | "renewed" | "taken_over" => Chain {
                        phase: "claimed",
                        holder: event.holder.clone(),
                        epoch: event.epoch,
                        saw_offer: false,
                    },
                    "released" => Chain {
                        phase: "open",
                        holder: None,
                        epoch: event.epoch,
                        saw_offer: false,
                    },
                    _ => Chain {
                        phase: "completed",
                        holder: None,
                        epoch: event.epoch,
                        saw_offer: false,
                    },
                };
                chains.insert(event.item.as_str(), chain);
                continue;
            }
        };
        if chain.phase == "completed" {
            return Err(corrupt(format!(
                "the history of item {} continues past its completion",
                py_repr(&event.item)
            )));
        }
        match event.kind.as_str() {
            "offered" => {
                if chain.saw_offer {
                    return Err(corrupt(format!(
                        "item {} is offered twice in its history",
                        py_repr(&event.item)
                    )));
                }
                return Err(corrupt(format!(
                    "an offered event on item {} does not stand first in its history",
                    py_repr(&event.item)
                )));
            }
            "claimed" | "taken_over" => {
                if event.kind == "claimed" && chain.phase != "open" {
                    return Err(corrupt(format!(
                        "the history of item {} claims a lease that already stands",
                        py_repr(&event.item)
                    )));
                }
                if event.kind == "taken_over" {
                    if chain.phase != "claimed" {
                        return Err(corrupt(format!(
                            "the history of item {} takes over a lease that does not stand",
                            py_repr(&event.item)
                        )));
                    }
                    if event.previous_holder.as_deref() != chain.holder.as_deref() {
                        return Err(corrupt(format!(
                            "a taken over event on item {} names {} as the previous \
                             holder, not the standing holder {}",
                            py_repr(&event.item),
                            py_repr(event.previous_holder.as_deref().unwrap_or_default()),
                            py_repr(chain.holder.as_deref().unwrap_or_default())
                        )));
                    }
                }
                if chain.epoch.checked_add(1) != Some(event.epoch) {
                    return Err(corrupt(format!(
                        "a claim event on item {} carries epoch {}, not one past \
                         the standing epoch {}",
                        py_repr(&event.item),
                        event.epoch,
                        chain.epoch
                    )));
                }
                chain.phase = "claimed";
                chain.holder = event.holder.clone();
                chain.epoch = event.epoch;
            }
            kind => {
                if chain.phase != "claimed" {
                    let verb = match kind {
                        "renewed" => "renews",
                        "released" => "releases",
                        _ => "completes",
                    };
                    return Err(corrupt(format!(
                        "the history of item {} {verb} a lease that does not stand",
                        py_repr(&event.item)
                    )));
                }
                if event.holder.as_deref() != chain.holder.as_deref() {
                    return Err(corrupt(format!(
                        "a {kind} event on item {} names holder {}, not the \
                         standing holder {}",
                        py_repr(&event.item),
                        py_repr(event.holder.as_deref().unwrap_or_default()),
                        py_repr(chain.holder.as_deref().unwrap_or_default())
                    )));
                }
                if event.epoch != chain.epoch {
                    return Err(corrupt(format!(
                        "a {kind} event on item {} carries epoch {}, not the \
                         standing epoch {}",
                        py_repr(&event.item),
                        event.epoch,
                        chain.epoch
                    )));
                }
                if kind == "released" {
                    chain.phase = "open";
                    chain.holder = None;
                } else if kind == "completed" {
                    chain.phase = "completed";
                    chain.holder = None;
                }
            }
        }
    }
    for item in items {
        let chain = match chains.get(item.id.as_str()) {
            Some(chain) => chain,
            None => continue,
        };
        if item.status != chain.phase {
            return Err(corrupt(format!(
                "item {} stands {} where its history ends {}",
                py_repr(&item.id),
                item.status,
                chain.phase
            )));
        }
        if item.epoch != chain.epoch {
            return Err(corrupt(format!(
                "item {} stands at epoch {} where its history ends at epoch {}",
                py_repr(&item.id),
                item.epoch,
                chain.epoch
            )));
        }
        if chain.phase == "claimed" && item.holder != chain.holder {
            return Err(corrupt(format!(
                "item {} stands held by {}, not by {} as its history ends",
                py_repr(&item.id),
                py_repr(item.holder.as_deref().unwrap_or_default()),
                py_repr(chain.holder.as_deref().unwrap_or_default())
            )));
        }
    }
    Ok(())
}

/// Join items, optional compaction baseline, and post-compaction
/// history of a dibs-registry/2 document (DB-7, DB-23, DB-24).
fn validate_history_against_items_v2(
    items: &[StoredItem],
    compaction: Option<&Compaction>,
    history: &[Event],
) -> Result<()> {
    let mut item_by_id: HashMap<&str, &StoredItem> = HashMap::with_capacity(items.len());
    for item in items {
        item_by_id.insert(item.core.id.as_str(), item);
    }
    let mut chains: HashMap<&str, ChainV2> = HashMap::with_capacity(items.len());
    if let Some(comp) = compaction {
        for base in &comp.baseline {
            let Some(&standing_item) = item_by_id.get(base.id.as_str()) else {
                return Err(corrupt(format!(
                    "the compaction baseline names item {}, which does not stand in the backlog",
                    py_repr(&base.id)
                )));
            };
            if let Some(cap) = standing_item.max_takeovers {
                if base.takeover_count > cap {
                    return Err(corrupt(format!(
                        "compaction baseline entry {} holds takeover_count {} past max_takeovers {cap}",
                        py_repr(&base.id),
                        base.takeover_count
                    )));
                }
            }
            chains.insert(
                base.id.as_str(),
                ChainV2 {
                    phase: phase_str(&base.status),
                    holder: base.holder.clone(),
                    epoch: base.epoch,
                    min_takeovers: base.takeover_count,
                    max_takeovers: base.takeover_count,
                    saw_offer: true,
                },
            );
        }
    } else {
        let mut items_with_events: HashSet<&str> = HashSet::with_capacity(items.len());
        for ev in history {
            items_with_events.insert(ev.item.as_str());
        }
        for item in items {
            if !items_with_events.contains(item.core.id.as_str()) {
                chains.insert(
                    item.core.id.as_str(),
                    ChainV2 {
                        phase: phase_str(&item.core.status),
                        holder: item.core.holder.clone(),
                        epoch: item.core.epoch,
                        min_takeovers: item.takeover_count,
                        max_takeovers: item.takeover_count,
                        saw_offer: false,
                    },
                );
            }
        }
    }

    for event in history {
        let Some(&standing_item) = item_by_id.get(event.item.as_str()) else {
            return Err(corrupt(format!(
                "a history event names item {}, which does not stand in the backlog",
                py_repr(&event.item)
            )));
        };
        if matches!(event.kind.as_str(), "claimed" | "taken_over") {
            for dep in &standing_item.depends_on {
                if !matches!(chains.get(dep.as_str()), Some(c) if c.phase == "completed") {
                    return Err(corrupt(format!(
                        "the history of item {} claims while dependency {} is not completed",
                        py_repr(&event.item),
                        py_repr(dep)
                    )));
                }
            }
        }
        let chain = match chains.get_mut(event.item.as_str()) {
            Some(chain) => chain,
            None => {
                if compaction.is_some() && event.kind != "offered" {
                    return Err(corrupt(format!(
                        "item {} is absent from the compaction baseline and does not open its post-compaction history with offered",
                        py_repr(&event.item)
                    )));
                }
                if event.kind == "taken_over" && event.epoch < 2 {
                    return Err(corrupt(format!(
                        "a taken over event on item {} carries epoch {}, and no \
                         takeover stands below epoch 2",
                        py_repr(&event.item),
                        event.epoch
                    )));
                }
                if event.kind == "reset" {
                    let Some(cap) = standing_item.max_takeovers else {
                        return Err(corrupt(format!(
                            "a reset event stands on item {}, which carries no max_takeovers",
                            py_repr(&event.item)
                        )));
                    };
                    if event.epoch < (cap as i128) + 1 {
                        return Err(corrupt(format!(
                            "a reset event on item {} carries epoch {}, below the minimum epoch {} for max_takeovers {cap}",
                            py_repr(&event.item),
                            event.epoch,
                            (cap as i128) + 1
                        )));
                    }
                }
                let chain = match event.kind.as_str() {
                    "offered" => ChainV2 {
                        phase: "open",
                        holder: None,
                        epoch: event.epoch,
                        min_takeovers: 0,
                        max_takeovers: 0,
                        saw_offer: true,
                    },
                    "claimed" => {
                        let epoch_before = event.epoch - 1;
                        let mut prev_max = if epoch_before <= 1 {
                            0
                        } else {
                            u64::try_from(epoch_before - 1).unwrap_or(u64::MAX)
                        };
                        if let Some(cap) = standing_item.max_takeovers {
                            prev_max = prev_max.min(cap - 1);
                        }
                        ChainV2 {
                            phase: "claimed",
                            holder: event.holder.clone(),
                            epoch: event.epoch,
                            min_takeovers: 0,
                            max_takeovers: prev_max,
                            saw_offer: false,
                        }
                    }
                    "taken_over" => {
                        let epoch_before = event.epoch - 1;
                        let mut prev_max = u64::try_from(epoch_before - 1).unwrap_or(u64::MAX);
                        if let Some(cap) = standing_item.max_takeovers {
                            prev_max = prev_max.min(cap - 1);
                        }
                        ChainV2 {
                            phase: "claimed",
                            holder: event.holder.clone(),
                            epoch: event.epoch,
                            min_takeovers: 1,
                            max_takeovers: prev_max.saturating_add(1),
                            saw_offer: false,
                        }
                    }
                    "renewed" => {
                        let mut prev_max = u64::try_from(event.epoch - 1).unwrap_or(u64::MAX);
                        if let Some(cap) = standing_item.max_takeovers {
                            prev_max = prev_max.min(cap);
                        }
                        ChainV2 {
                            phase: "claimed",
                            holder: event.holder.clone(),
                            epoch: event.epoch,
                            min_takeovers: 0,
                            max_takeovers: prev_max,
                            saw_offer: false,
                        }
                    }
                    "released" => {
                        let mut prev_max = u64::try_from(event.epoch - 1).unwrap_or(u64::MAX);
                        if let Some(cap) = standing_item.max_takeovers {
                            prev_max = prev_max.min(cap);
                        }
                        ChainV2 {
                            phase: "open",
                            holder: None,
                            epoch: event.epoch,
                            min_takeovers: 0,
                            max_takeovers: prev_max,
                            saw_offer: false,
                        }
                    }
                    "reset" => ChainV2 {
                        phase: "open",
                        holder: None,
                        epoch: event.epoch,
                        min_takeovers: 0,
                        max_takeovers: 0,
                        saw_offer: false,
                    },
                    _ => {
                        let mut prev_max = u64::try_from(event.epoch - 1).unwrap_or(u64::MAX);
                        if let Some(cap) = standing_item.max_takeovers {
                            prev_max = prev_max.min(cap);
                        }
                        ChainV2 {
                            phase: "completed",
                            holder: None,
                            epoch: event.epoch,
                            min_takeovers: 0,
                            max_takeovers: prev_max,
                            saw_offer: false,
                        }
                    }
                };
                chains.insert(event.item.as_str(), chain);
                continue;
            }
        };
        if chain.phase == "completed" {
            return Err(corrupt(format!(
                "the history of item {} continues past its completion",
                py_repr(&event.item)
            )));
        }
        match event.kind.as_str() {
            "offered" => {
                if chain.saw_offer {
                    return Err(corrupt(format!(
                        "item {} is offered twice in its history",
                        py_repr(&event.item)
                    )));
                }
                return Err(corrupt(format!(
                    "an offered event on item {} does not stand first in its history",
                    py_repr(&event.item)
                )));
            }
            "claimed" | "taken_over" => {
                if event.kind == "claimed" && chain.phase != "open" {
                    return Err(corrupt(format!(
                        "the history of item {} claims a lease that already stands",
                        py_repr(&event.item)
                    )));
                }
                if event.kind == "taken_over" {
                    if chain.phase != "claimed" {
                        return Err(corrupt(format!(
                            "the history of item {} takes over a lease that does not stand",
                            py_repr(&event.item)
                        )));
                    }
                    if event.previous_holder.as_deref() != chain.holder.as_deref() {
                        return Err(corrupt(format!(
                            "a taken over event on item {} names {} as the previous \
                             holder, not the standing holder {}",
                            py_repr(&event.item),
                            py_repr(event.previous_holder.as_deref().unwrap_or_default()),
                            py_repr(chain.holder.as_deref().unwrap_or_default())
                        )));
                    }
                }
                if let Some(cap) = standing_item.max_takeovers {
                    if chain.min_takeovers >= cap {
                        let label = if event.kind == "taken_over" {
                            "taken over"
                        } else {
                            "claimed"
                        };
                        return Err(corrupt(format!(
                            "a {label} event on item {} exceeds max_takeovers {cap}",
                            py_repr(&event.item)
                        )));
                    }
                    chain.max_takeovers = chain.max_takeovers.min(cap - 1);
                }
                if chain.epoch.checked_add(1) != Some(event.epoch) {
                    return Err(corrupt(format!(
                        "a claim event on item {} carries epoch {}, not one past \
                         the standing epoch {}",
                        py_repr(&event.item),
                        event.epoch,
                        chain.epoch
                    )));
                }
                chain.phase = "claimed";
                chain.holder = event.holder.clone();
                chain.epoch = event.epoch;
                if event.kind == "taken_over" {
                    chain.min_takeovers = chain.min_takeovers.checked_add(1).ok_or_else(|| {
                        corrupt(format!(
                            "item {} overflows takeover_count",
                            py_repr(&event.item)
                        ))
                    })?;
                    chain.max_takeovers = chain.max_takeovers.checked_add(1).ok_or_else(|| {
                        corrupt(format!(
                            "item {} overflows takeover_count",
                            py_repr(&event.item)
                        ))
                    })?;
                }
            }
            "reset" => {
                let Some(cap) = standing_item.max_takeovers else {
                    return Err(corrupt(format!(
                        "a reset event stands on item {}, which carries no max_takeovers",
                        py_repr(&event.item)
                    )));
                };
                if event.epoch < (cap as i128) + 1 {
                    return Err(corrupt(format!(
                        "a reset event on item {} carries epoch {}, below the minimum epoch {} for max_takeovers {cap}",
                        py_repr(&event.item),
                        event.epoch,
                        (cap as i128) + 1
                    )));
                }
                if chain.max_takeovers < cap {
                    return Err(corrupt(format!(
                        "a reset event on item {} fires at takeover_count {}, below max_takeovers {cap}",
                        py_repr(&event.item),
                        chain.max_takeovers
                    )));
                }
                if event.previous_holder.as_deref() != chain.holder.as_deref() {
                    return Err(corrupt(format!(
                        "a reset event on item {} names {} as the previous holder, not {}",
                        py_repr(&event.item),
                        py_repr_value(
                            &event
                                .previous_holder
                                .as_ref()
                                .map(|s| Value::Str(s.clone()))
                                .unwrap_or(Value::Null)
                        ),
                        py_repr_value(
                            &chain
                                .holder
                                .as_ref()
                                .map(|s| Value::Str(s.clone()))
                                .unwrap_or(Value::Null)
                        )
                    )));
                }
                if event.epoch != chain.epoch {
                    return Err(corrupt(format!(
                        "a reset event on item {} carries epoch {}, not the standing epoch {}",
                        py_repr(&event.item),
                        event.epoch,
                        chain.epoch
                    )));
                }
                chain.phase = "open";
                chain.holder = None;
                chain.min_takeovers = 0;
                chain.max_takeovers = 0;
            }
            kind => {
                if chain.phase != "claimed" {
                    let verb = match kind {
                        "renewed" => "renews",
                        "released" => "releases",
                        _ => "completes",
                    };
                    return Err(corrupt(format!(
                        "the history of item {} {verb} a lease that does not stand",
                        py_repr(&event.item)
                    )));
                }
                if event.holder.as_deref() != chain.holder.as_deref() {
                    return Err(corrupt(format!(
                        "a {kind} event on item {} names holder {}, not the \
                         standing holder {}",
                        py_repr(&event.item),
                        py_repr(event.holder.as_deref().unwrap_or_default()),
                        py_repr(chain.holder.as_deref().unwrap_or_default())
                    )));
                }
                if event.epoch != chain.epoch {
                    return Err(corrupt(format!(
                        "a {kind} event on item {} carries epoch {}, not the \
                         standing epoch {}",
                        py_repr(&event.item),
                        event.epoch,
                        chain.epoch
                    )));
                }
                if kind == "released" {
                    chain.phase = "open";
                    chain.holder = None;
                } else if kind == "completed" {
                    chain.phase = "completed";
                    chain.holder = None;
                }
            }
        }
    }

    for item in items {
        let Some(chain) = chains.get(item.core.id.as_str()) else {
            if compaction.is_some() {
                return Err(corrupt(format!(
                    "item {} is absent from the compaction baseline and has no post-compaction history",
                    py_repr(&item.core.id)
                )));
            }
            continue;
        };
        if item.core.status != chain.phase {
            return Err(corrupt(format!(
                "item {} stands {} where its history ends {}",
                py_repr(&item.core.id),
                item.core.status,
                chain.phase
            )));
        }
        if item.core.epoch != chain.epoch {
            return Err(corrupt(format!(
                "item {} stands at epoch {} where its history ends at epoch {}",
                py_repr(&item.core.id),
                item.core.epoch,
                chain.epoch
            )));
        }
        if chain.phase == "claimed" && item.core.holder != chain.holder {
            return Err(corrupt(format!(
                "item {} stands held by {}, not by {} as its history ends",
                py_repr(&item.core.id),
                py_repr(item.core.holder.as_deref().unwrap_or_default()),
                py_repr(chain.holder.as_deref().unwrap_or_default())
            )));
        }
        if item.takeover_count < chain.min_takeovers || item.takeover_count > chain.max_takeovers {
            let expected = if item.takeover_count < chain.min_takeovers {
                chain.min_takeovers
            } else {
                chain.max_takeovers
            };
            return Err(corrupt(format!(
                "item {} stands at takeover_count {} where its history ends at takeover_count {expected}",
                py_repr(&item.core.id),
                item.takeover_count
            )));
        }
    }
    Ok(())
}

/// Escape a string the way Python's json module escapes it with
/// its default ensure_ascii, so a registry the port writes is
/// byte identical to the registry the Python implementation wrote
/// for the same content: the quote, the backslash, and every
/// character outside the printable ASCII range is escaped, with a
/// surrogate pair for a character past the basic plane.
pub fn escape_json_string(text: &str, out: &mut String) {
    out.push('"');
    for ch in text.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 || (c as u32) > 0x7e => {
                let n = c as u32;
                if n <= 0xffff {
                    out.push_str(&format!("\\u{n:04x}"));
                } else {
                    let shifted = n - 0x10000;
                    let high = 0xd800 + (shifted >> 10);
                    let low = 0xdc00 + (shifted & 0x3ff);
                    out.push_str(&format!("\\u{high:04x}\\u{low:04x}"));
                }
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

fn write_indent(out: &mut String, level: usize) {
    for _ in 0..level {
        out.push_str("  ");
    }
}

fn render_opt_string(value: &Option<String>, out: &mut String) {
    match value {
        Some(text) => escape_json_string(text, out),
        None => out.push_str("null"),
    }
}

fn render_string_array(values: &[String], level: usize, out: &mut String) {
    if values.is_empty() {
        out.push_str("[]");
        return;
    }
    out.push_str("[\n");
    let count = values.len();
    for (index, val) in values.iter().enumerate() {
        write_indent(out, level + 1);
        escape_json_string(val, out);
        if index + 1 < count {
            out.push(',');
        }
        out.push('\n');
    }
    write_indent(out, level);
    out.push(']');
}

/// One field of a rendered object, its key and the closure
/// that writes its value.
type FieldRenderer<'a> = (&'a str, &'a dyn Fn(&mut String));

/// Render one object from its field table.
fn render_fields(fields: &[FieldRenderer], level: usize, out: &mut String) {
    out.push_str("{\n");
    let count = fields.len();
    for (index, (key, render)) in fields.iter().enumerate() {
        write_indent(out, level + 1);
        escape_json_string(key, out);
        out.push_str(": ");
        render(out);
        if index + 1 < count {
            out.push(',');
        }
        out.push('\n');
    }
    write_indent(out, level);
    out.push('}');
}

fn render_item(item: &Item, level: usize, out: &mut String) {
    let fields: [FieldRenderer; 7] = [
        ("id", &|out| escape_json_string(&item.id, out)),
        ("note", &|out| escape_json_string(&item.note, out)),
        ("status", &|out| escape_json_string(&item.status, out)),
        ("holder", &|out| render_opt_string(&item.holder, out)),
        ("token_hash", &|out| {
            render_opt_string(&item.token_hash, out)
        }),
        ("epoch", &|out| out.push_str(&item.epoch.to_string())),
        ("expires_at", &|out| {
            render_opt_string(&item.expires_at, out)
        }),
    ];
    render_fields(&fields, level, out);
}

fn render_item_v2(item: &StoredItem, level: usize, out: &mut String) {
    let fields: [FieldRenderer; 12] = [
        ("id", &|out| escape_json_string(&item.core.id, out)),
        ("note", &|out| escape_json_string(&item.core.note, out)),
        ("status", &|out| escape_json_string(&item.core.status, out)),
        ("priority", &|out| out.push_str(&item.priority.to_string())),
        ("tags", &|out| {
            render_string_array(&item.tags, level + 1, out)
        }),
        ("depends_on", &|out| {
            render_string_array(&item.depends_on, level + 1, out)
        }),
        ("max_takeovers", &|out| match item.max_takeovers {
            Some(cap) => out.push_str(&cap.to_string()),
            None => out.push_str("null"),
        }),
        ("takeover_count", &|out| {
            out.push_str(&item.takeover_count.to_string())
        }),
        ("holder", &|out| render_opt_string(&item.core.holder, out)),
        ("token_hash", &|out| {
            render_opt_string(&item.core.token_hash, out)
        }),
        ("epoch", &|out| out.push_str(&item.core.epoch.to_string())),
        ("expires_at", &|out| {
            render_opt_string(&item.core.expires_at, out)
        }),
    ];
    render_fields(&fields, level, out);
}

fn render_baseline_entry(entry: &BaselineEntry, level: usize, out: &mut String) {
    let fields: [FieldRenderer; 5] = [
        ("id", &|out| escape_json_string(&entry.id, out)),
        ("status", &|out| escape_json_string(&entry.status, out)),
        ("holder", &|out| render_opt_string(&entry.holder, out)),
        ("epoch", &|out| out.push_str(&entry.epoch.to_string())),
        ("takeover_count", &|out| {
            out.push_str(&entry.takeover_count.to_string())
        }),
    ];
    render_fields(&fields, level, out);
}

fn render_compaction(compaction: &Compaction, level: usize, out: &mut String) {
    let fields: [FieldRenderer; 4] = [
        ("through_seq", &|out| {
            out.push_str(&compaction.through_seq.to_string())
        }),
        ("archived_events_sha256", &|out| {
            escape_json_string(&compaction.archived_events_sha256, out)
        }),
        ("compacted_at", &|out| {
            escape_json_string(&compaction.compacted_at, out)
        }),
        ("baseline", &|out| {
            if compaction.baseline.is_empty() {
                out.push_str("[]");
            } else {
                out.push_str("[\n");
                let count = compaction.baseline.len();
                for (index, entry) in compaction.baseline.iter().enumerate() {
                    write_indent(out, level + 2);
                    render_baseline_entry(entry, level + 2, out);
                    if index + 1 < count {
                        out.push(',');
                    }
                    out.push('\n');
                }
                write_indent(out, level + 1);
                out.push(']');
            }
        }),
    ];
    render_fields(&fields, level, out);
}

fn render_event(event: &Event, level: usize, out: &mut String) {
    let fields: [FieldRenderer; 7] = [
        ("seq", &|out| out.push_str(&event.seq.to_string())),
        ("kind", &|out| escape_json_string(&event.kind, out)),
        ("item", &|out| escape_json_string(&event.item, out)),
        ("holder", &|out| render_opt_string(&event.holder, out)),
        ("at", &|out| escape_json_string(&event.at, out)),
        ("epoch", &|out| out.push_str(&event.epoch.to_string())),
        ("previous_holder", &|out| {
            render_opt_string(&event.previous_holder, out)
        }),
    ];
    render_fields(&fields, level, out);
}

fn render_event_jsonl_line(event: &Event) -> String {
    let mut out = format!("{{\"seq\": {}, \"kind\": ", event.seq);
    escape_json_string(&event.kind, &mut out);
    out.push_str(", \"item\": ");
    escape_json_string(&event.item, &mut out);
    out.push_str(", \"holder\": ");
    render_opt_string(&event.holder, &mut out);
    out.push_str(", \"at\": ");
    escape_json_string(&event.at, &mut out);
    out.push_str(&format!(
        ", \"epoch\": {}, \"previous_holder\": ",
        event.epoch
    ));
    render_opt_string(&event.previous_holder, &mut out);
    out.push('}');
    out
}

fn should_render_v2(document: &RegistryDoc) -> bool {
    if document.is_v2 || document.compaction.is_some() {
        return true;
    }
    if document.items.iter().any(|item| {
        !item.depends_on.is_empty()
            || item.priority != PRIORITY_DEFAULT
            || !item.tags.is_empty()
            || item.max_takeovers.is_some()
    }) {
        return true;
    }
    document.history.iter().any(|event| event.kind == "reset")
}

/// Render a document in dibs-registry/1 when only /1 features
/// stand, or dibs-registry/2 once promoted (DB-14, DB-21).
fn render_registry_doc(document: &RegistryDoc) -> String {
    let v2 = should_render_v2(document);
    let mut out =
        String::with_capacity(64 + document.items.len() * 480 + document.history.len() * 202);
    out.push_str("{\n");
    write_indent(&mut out, 1);
    escape_json_string("format", &mut out);
    out.push_str(": ");
    escape_json_string(if v2 { FORMAT_NAME_V2 } else { FORMAT_NAME }, &mut out);
    out.push_str(",\n");
    if v2 {
        write_indent(&mut out, 1);
        escape_json_string("compaction", &mut out);
        out.push_str(": ");
        match &document.compaction {
            Some(compaction) => render_compaction(compaction, 1, &mut out),
            None => out.push_str("null"),
        }
        out.push_str(",\n");
    }
    write_indent(&mut out, 1);
    escape_json_string("items", &mut out);
    out.push_str(": ");
    if document.items.is_empty() {
        out.push_str("[]");
    } else {
        out.push_str("[\n");
        let count = document.items.len();
        for (index, item) in document.items.iter().enumerate() {
            write_indent(&mut out, 2);
            if v2 {
                render_item_v2(item, 2, &mut out);
            } else {
                render_item(&item.core, 2, &mut out);
            }
            if index + 1 < count {
                out.push(',');
            }
            out.push('\n');
        }
        write_indent(&mut out, 1);
        out.push(']');
    }
    out.push_str(",\n");
    write_indent(&mut out, 1);
    escape_json_string("history", &mut out);
    out.push_str(": ");
    if document.history.is_empty() {
        out.push_str("[]");
    } else {
        out.push_str("[\n");
        let count = document.history.len();
        for (index, event) in document.history.iter().enumerate() {
            write_indent(&mut out, 2);
            render_event(event, 2, &mut out);
            if index + 1 < count {
                out.push(',');
            }
            out.push('\n');
        }
        write_indent(&mut out, 1);
        out.push(']');
    }
    out.push_str("\n}");
    out
}

/// Draw a fresh claim token, 32 bytes from the system random
/// source rendered in the URL safe base64 alphabet without
/// padding, the shape the Python secrets module produced. A
/// failure of the random source is a typed error raised before
/// any state changes.
fn draw_token() -> Result<String> {
    let mut bytes = [0u8; TOKEN_BYTES];
    getrandom::fill(&mut bytes)
        .map_err(|err| Error::Failure(format!("the claim token could not be drawn, {err}")))?;
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::with_capacity(43);
    for group in bytes.chunks(3) {
        let first = group[0] as u32;
        let second = *group.get(1).unwrap_or(&0) as u32;
        let third = *group.get(2).unwrap_or(&0) as u32;
        let joined = (first << 16) | (second << 8) | third;
        out.push(ALPHABET[(joined >> 18) as usize & 63] as char);
        out.push(ALPHABET[(joined >> 12) as usize & 63] as char);
        if group.len() > 1 {
            out.push(ALPHABET[(joined >> 6) as usize & 63] as char);
        }
        if group.len() > 2 {
            out.push(ALPHABET[joined as usize & 63] as char);
        }
    }
    Ok(out)
}

fn token_matches(presented: &str, stored_hash: &str) -> bool {
    let presented_hash = token_hash(presented);
    let left = presented_hash.as_bytes();
    let right = stored_hash.as_bytes();
    if left.len() != right.len() {
        return false;
    }
    let mut difference = 0u8;
    for index in 0..left.len() {
        difference |= left[index] ^ right[index];
    }
    difference == 0
}

/// Decode registry bytes to text the way the Python json module
/// decoded them, detecting a byte order mark and transcoding
/// UTF-16 and UTF-32, detecting the same encodings by the NUL
/// pattern of the opening bytes where no mark stands, and
/// refusing what does not decode.
fn decode_document_bytes(bytes: Vec<u8>) -> std::result::Result<String, String> {
    if bytes.starts_with(&[0xff, 0xfe, 0x00, 0x00]) {
        decode_utf32(&bytes[4..], true)
    } else if bytes.starts_with(&[0x00, 0x00, 0xfe, 0xff]) {
        decode_utf32(&bytes[4..], false)
    } else if bytes.starts_with(&[0xff, 0xfe]) {
        decode_utf16(&bytes[2..], true)
    } else if bytes.starts_with(&[0xfe, 0xff]) {
        decode_utf16(&bytes[2..], false)
    } else if bytes.starts_with(&[0xef, 0xbb, 0xbf]) {
        // The buffer is owned, so the mark is drained off the
        // front instead of copying the rest of the document.
        let mut bytes = bytes;
        bytes.drain(..3);
        String::from_utf8(bytes)
            .map_err(|err| format!("the document does not decode as UTF-8, {err}"))
    } else if let Some(decoded) = decode_detected_encoding(&bytes) {
        decoded
    } else {
        String::from_utf8(bytes)
            .map_err(|err| format!("the document does not decode as UTF-8, {err}"))
    }
}

/// Decode registry bytes that carry no byte order mark under
/// the encoding the NUL pattern of their first four bytes
/// names, the rest of the detection the Python json module
/// performs. A document in UTF-16 or UTF-32 without a mark
/// still decodes there, because the zero bytes of its opening
/// units name the encoding and the endianness: 00 00 names
/// UTF-32 big endian, 00 XX names UTF-16 big endian, XX 00 00
/// 00 names UTF-32 little endian, and XX 00 with a nonzero
/// third or fourth byte names UTF-16 little endian. The
/// decoding is the strict decoding the mark arms call, so a
/// document the pattern names but the encoding cannot carry
/// refuses with the same sentences. Bytes whose opening names
/// no encoding return None and fall through to the UTF-8
/// reading, and an input shorter than four bytes names
/// nothing here, as it names nothing under the Python
/// detection.
fn decode_detected_encoding(bytes: &[u8]) -> Option<std::result::Result<String, String>> {
    if bytes.len() < 4 {
        return None;
    }
    if bytes[0] == 0 {
        if bytes[1] == 0 {
            Some(decode_utf32(bytes, false))
        } else {
            Some(decode_utf16(bytes, false))
        }
    } else if bytes[1] == 0 {
        if bytes[2] == 0 && bytes[3] == 0 {
            Some(decode_utf32(bytes, true))
        } else {
            Some(decode_utf16(bytes, true))
        }
    } else {
        None
    }
}

fn decode_utf16(bytes: &[u8], little_endian: bool) -> std::result::Result<String, String> {
    if !bytes.len().is_multiple_of(2) {
        return Err("the document does not decode as UTF-16".to_string());
    }
    let units: Vec<u16> = bytes
        .chunks(2)
        .map(|pair| {
            if little_endian {
                (pair[0] as u16) | ((pair[1] as u16) << 8)
            } else {
                ((pair[0] as u16) << 8) | pair[1] as u16
            }
        })
        .collect();
    char::decode_utf16(units)
        .collect::<std::result::Result<String, _>>()
        .map_err(|_| "the document does not decode as UTF-16".to_string())
}

fn decode_utf32(bytes: &[u8], little_endian: bool) -> std::result::Result<String, String> {
    if !bytes.len().is_multiple_of(4) {
        return Err("the document does not decode as UTF-32".to_string());
    }
    let mut out = String::new();
    for group in bytes.chunks(4) {
        let value = if little_endian {
            (group[0] as u32)
                | ((group[1] as u32) << 8)
                | ((group[2] as u32) << 16)
                | ((group[3] as u32) << 24)
        } else {
            ((group[0] as u32) << 24)
                | ((group[1] as u32) << 16)
                | ((group[2] as u32) << 8)
                | group[3] as u32
        };
        match char::from_u32(value) {
            Some(ch) => out.push(ch),
            None => return Err("the document does not decode as UTF-32".to_string()),
        }
    }
    Ok(out)
}

/// Open the temporary file a publish stages its document in.
/// The open creates the file exclusively, so a plant standing
/// at the temporary name, a symlink to a victim file among
/// them, fails the publish instead of being followed and
/// written through.
fn open_publish_temp(path: &Path) -> std::io::Result<File> {
    OpenOptions::new().write(true).create_new(true).open(path)
}

/// Draw the four random bytes a publish temporary name carries.
fn draw_publish_suffix() -> Result<[u8; 4]> {
    let mut random = [0u8; 4];
    getrandom::fill(&mut random)
        .map_err(|err| Error::Failure(format!("the registry could not be published, {err}")))?;
    Ok(random)
}

/// Create the temporary file a publish stages its document in,
/// under a name drawn fresh beside the registry file. A name
/// that already stands is a collision, and the draw repeats a
/// bounded number of times before the publish fails: the name
/// carries four random bytes, so one plant or one racing
/// publish cannot hold the whole space, and a redraw lands
/// clear of it. The open itself stays the exclusive create the
/// planted symlink pin holds, so a plant is never followed or
/// written through, only stepped around.
fn create_publish_temp(
    path: &Path,
    name: &str,
    draw: &mut impl FnMut() -> Result<[u8; 4]>,
) -> Result<(PathBuf, File)> {
    for _ in 0..8 {
        let random = draw()?;
        let hex: String = random.iter().map(|byte| format!("{byte:02x}")).collect();
        let temp = path.with_file_name(format!(".{name}.tmp-{}-{hex}", std::process::id()));
        match open_publish_temp(&temp) {
            Ok(handle) => return Ok((temp, handle)),
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(err) => {
                return Err(Error::Failure(format!(
                    "the registry could not be published, {err}"
                )));
            }
        }
    }
    Err(Error::Failure(
        "the registry could not be published, the temporary file name collided on every draw"
            .to_string(),
    ))
}

/// One backlog in one registry file, opened fresh on every call.
pub struct Registry {
    path: PathBuf,
    lock_path: PathBuf,
}

/// The lock a command holds, with the path it locked. Holding
/// the descriptor is not enough on its own on Unix, because the
/// file at the lock path can be deleted and replaced while a
/// command works, and a later command then locks the replacement
/// and works concurrently against the same registry. The guard
/// therefore checks on Unix that the file it holds is still the
/// file at the path, by device and inode, while on Windows the
/// lock file is opened without delete sharing so the operating
/// system refuses deletion while a handle stands open.
struct LockGuard {
    file: File,
    #[cfg(unix)]
    lock_path: PathBuf,
}

impl Drop for LockGuard {
    fn drop(&mut self) {
        // Explicitly release the kernel lock before closing the file
        // descriptor so a concurrent fork in another thread cannot hold
        // an inherited open file description locked until execve.
        let _ = self.file.unlock();
    }
}

impl LockGuard {
    /// Whether the file this guard holds is the file standing
    /// at the lock path now.
    #[cfg(unix)]
    fn identity_holds(&self) -> bool {
        use std::os::unix::fs::MetadataExt;
        let held = match self.file.metadata() {
            Ok(metadata) => metadata,
            Err(_) => return false,
        };
        let standing = match std::fs::metadata(&self.lock_path) {
            Ok(metadata) => metadata,
            Err(_) => return false,
        };
        held.dev() == standing.dev() && held.ino() == standing.ino()
    }

    #[cfg(not(unix))]
    fn identity_holds(&self) -> bool {
        true
    }

    /// Refuse to go on when the held lock is not the file at
    /// the lock path, because anything this command published
    /// could then be overwritten by the command that locked the
    /// standing file, and a success of this command could be
    /// erased without a trace.
    #[cfg(unix)]
    fn verify(&self) -> Result<()> {
        if self.identity_holds() {
            Ok(())
        } else {
            Err(Error::Failure(format!(
                "the lock file at {} is not the file this command locked, \
                 so the registry was left unchanged",
                self.lock_path.display()
            )))
        }
    }

    #[cfg(not(unix))]
    fn verify(&self) -> Result<()> {
        Ok(())
    }
}

#[cfg(unix)]
fn has_trailing_dir_syntax(raw: &std::ffi::OsStr) -> bool {
    use std::os::unix::ffi::OsStrExt;
    let bytes = raw.as_bytes();
    bytes.ends_with(b"/") || bytes.ends_with(b"/.") || bytes.ends_with(b"/..")
}

#[cfg(windows)]
fn has_trailing_dir_syntax(raw: &std::ffi::OsStr) -> bool {
    use std::os::windows::ffi::OsStrExt;
    let units: Vec<u16> = raw.encode_wide().collect();
    let is_sep = |u: u16| u == b'/' as u16 || u == b'\\' as u16;
    matches!(
        units.as_slice(),
        [.., last] if is_sep(*last) || *last == b'.' as u16 || *last == b' ' as u16
    )
}

#[cfg(not(any(unix, windows)))]
fn has_trailing_dir_syntax(raw: &std::ffi::OsStr) -> bool {
    let lossy = raw.to_string_lossy();
    lossy.ends_with('/') || lossy.ends_with("/.") || lossy.ends_with("/..")
}

#[doc(hidden)]
pub fn retry_transient_rename<F: FnMut() -> std::io::Result<()>>(mut op: F) -> std::io::Result<()> {
    let mut attempt = 0u64;
    loop {
        match op() {
            Ok(()) => return Ok(()),
            Err(err)
                if (err.kind() == std::io::ErrorKind::PermissionDenied
                    || err.raw_os_error() == Some(32))
                    && attempt < 5 =>
            {
                attempt += 1;
                std::thread::sleep(std::time::Duration::from_millis(attempt * 5));
            }
            Err(err) => return Err(err),
        }
    }
}

#[cfg(not(windows))]
fn rename_temp_over_registry(temp: &Path, target: &Path) -> std::io::Result<()> {
    std::fs::rename(temp, target)
}

#[cfg(windows)]
fn rename_temp_over_registry(temp: &Path, target: &Path) -> std::io::Result<()> {
    retry_transient_rename(|| std::fs::rename(temp, target))
}

fn validate_file_path(path: &Path, label: &str) -> Result<()> {
    let raw = path.as_os_str();
    if raw.is_empty() || path.file_name().is_none() || has_trailing_dir_syntax(raw) {
        let rendered = py_repr(&path.to_string_lossy());
        return Err(Error::InvalidRequest(format!(
            "invalid {label} path {rendered}, the path must name a file"
        )));
    }
    Ok(())
}

fn count_missing_tail(path: &Path) -> usize {
    let mut missing = 0usize;
    let mut cur = path;
    while std::fs::symlink_metadata(cur).is_err() {
        if cur.file_name().is_none() {
            break;
        }
        missing += 1;
        match cur.parent() {
            Some(parent) if !parent.as_os_str().is_empty() => cur = parent,
            _ => break,
        }
    }
    missing
}

fn resolve_physical_path_bounded(path: &Path, depth: usize) -> PathBuf {
    if depth >= 16 {
        return path.to_path_buf();
    }
    if let Ok(canon) = std::fs::canonicalize(path) {
        return canon;
    }
    let base = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(path)
    };
    let mut acc = PathBuf::new();
    let mut unresolved_depth = 0usize;
    for comp in base.components() {
        match comp {
            Component::Prefix(prefix) => {
                acc.push(prefix.as_os_str());
            }
            Component::RootDir => {
                acc.push(comp.as_os_str());
                if let Ok(canon) = std::fs::canonicalize(&acc) {
                    acc = canon;
                }
            }
            Component::CurDir => {}
            Component::ParentDir => {
                if unresolved_depth > 0 {
                    acc.pop();
                    unresolved_depth -= 1;
                } else {
                    if let Ok(canon) = std::fs::canonicalize(&acc) {
                        acc = canon;
                    }
                    acc.pop();
                }
            }
            Component::Normal(name) => {
                acc.push(name);
                if unresolved_depth == 0 {
                    if let Ok(canon) = std::fs::canonicalize(&acc) {
                        acc = canon;
                    } else if let Ok(meta) = std::fs::symlink_metadata(&acc) {
                        if meta.file_type().is_symlink() {
                            if let Ok(target) = std::fs::read_link(&acc) {
                                let next = if target.is_absolute() {
                                    target
                                } else {
                                    let mut joined = acc.clone();
                                    joined.pop();
                                    joined.push(target);
                                    joined
                                };
                                acc = resolve_physical_path_bounded(&next, depth + 1);
                                unresolved_depth = count_missing_tail(&acc);
                            }
                        }
                    } else {
                        unresolved_depth = 1;
                    }
                } else {
                    unresolved_depth += 1;
                }
            }
        }
    }
    acc
}

fn resolve_physical_path(path: &Path) -> PathBuf {
    resolve_physical_path_bounded(path, 0)
}

#[cfg(unix)]
fn same_file_identity(left: &Path, right: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    if let (Ok(ml), Ok(mr)) = (std::fs::metadata(left), std::fs::metadata(right)) {
        ml.dev() == mr.dev() && ml.ino() == mr.ino()
    } else {
        false
    }
}

#[cfg(windows)]
fn same_file_identity(left: &Path, right: &Path) -> bool {
    fn windows_file_id(path: &Path) -> Option<(u32, u64)> {
        use std::os::windows::fs::OpenOptionsExt;
        use std::os::windows::io::AsRawHandle;

        #[repr(C)]
        struct FileTime {
            low: u32,
            high: u32,
        }
        #[repr(C)]
        struct ByHandleFileInformation {
            file_attributes: u32,
            creation_time: FileTime,
            last_access_time: FileTime,
            last_write_time: FileTime,
            volume_serial_number: u32,
            file_size_high: u32,
            file_size_low: u32,
            number_of_links: u32,
            file_index_high: u32,
            file_index_low: u32,
        }
        extern "system" {
            fn GetFileInformationByHandle(
                file: std::os::windows::io::RawHandle,
                info: *mut ByHandleFileInformation,
            ) -> i32;
        }

        let file = OpenOptions::new()
            .access_mode(0)
            .share_mode(0x0000_0001 | 0x0000_0002 | 0x0000_0004)
            .open(path)
            .ok()?;
        let mut info = std::mem::MaybeUninit::<ByHandleFileInformation>::zeroed();
        let ok = unsafe { GetFileInformationByHandle(file.as_raw_handle(), info.as_mut_ptr()) };
        if ok == 0 {
            return None;
        }
        let info = unsafe { info.assume_init() };
        let index = ((info.file_index_high as u64) << 32) | (info.file_index_low as u64);
        Some((info.volume_serial_number, index))
    }

    match (windows_file_id(left), windows_file_id(right)) {
        (Some(id_l), Some(id_r)) => id_l == id_r,
        _ => false,
    }
}

#[cfg(not(any(unix, windows)))]
fn same_file_identity(_left: &Path, _right: &Path) -> bool {
    false
}

#[cfg(unix)]
fn is_dir_case_insensitive(dir: &Path) -> bool {
    let mut existing = if dir.as_os_str().is_empty() {
        Path::new(".")
    } else {
        dir
    };
    while std::fs::metadata(existing).is_err() {
        match existing.parent() {
            Some(parent) if !parent.as_os_str().is_empty() => existing = parent,
            _ => return false,
        }
    }
    #[cfg(target_os = "macos")]
    {
        use std::ffi::{c_char, c_int, c_long, CString};
        use std::os::unix::ffi::OsStrExt;
        extern "C" {
            fn pathconf(path: *const c_char, name: c_int) -> c_long;
        }
        const PC_CASE_SENSITIVE: c_int = 11;
        if let Ok(c_path) = CString::new(existing.as_os_str().as_bytes()) {
            let res = unsafe { pathconf(c_path.as_ptr(), PC_CASE_SENSITIVE) };
            if res == 0 {
                return true;
            }
            if res > 0 {
                return false;
            }
        }
    }
    if let Some(dir_name) = existing.file_name().and_then(|n| n.to_str()) {
        if let Some(pos) = dir_name.bytes().position(|b| b.is_ascii_alphabetic()) {
            let mut bytes = dir_name.as_bytes().to_vec();
            bytes[pos] = if bytes[pos].is_ascii_lowercase() {
                bytes[pos].to_ascii_uppercase()
            } else {
                bytes[pos].to_ascii_lowercase()
            };
            if let Ok(toggled) = String::from_utf8(bytes) {
                let candidate = existing.with_file_name(toggled);
                return same_file_identity(existing, &candidate);
            }
        }
    }
    false
}

fn paths_eq_platform(left: &Path, right: &Path) -> bool {
    #[cfg(windows)]
    {
        left.to_string_lossy().to_lowercase() == right.to_string_lossy().to_lowercase()
    }
    #[cfg(not(windows))]
    {
        if left == right {
            return true;
        }
        #[cfg(unix)]
        {
            if left.parent() == right.parent() {
                if let (Some(parent), Some(nl), Some(nr)) = (
                    left.parent(),
                    left.file_name().and_then(|s| s.to_str()),
                    right.file_name().and_then(|s| s.to_str()),
                ) {
                    if nl.eq_ignore_ascii_case(nr) && is_dir_case_insensitive(parent) {
                        return true;
                    }
                }
            }
        }
        false
    }
}

fn paths_alias(left: &Path, right: &Path) -> bool {
    if left == right || same_file_identity(left, right) {
        return true;
    }
    paths_eq_platform(&resolve_physical_path(left), &resolve_physical_path(right))
}

fn sha256_hex_slices(first: &[u8], second: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(first);
    hasher.update(second);
    let digest = hasher.finalize();
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn lock_path_for(path: &Path) -> PathBuf {
    match path.file_name() {
        Some(name) => {
            let mut lock_name = name.to_os_string();
            lock_name.push(LOCK_SUFFIX);
            path.with_file_name(lock_name)
        }
        None => {
            let mut raw = path.to_path_buf().into_os_string();
            raw.push(LOCK_SUFFIX);
            PathBuf::from(raw)
        }
    }
}

impl Registry {
    pub fn new(path: PathBuf) -> Registry {
        let lock_path = lock_path_for(&path);
        Registry { path, lock_path }
    }

    /// Validate that the registry path names a file component and
    /// carries no trailing directory syntax (DB-15).
    pub fn validate_registry_path(&self) -> Result<()> {
        validate_file_path(&self.path, "registry")
    }

    /// Validate an archive path under DB-15 and verify neither it nor
    /// its sibling lock file can overwrite or alias the registry file
    /// or its lock file (DB-24).
    pub fn validate_archive_path(&self, archive_path: &Path) -> Result<()> {
        validate_file_path(archive_path, "archive")?;
        let archive_lock = lock_path_for(archive_path);
        if paths_alias(archive_path, &self.path)
            || paths_alias(archive_path, &self.lock_path)
            || paths_alias(&archive_lock, &self.path)
            || paths_alias(&archive_lock, &self.lock_path)
        {
            return Err(Error::InvalidRequest(
                "the archive path must not point to the registry file or its lock file".to_string(),
            ));
        }
        Ok(())
    }

    /// Take a shared read lock when the sibling lock file stands
    /// readable, or proceed lock free over the atomic rename
    /// registry file when no lock file stands or the directory
    /// or filesystem is read only (DB-16).
    fn lock_shared_read(&self) -> Result<Option<LockGuard>> {
        for attempt in 1..=5 {
            let mut options = OpenOptions::new();
            options.read(true);
            #[cfg(windows)]
            {
                use std::os::windows::fs::OpenOptionsExt;
                options.share_mode(0x0000_0001 | 0x0000_0002);
            }
            let file = match options.open(&self.lock_path) {
                Ok(file) => file,
                Err(err)
                    if matches!(
                        err.kind(),
                        std::io::ErrorKind::NotFound
                            | std::io::ErrorKind::PermissionDenied
                            | std::io::ErrorKind::ReadOnlyFilesystem
                    ) =>
                {
                    return Ok(None);
                }
                Err(err) => {
                    return Err(Error::Failure(format!(
                        "the lock file beside the registry cannot be opened, {err}"
                    )));
                }
            };
            file.lock_shared().map_err(|err| {
                Error::Failure(format!(
                    "the lock file beside the registry cannot be locked, {err}"
                ))
            })?;
            let guard = LockGuard {
                file,
                #[cfg(unix)]
                lock_path: self.lock_path.clone(),
            };
            if guard.identity_holds() {
                return Ok(Some(guard));
            }
            if attempt == 5 {
                return Err(Error::Failure(format!(
                    "the lock file at {} is not the file this command locked, \
                     so the registry was left unchanged",
                    self.lock_path.display()
                )));
            }
        }
        unreachable!()
    }

    fn lock_exclusive_at(lock_path: &Path, target_label: &str) -> Result<LockGuard> {
        for attempt in 1..=5 {
            let mut options = OpenOptions::new();
            options.read(true).write(true).create(true).truncate(false);
            #[cfg(windows)]
            {
                use std::os::windows::fs::OpenOptionsExt;
                options.share_mode(0x0000_0001 | 0x0000_0002);
            }
            let file = options.open(lock_path).map_err(|err| {
                Error::Failure(format!(
                    "the lock file beside the {target_label} cannot be opened, {err}"
                ))
            })?;
            file.lock().map_err(|err| {
                Error::Failure(format!(
                    "the lock file beside the {target_label} cannot be locked, {err}"
                ))
            })?;
            let guard = LockGuard {
                file,
                #[cfg(unix)]
                lock_path: lock_path.to_path_buf(),
            };
            if guard.identity_holds() {
                return Ok(guard);
            }
            if attempt == 5 {
                return Err(Error::Failure(format!(
                    "the lock file at {} is not the file this command locked, \
                     so the {target_label} was left unchanged",
                    lock_path.display()
                )));
            }
        }
        unreachable!()
    }

    /// Take an exclusive lock on the sibling lock file, creating
    /// it under the process umask with no hardcoded mode override
    /// so shared group directories stay writable by peers (DB-16).
    fn lock_exclusive_mut(&self) -> Result<LockGuard> {
        Self::lock_exclusive_at(&self.lock_path, "registry")
    }

    /// Read and strictly validate the registry, or start empty.
    fn read_document(&self) -> Result<RegistryDoc> {
        let raw = match std::fs::read(&self.path) {
            Ok(raw) => raw,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                return Ok(empty_registry_doc())
            }
            Err(err) => {
                return Err(Error::Failure(format!(
                    "the registry file cannot be read, {err}"
                )))
            }
        };
        let text = decode_document_bytes(raw)
            .map_err(|detail| corrupt(format!("the document does not parse, {detail}")))?;
        let value: Value = serde_json::from_str(&text).map_err(|err| {
            let text = err.to_string();
            if let Some(key) = duplicate_key_of(&text) {
                corrupt(format!(
                    "a key stands twice in the document, {}",
                    duplicate_key_display(&key)
                ))
            } else {
                corrupt(format!("the document does not parse, {text}"))
            }
        })?;
        validate_registry_doc(&value)
    }

    /// The directory the registry file stands in, with a
    /// path carrying no directory part naming the current
    /// directory.
    fn parent_dir(&self) -> Option<PathBuf> {
        self.path.parent().map(|parent| {
            if parent.as_os_str().is_empty() {
                PathBuf::from(".")
            } else {
                parent.to_path_buf()
            }
        })
    }

    fn publish(&self, document: &RegistryDoc) -> Result<()> {
        let text = render_registry_doc(document) + "\n";
        let name = self
            .path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let (temp, mut handle) = create_publish_temp(&self.path, &name, &mut draw_publish_suffix)?;
        let staged = (|| -> std::io::Result<()> {
            handle.write_all(text.as_bytes())?;
            handle.sync_all()?;
            drop(handle);
            rename_temp_over_registry(&temp, &self.path)?;
            Ok(())
        })();
        if let Err(err) = staged {
            let _ = std::fs::remove_file(&temp);
            return Err(Error::Failure(format!(
                "the registry could not be published, {err}"
            )));
        }
        #[cfg(unix)]
        if let Some(dir) = self.parent_dir() {
            if let Ok(handle) = File::open(&dir) {
                let _ = handle.sync_all();
            }
        }
        Ok(())
    }

    fn mutate<T>(&self, action: impl FnOnce(&mut RegistryDoc) -> Result<T>) -> Result<T> {
        self.validate_registry_path()?;
        if let Some(parent) = self.parent_dir() {
            std::fs::create_dir_all(&parent).map_err(|err| {
                Error::Failure(format!("the registry directory cannot be created, {err}"))
            })?;
        }
        let guard = self.lock_exclusive_mut()?;
        let mut document = self.read_document()?;
        let result = action(&mut document)?;
        guard.verify()?;
        self.publish(&document)?;
        Ok(result)
    }

    /// Check whether neither the registry file nor its lock file
    /// stands on disk, while still reporting ancestor non-directory
    /// or permission errors as Error::Failure.
    fn both_registry_and_lock_absent(&self) -> Result<bool> {
        match std::fs::metadata(&self.path) {
            Ok(_) => Ok(false),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                for ancestor in self.path.ancestors().skip(1) {
                    if ancestor.as_os_str().is_empty() {
                        break;
                    }
                    match std::fs::metadata(ancestor) {
                        Ok(meta) if meta.is_dir() => break,
                        Ok(_) => {
                            return Err(Error::Failure(format!(
                                "the registry file cannot be read, {err}"
                            )));
                        }
                        Err(anc_err) if anc_err.kind() == std::io::ErrorKind::NotFound => continue,
                        Err(anc_err) => {
                            return Err(Error::Failure(format!(
                                "the registry file cannot be read, {anc_err}"
                            )));
                        }
                    }
                }
                Ok(std::fs::metadata(&self.lock_path)
                    .err()
                    .is_some_and(|e| e.kind() == std::io::ErrorKind::NotFound))
            }
            Err(err) => Err(Error::Failure(format!(
                "the registry file cannot be read, {err}"
            ))),
        }
    }

    fn read<T>(&self, action: impl FnOnce(&RegistryDoc) -> Result<T>) -> Result<T> {
        self.validate_registry_path()?;
        match std::fs::metadata(&self.path) {
            Ok(_) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                for ancestor in self.path.ancestors().skip(1) {
                    if ancestor.as_os_str().is_empty() {
                        break;
                    }
                    match std::fs::metadata(ancestor) {
                        Ok(meta) if meta.is_dir() => break,
                        Ok(_) => {
                            return Err(Error::Failure(format!(
                                "the registry file cannot be read, {err}"
                            )));
                        }
                        Err(anc_err) if anc_err.kind() == std::io::ErrorKind::NotFound => continue,
                        Err(anc_err) => {
                            return Err(Error::Failure(format!(
                                "the registry file cannot be read, {anc_err}"
                            )));
                        }
                    }
                }
                return action(&empty_registry_doc());
            }
            Err(err) => {
                return Err(Error::Failure(format!(
                    "the registry file cannot be read, {err}"
                )));
            }
        }
        let _guard = self.lock_shared_read()?;
        let document = self.read_document()?;
        action(&document)
    }

    fn last_seq(document: &RegistryDoc) -> i128 {
        let base = document
            .compaction
            .as_ref()
            .map_or(0i128, |c| c.through_seq as i128);
        base + document.history.len() as i128
    }

    fn ensure_seq_capacity(document: &RegistryDoc, count: usize) -> Result<()> {
        let last = Self::last_seq(document);
        if last
            .checked_add(count as i128)
            .is_none_or(|final_seq| final_seq > u64::MAX as i128)
        {
            return Err(Error::Refusal(
                "history sequence has reached the 64-bit limit".to_string(),
            ));
        }
        Ok(())
    }

    fn next_seq(document: &RegistryDoc) -> i128 {
        Self::last_seq(document) + 1
    }

    fn append_event(
        document: &mut RegistryDoc,
        kind: &str,
        item: &str,
        holder: Option<&str>,
        now: &Instant,
        epoch: i128,
        previous: Option<&str>,
    ) {
        let seq = Self::next_seq(document);
        document.history.push(Event {
            seq,
            kind: kind.to_string(),
            item: item.to_string(),
            holder: holder.map(str::to_string),
            at: format_instant(now),
            epoch,
            previous_holder: previous.map(str::to_string),
        });
    }

    fn lease_live(item: &Item, now: &Instant) -> Result<bool> {
        if item.status != "claimed" {
            return Ok(false);
        }
        let expires = match &item.expires_at {
            Some(text) => parse_stored_instant(&Value::Str(text.clone()))?,
            None => return Ok(false),
        };
        Ok(*now < expires)
    }

    fn completed_set(document: &RegistryDoc) -> HashSet<&str> {
        document
            .items
            .iter()
            .filter(|item| item.core.status == "completed")
            .map(|item| item.core.id.as_str())
            .collect()
    }

    fn is_item_blocked(item: &StoredItem, completed: &HashSet<&str>) -> bool {
        item.depends_on
            .iter()
            .any(|dep| !completed.contains(dep.as_str()))
    }

    fn item_effective_status(
        item: &StoredItem,
        live_claimed: bool,
        completed: &HashSet<&str>,
    ) -> &'static str {
        if item.core.status == "completed" {
            "completed"
        } else if live_claimed {
            "claimed"
        } else if Self::is_item_blocked(item, completed) {
            "blocked"
        } else if item
            .max_takeovers
            .is_some_and(|cap| item.takeover_count >= cap)
        {
            "exhausted"
        } else {
            "open"
        }
    }

    fn evaluate_item(
        item: &StoredItem,
        completed: &HashSet<&str>,
        now: &Instant,
    ) -> Result<ItemDetail> {
        let (status, holder, expires_at, live_claimed) = if item.core.status == "completed" {
            ("completed".to_string(), None, None, false)
        } else if Self::lease_live(&item.core, now)? {
            (
                "claimed".to_string(),
                item.core.holder.clone(),
                item.core.expires_at.clone(),
                true,
            )
        } else {
            ("open".to_string(), None, None, false)
        };
        let effective_status =
            Self::item_effective_status(item, live_claimed, completed).to_string();
        Ok(ItemDetail {
            id: item.core.id.clone(),
            note: item.core.note.clone(),
            status,
            effective_status,
            priority: item.priority,
            tags: item.tags.clone(),
            depends_on: item.depends_on.clone(),
            holder,
            expires_at,
            epoch: item.core.epoch,
            takeover_count: item.takeover_count,
            max_takeovers: item.max_takeovers,
        })
    }

    fn standing_lease_index(document: &RegistryDoc, item_id: &str, now: &Instant) -> Result<usize> {
        let index = match document
            .items
            .iter()
            .position(|item| item.core.id == item_id)
        {
            Some(index) => index,
            None => {
                return Err(Error::NotFound(format!(
                    "item {} does not stand in the backlog",
                    py_repr(item_id)
                )))
            }
        };
        let item = &document.items[index].core;
        if item.status == "completed" {
            return Err(Error::Refusal(format!(
                "item {} is completed and accepts no lease",
                py_repr(item_id)
            )));
        }
        if item.status != "claimed" {
            return Err(Error::Refusal(format!(
                "item {} stands open and holds no lease",
                py_repr(item_id)
            )));
        }
        if !Self::lease_live(item, now)? {
            return Err(Error::Refusal(format!(
                "the lease on item {} has expired and no longer stands",
                py_repr(item_id)
            )));
        }
        Ok(index)
    }

    fn validate_and_normalize_spec(spec: &OfferSpec) -> Result<OfferSpec> {
        validate_name_str(&spec.id, "item")?;
        validate_note_str(&spec.note)?;
        let priority = validate_priority(&Value::Int(spec.priority as i128))?;
        let tags = normalize_tags(&spec.tags)?;
        let depends_on = normalize_depends_on(&spec.id, &spec.depends_on)?;
        let max_takeovers = match spec.max_takeovers {
            Some(cap) => validate_max_takeovers(&Value::Int(cap as i128))?,
            None => None,
        };
        Ok(OfferSpec {
            id: spec.id.clone(),
            note: spec.note.clone(),
            priority,
            tags,
            depends_on,
            max_takeovers,
        })
    }

    fn verify_offer_batch_dag(
        standing_items: &[StoredItem],
        normalized_specs: &[OfferSpec],
    ) -> Result<()> {
        let total = standing_items.len() + normalized_specs.len();
        let mut index_by_id: HashMap<&str, usize> = HashMap::with_capacity(total);
        for (idx, item) in standing_items.iter().enumerate() {
            index_by_id.insert(item.core.id.as_str(), idx);
        }
        for (offset, spec) in normalized_specs.iter().enumerate() {
            if index_by_id.contains_key(spec.id.as_str()) {
                return Err(Error::Refusal(format!(
                    "item {} already stands in the backlog",
                    py_repr(&spec.id)
                )));
            }
            index_by_id.insert(spec.id.as_str(), standing_items.len() + offset);
        }
        let mut adj: Vec<Vec<usize>> = Vec::with_capacity(total);
        for item in standing_items {
            let mut edges = Vec::with_capacity(item.depends_on.len());
            for dep in &item.depends_on {
                if let Some(&dep_idx) = index_by_id.get(dep.as_str()) {
                    edges.push(dep_idx);
                }
            }
            adj.push(edges);
        }
        for spec in normalized_specs {
            let mut edges = Vec::with_capacity(spec.depends_on.len());
            for dep in &spec.depends_on {
                let Some(&dep_idx) = index_by_id.get(dep.as_str()) else {
                    return Err(Error::Refusal(format!(
                        "item {} depends on unknown item {}",
                        py_repr(&spec.id),
                        py_repr(dep)
                    )));
                };
                edges.push(dep_idx);
            }
            adj.push(edges);
        }
        if has_dependency_cycle(&adj) {
            return Err(Error::Refusal(
                "item dependencies contain a cycle".to_string(),
            ));
        }
        Ok(())
    }

    /// Add an item to the backlog and record the offered event.
    pub fn offer(&self, item_id: &str, note: &str, now: &Instant) -> Result<()> {
        self.offer_with(&OfferSpec::new(item_id, note), now)
    }

    /// Add an item with optional dibs-registry/2 priority, tags,
    /// dependencies, and max_takeovers (DB-21, DB-22, DB-23).
    pub fn offer_with(&self, spec: &OfferSpec, now: &Instant) -> Result<()> {
        let normalized = Self::validate_and_normalize_spec(spec)?;
        validate_now(now)?;
        self.mutate(|document| {
            Self::verify_offer_batch_dag(&document.items, std::slice::from_ref(&normalized))?;
            Self::ensure_seq_capacity(document, 1)?;
            let id = normalized.id.clone();
            document.items.push(StoredItem {
                core: Item {
                    id: normalized.id,
                    note: normalized.note,
                    status: "open".to_string(),
                    holder: None,
                    token_hash: None,
                    epoch: 0,
                    expires_at: None,
                },
                priority: normalized.priority,
                tags: normalized.tags,
                depends_on: normalized.depends_on,
                max_takeovers: normalized.max_takeovers,
                takeover_count: 0,
            });
            Self::append_event(document, "offered", &id, None, now, 0, None);
            Ok(())
        })
    }

    /// Add a batch of items to the backlog all or nothing under one
    /// exclusive lock and return the offered item ids in order (DB-18, DB-22, DB-23).
    pub fn offer_many(&self, specs: &[OfferSpec], now: &Instant) -> Result<Vec<String>> {
        if specs.is_empty() {
            return Err(Error::InvalidRequest(
                "offer batch requires at least one item".to_string(),
            ));
        }
        let mut seen_in_batch: HashSet<&str> = HashSet::with_capacity(specs.len());
        let mut normalized_specs = Vec::with_capacity(specs.len());
        for spec in specs {
            let normalized = Self::validate_and_normalize_spec(spec)?;
            if !seen_in_batch.insert(spec.id.as_str()) {
                return Err(Error::InvalidRequest(format!(
                    "duplicate item id {} in offer batch",
                    py_repr(&spec.id)
                )));
            }
            normalized_specs.push(normalized);
        }
        validate_now(now)?;
        self.mutate(|document| {
            Self::verify_offer_batch_dag(&document.items, &normalized_specs)?;
            Self::ensure_seq_capacity(document, normalized_specs.len())?;
            let mut offered = Vec::with_capacity(normalized_specs.len());
            for spec in normalized_specs {
                let id = spec.id.clone();
                document.items.push(StoredItem {
                    core: Item {
                        id: spec.id,
                        note: spec.note,
                        status: "open".to_string(),
                        holder: None,
                        token_hash: None,
                        epoch: 0,
                        expires_at: None,
                    },
                    priority: spec.priority,
                    tags: spec.tags,
                    depends_on: spec.depends_on,
                    max_takeovers: spec.max_takeovers,
                    takeover_count: 0,
                });
                Self::append_event(document, "offered", &id, None, now, 0, None);
                offered.push(id);
            }
            Ok(offered)
        })
    }

    fn grant_claim_at(
        document: &mut RegistryDoc,
        index: usize,
        holder: &str,
        ttl: i64,
        now: &Instant,
    ) -> Result<ClaimGranted> {
        let item_id = document.items[index].core.id.clone();
        let is_takeover = document.items[index].core.status == "claimed";
        let (kind, previous) = if is_takeover {
            ("taken_over", document.items[index].core.holder.clone())
        } else {
            ("claimed", None)
        };
        if document.items[index].core.epoch >= u64::MAX as i128 {
            return Err(Error::Refusal(format!(
                "item {} stands at epoch {}, and no epoch past it can be \
                 assigned, so the claim is refused",
                py_repr(&item_id),
                document.items[index].core.epoch
            )));
        }
        Self::ensure_seq_capacity(document, 1)?;
        let token = draw_token()?;
        let expires = expiry_after(now, ttl)?;
        let stored = &mut document.items[index];
        stored.core.status = "claimed".to_string();
        stored.core.holder = Some(holder.to_string());
        stored.core.token_hash = Some(token_hash(&token));
        stored.core.epoch = match stored.core.epoch.checked_add(1) {
            Some(epoch) => epoch,
            None => {
                return Err(corrupt(format!(
                    "item {} holds an invalid epoch",
                    py_repr(&item_id)
                )))
            }
        };
        if is_takeover {
            stored.takeover_count += 1;
        }
        stored.core.expires_at = Some(format_instant(&expires));
        let epoch = stored.core.epoch;
        Self::append_event(
            document,
            kind,
            &item_id,
            Some(holder),
            now,
            epoch,
            previous.as_deref(),
        );
        Ok(ClaimGranted {
            item: item_id,
            holder: holder.to_string(),
            token,
            epoch,
            expires_at: expires,
        })
    }

    /// Lease an open or expired ready item and return the one
    /// ClaimGranted (DB-1, DB-22, DB-23).
    pub fn claim(
        &self,
        item_id: &str,
        holder: &str,
        ttl: i64,
        now: &Instant,
    ) -> Result<ClaimGranted> {
        validate_name_str(item_id, "item")?;
        validate_name_str(holder, "holder")?;
        let ttl = validate_ttl_i64(ttl)?;
        validate_now(now)?;
        self.mutate(|document| {
            let index = match document
                .items
                .iter()
                .position(|item| item.core.id == item_id)
            {
                Some(index) => index,
                None => {
                    return Err(Error::NotFound(format!(
                        "item {} does not stand in the backlog",
                        py_repr(item_id)
                    )))
                }
            };
            if document.items[index].core.status == "completed" {
                return Err(Error::Refusal(format!(
                    "item {} is completed and accepts no claim",
                    py_repr(item_id)
                )));
            }
            if document.items[index].core.status == "claimed"
                && Self::lease_live(&document.items[index].core, now)?
            {
                return Err(Error::Refusal(format!(
                    "item {} is already claimed by {}",
                    py_repr(item_id),
                    py_repr(
                        document.items[index]
                            .core
                            .holder
                            .as_deref()
                            .unwrap_or_default()
                    )
                )));
            }
            let completed = Self::completed_set(document);
            if Self::is_item_blocked(&document.items[index], &completed) {
                return Err(Error::Refusal(format!(
                    "item {} is blocked by incomplete dependencies",
                    py_repr(item_id)
                )));
            }
            if document.items[index]
                .max_takeovers
                .is_some_and(|cap| document.items[index].takeover_count >= cap)
            {
                return Err(Error::Refusal(format!(
                    "item {} has reached max_takeovers and must be reset",
                    py_repr(item_id)
                )));
            }
            Self::grant_claim_at(document, index, holder, ttl, now)
        })
    }

    /// Lease the highest-priority ready item in offer order and
    /// return the issued ClaimGranted (DB-17, DB-23).
    pub fn claim_next(&self, holder: &str, ttl: i64, now: &Instant) -> Result<ClaimGranted> {
        self.claim_next_with_tags(holder, ttl, &[], now)
    }

    /// Lease the highest-priority ready item matching every tag in
    /// `tags` (with ties broken by offer order) and return the
    /// issued ClaimGranted (DB-17, DB-22, DB-23).
    pub fn claim_next_with_tags(
        &self,
        holder: &str,
        ttl: i64,
        tags: &[String],
        now: &Instant,
    ) -> Result<ClaimGranted> {
        validate_name_str(holder, "holder")?;
        let ttl = validate_ttl_i64(ttl)?;
        for tag in tags {
            validate_name_str(tag, "tag")?;
        }
        validate_now(now)?;
        let _ = expiry_after(now, ttl)?;
        self.validate_registry_path()?;
        if self.both_registry_and_lock_absent()? {
            return Err(Error::Refusal(
                "no ready item stands in the backlog".to_string(),
            ));
        }
        self.mutate(|document| {
            let completed = Self::completed_set(document);
            let mut best: Option<(u8, usize)> = None;
            for (index, item) in document.items.iter().enumerate() {
                if item.core.status == "completed" {
                    continue;
                }
                let live = Self::lease_live(&item.core, now)?;
                if Self::item_effective_status(item, live, &completed) != "open" {
                    continue;
                }
                if !tags.iter().all(|req| item.tags.binary_search(req).is_ok()) {
                    continue;
                }
                let candidate = (item.priority, index);
                if best.is_none_or(|current| candidate < current) {
                    best = Some(candidate);
                }
            }
            let Some((_, index)) = best else {
                return Err(Error::Refusal(
                    "no ready item stands in the backlog".to_string(),
                ));
            };
            Self::grant_claim_at(document, index, holder, ttl, now)
        })
    }

    /// Extend the standing lease from its current expiry and
    /// return the new expiry instant.
    pub fn renew(
        &self,
        item_id: &str,
        holder: &str,
        token: &str,
        epoch: i128,
        ttl: i64,
        now: &Instant,
    ) -> Result<Instant> {
        validate_name_str(item_id, "item")?;
        validate_name_str(holder, "holder")?;
        let ttl = validate_ttl_i64(ttl)?;
        validate_now(now)?;
        self.mutate(|document| {
            let index = Self::standing_lease_index(document, item_id, now)?;
            let item = &document.items[index].core;
            check_possession(item, holder, token, &Value::Int(epoch))?;
            let standing =
                parse_stored_instant(&Value::Str(item.expires_at.clone().unwrap_or_default()))?;
            let expires = expiry_after(&standing, ttl)?;
            Self::ensure_seq_capacity(document, 1)?;
            let rendered = format_instant(&expires);
            let standing_epoch = item.epoch;
            document.items[index].core.expires_at = Some(rendered);
            Self::append_event(
                document,
                "renewed",
                item_id,
                Some(holder),
                now,
                standing_epoch,
                None,
            );
            Ok(expires)
        })
    }

    fn end_lease(item: &mut Item, status: &str) {
        item.status = status.to_string();
        item.holder = None;
        item.token_hash = None;
        item.expires_at = None;
    }

    /// End the standing lease early and return the item to open.
    pub fn release(
        &self,
        item_id: &str,
        holder: &str,
        token: &str,
        epoch: i128,
        now: &Instant,
    ) -> Result<()> {
        validate_name_str(item_id, "item")?;
        validate_name_str(holder, "holder")?;
        validate_now(now)?;
        self.mutate(|document| {
            let index = Self::standing_lease_index(document, item_id, now)?;
            let item = &document.items[index].core;
            check_possession(item, holder, token, &Value::Int(epoch))?;
            Self::ensure_seq_capacity(document, 1)?;
            let standing_epoch = item.epoch;
            Self::end_lease(&mut document.items[index].core, "open");
            Self::append_event(
                document,
                "released",
                item_id,
                Some(holder),
                now,
                standing_epoch,
                None,
            );
            Ok(())
        })
    }

    /// Mark the item done under its standing lease. Completion is
    /// terminal, and the item accepts no claim afterwards.
    pub fn complete(
        &self,
        item_id: &str,
        holder: &str,
        token: &str,
        epoch: i128,
        now: &Instant,
    ) -> Result<()> {
        validate_name_str(item_id, "item")?;
        validate_name_str(holder, "holder")?;
        validate_now(now)?;
        self.mutate(|document| {
            let index = Self::standing_lease_index(document, item_id, now)?;
            let item = &document.items[index].core;
            check_possession(item, holder, token, &Value::Int(epoch))?;
            Self::ensure_seq_capacity(document, 1)?;
            let standing_epoch = item.epoch;
            Self::end_lease(&mut document.items[index].core, "completed");
            Self::append_event(
                document,
                "completed",
                item_id,
                Some(holder),
                now,
                standing_epoch,
                None,
            );
            Ok(())
        })
    }

    /// Reset an exhausted item back to open with takeover_count 0
    /// while preserving its monotonic epoch (DB-23).
    pub fn reset(&self, item_id: &str, now: &Instant) -> Result<ItemDetail> {
        validate_name_str(item_id, "item")?;
        validate_now(now)?;
        self.validate_registry_path()?;
        if self.both_registry_and_lock_absent()? {
            return Err(Error::NotFound(format!(
                "item {} does not stand in the backlog",
                py_repr(item_id)
            )));
        }
        self.mutate(|document| {
            let index = match document
                .items
                .iter()
                .position(|item| item.core.id == item_id)
            {
                Some(index) => index,
                None => {
                    return Err(Error::NotFound(format!(
                        "item {} does not stand in the backlog",
                        py_repr(item_id)
                    )))
                }
            };
            let live = Self::lease_live(&document.items[index].core, now)?;
            let completed = Self::completed_set(document);
            match Self::item_effective_status(&document.items[index], live, &completed) {
                "exhausted" => {}
                "completed" => {
                    return Err(Error::Refusal(format!(
                        "item {} is completed and cannot be reset",
                        py_repr(item_id)
                    )));
                }
                "claimed" => {
                    return Err(Error::Refusal(format!(
                        "item {} holds a live lease by {} and cannot be reset",
                        py_repr(item_id),
                        py_repr(
                            document.items[index]
                                .core
                                .holder
                                .as_deref()
                                .unwrap_or_default()
                        )
                    )));
                }
                "blocked" => {
                    return Err(Error::Refusal(format!(
                        "item {} is blocked by incomplete dependencies and is not exhausted",
                        py_repr(item_id)
                    )));
                }
                _ => {
                    return Err(Error::Refusal(format!(
                        "item {} is not exhausted and needs no reset",
                        py_repr(item_id)
                    )));
                }
            }
            Self::ensure_seq_capacity(document, 1)?;
            let previous_holder = document.items[index].core.holder.clone();
            let standing_epoch = document.items[index].core.epoch;
            Self::end_lease(&mut document.items[index].core, "open");
            document.items[index].takeover_count = 0;
            document.is_v2 = true;
            Self::append_event(
                document,
                "reset",
                item_id,
                None,
                now,
                standing_epoch,
                previous_holder.as_deref(),
            );
            let completed_after = Self::completed_set(document);
            Self::evaluate_item(&document.items[index], &completed_after, now)
        })
    }

    /// Return the non-completed items standing on the board in offer
    /// order, reporting effective status at the instant (DB-10, DB-22, DB-23).
    pub fn board(&self, now: &Instant) -> Result<Vec<BoardRow>> {
        validate_now(now)?;
        self.read(|document| {
            let completed = Self::completed_set(document);
            let mut rows = Vec::with_capacity(document.items.len());
            for item in &document.items {
                if item.core.status != "completed" {
                    let live = Self::lease_live(&item.core, now)?;
                    let eff = Self::item_effective_status(item, live, &completed);
                    let (holder, expires_at) = if live {
                        (item.core.holder.clone(), item.core.expires_at.clone())
                    } else {
                        (None, None)
                    };
                    rows.push(BoardRow {
                        id: item.core.id.clone(),
                        note: item.core.note.clone(),
                        status: eff.to_string(),
                        holder,
                        expires_at,
                        epoch: item.core.epoch,
                    });
                }
            }
            Ok(rows)
        })
    }

    /// Return all ready items matching every tag in `tags`, sorted by
    /// `(priority ASC, offer_order ASC)` without creating a lock file (DB-22, DB-23).
    pub fn ready(&self, tags: &[String], now: &Instant) -> Result<Vec<ItemDetail>> {
        for tag in tags {
            validate_name_str(tag, "tag")?;
        }
        validate_now(now)?;
        self.read(|document| {
            let completed = Self::completed_set(document);
            let mut candidates: Vec<(u8, usize, ItemDetail)> = Vec::new();
            for (index, item) in document.items.iter().enumerate() {
                if item.core.status == "completed" {
                    continue;
                }
                if !tags.iter().all(|req| item.tags.binary_search(req).is_ok()) {
                    continue;
                }
                let detail = Self::evaluate_item(item, &completed, now)?;
                if detail.effective_status == "open" {
                    candidates.push((item.priority, index, detail));
                }
            }
            candidates.sort_by_key(|(prio, idx, _)| (*prio, *idx));
            Ok(candidates
                .into_iter()
                .map(|(_, _, detail)| detail)
                .collect())
        })
    }

    /// Return the full detail of one item evaluated at the instant (DB-19).
    pub fn show(&self, item_id: &str, now: &Instant) -> Result<ItemDetail> {
        validate_name_str(item_id, "item")?;
        validate_now(now)?;
        self.read(|document| {
            let Some(item) = document.items.iter().find(|item| item.core.id == item_id) else {
                return Err(Error::NotFound(format!(
                    "item {} does not stand in the backlog",
                    py_repr(item_id)
                )));
            };
            let completed = Self::completed_set(document);
            Self::evaluate_item(item, &completed, now)
        })
    }

    /// Return the full detail summaries of the non completed items
    /// standing on the board in offer order, evaluated at the instant.
    pub fn board_details(&self, now: &Instant) -> Result<Vec<ItemDetail>> {
        validate_now(now)?;
        self.read(|document| {
            let completed = Self::completed_set(document);
            let mut details = Vec::new();
            for item in &document.items {
                if item.core.status != "completed" {
                    details.push(Self::evaluate_item(item, &completed, now)?);
                }
            }
            Ok(details)
        })
    }

    /// Return the recorded events in order, or those of one item.
    pub fn history(&self, item_id: Option<&str>) -> Result<Vec<Event>> {
        if let Some(item_id) = item_id {
            validate_name_str(item_id, "item")?;
        }
        self.read(|document| {
            if let Some(item_id) = item_id {
                if !document.items.iter().any(|item| item.core.id == item_id) {
                    return Err(Error::NotFound(format!(
                        "item {} does not stand in the backlog",
                        py_repr(item_id)
                    )));
                }
            }
            Ok(document
                .history
                .iter()
                .filter(|event| item_id.is_none() || event.item == item_id.unwrap())
                .cloned()
                .collect())
        })
    }

    /// Crash-idempotently archive history events through `through_seq`
    /// into `archive_path` and publish a dibs-registry/2 compaction
    /// baseline (DB-24).
    pub fn compact(
        &self,
        through_seq: u64,
        archive_path: &Path,
        now: &Instant,
    ) -> Result<CompactSummary> {
        let through_seq = validate_through_seq(&Value::Int(through_seq as i128))?;
        validate_now(now)?;
        self.validate_registry_path()?;
        self.validate_archive_path(archive_path)?;
        if self.both_registry_and_lock_absent()? {
            return Err(Error::Refusal(format!(
                "requested through_seq {through_seq} exceeds the last recorded seq 0"
            )));
        }

        let archive_parent = archive_path.parent().map(|parent| {
            if parent.as_os_str().is_empty() {
                PathBuf::from(".")
            } else {
                parent.to_path_buf()
            }
        });
        if let Some(ref parent) = archive_parent {
            std::fs::create_dir_all(parent).map_err(|err| {
                Error::Failure(format!("the archive directory cannot be created, {err}"))
            })?;
        }
        self.validate_archive_path(archive_path)?;

        let archive_lock_path = lock_path_for(archive_path);
        let reg_phys = resolve_physical_path(&self.lock_path);
        let arch_phys = resolve_physical_path(&archive_lock_path);
        let (reg_guard, arch_guard) = if reg_phys <= arch_phys {
            let rg = self.lock_exclusive_mut()?;
            let ag = Self::lock_exclusive_at(&archive_lock_path, "archive")?;
            (rg, ag)
        } else {
            let ag = Self::lock_exclusive_at(&archive_lock_path, "archive")?;
            let rg = self.lock_exclusive_mut()?;
            (rg, ag)
        };
        // Re-verify archive path and lock non-aliasing now that both `.lock` files stand on disk.
        self.validate_archive_path(archive_path)?;
        let mut document = self.read_document()?;

        let prev_through_seq = document.compaction.as_ref().map_or(0u64, |c| c.through_seq);
        if prev_through_seq >= through_seq {
            return Err(Error::Refusal(format!(
                "the registry is already compacted through seq {prev_through_seq}"
            )));
        }
        let last_seq = document
            .history
            .last()
            .map_or(prev_through_seq, |ev| ev.seq as u64);
        if through_seq > last_seq {
            return Err(Error::Refusal(format!(
                "requested through_seq {through_seq} exceeds the last recorded seq {last_seq}"
            )));
        }

        let existing_bytes = match std::fs::read(archive_path) {
            Ok(bytes) => bytes,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                if prev_through_seq > 0 {
                    return Err(Error::Refusal(format!(
                        "the archive file is missing events 1 through {prev_through_seq} from the standing compaction"
                    )));
                }
                Vec::new()
            }
            Err(err) => {
                return Err(Error::Failure(format!(
                    "the archive file cannot be read, {err}"
                )));
            }
        };

        if existing_bytes.contains(&b'\r') {
            return Err(Error::Refusal(
                "the archive file is not canonical JSON Lines".to_string(),
            ));
        }
        let (complete_bytes, trailing_partial): (&[u8], &[u8]) = if existing_bytes.is_empty() {
            (&[], &[])
        } else if existing_bytes.ends_with(b"\n") {
            (&existing_bytes, &[])
        } else {
            match existing_bytes.iter().rposition(|&b| b == b'\n') {
                Some(last_nl) => (&existing_bytes[..=last_nl], &existing_bytes[last_nl + 1..]),
                None => (&[], &existing_bytes),
            }
        };

        let complete_text = std::str::from_utf8(complete_bytes)
            .map_err(|_| Error::Refusal("the archive file is not valid UTF-8".to_string()))?;
        let mut existing_lines: Vec<&str> = Vec::new();
        let mut parsed_archive_events: Vec<Event> = Vec::new();
        if !complete_text.is_empty() {
            existing_lines = complete_text[..complete_text.len() - 1]
                .split('\n')
                .collect();
            parsed_archive_events.reserve(existing_lines.len());
            for (idx, line) in existing_lines.iter().enumerate() {
                let line_no = idx + 1;
                let expected_seq = line_no as i128;
                let val: Value = serde_json::from_str(line).map_err(|err| {
                    let err_text = err.to_string();
                    if let Some(key) = duplicate_key_of(&err_text) {
                        Error::Refusal(format!(
                            "archive line {line_no} carries duplicate key {}",
                            duplicate_key_display(&key)
                        ))
                    } else {
                        Error::Refusal(format!("archive line {line_no} is not valid JSON"))
                    }
                })?;
                let ev = validate_event_with(&val, expected_seq, true).map_err(|err| {
                    Error::Refusal(format!(
                        "archive line {line_no} is invalid, {}",
                        err.message()
                    ))
                })?;
                if render_event_jsonl_line(&ev) != *line {
                    return Err(Error::Refusal(format!(
                        "archive line {line_no} is not in canonical JSON Lines form"
                    )));
                }
                parsed_archive_events.push(ev);
            }
        }

        let k = parsed_archive_events.len() as u64;
        if k < prev_through_seq {
            return Err(Error::Refusal(format!(
                "the archive file holds {k} events, fewer than the standing compaction through seq {prev_through_seq}"
            )));
        }
        if let Some(prev) = &document.compaction {
            let prefix_byte_len: usize = existing_lines[..prev.through_seq as usize]
                .iter()
                .map(|line| line.len() + 1)
                .sum();
            let prefix_sha = sha256_hex_slices(&complete_bytes[..prefix_byte_len], &[]);
            if prefix_sha != prev.archived_events_sha256 {
                return Err(Error::Refusal(
                    "the archive file digest does not match the standing compaction".to_string(),
                ));
            }
        }
        if k > through_seq {
            return Err(Error::Refusal(format!(
                "the archive file already holds {k} events, beyond requested through_seq {through_seq}"
            )));
        }
        for seq_num in (prev_through_seq + 1)..=k {
            let archive_ev = &parsed_archive_events[(seq_num - 1) as usize];
            let standing_ev = &document.history[(seq_num - prev_through_seq - 1) as usize];
            if archive_ev != standing_ev {
                return Err(Error::Refusal(format!(
                    "archive event at seq {seq_num} disagrees with the standing registry history"
                )));
            }
        }

        let mut tail_bytes = Vec::new();
        let tail_start_seq = if !trailing_partial.is_empty() {
            if k >= through_seq {
                return Err(Error::Refusal(format!(
                    "the archive file holds unterminated bytes past requested through_seq {through_seq}"
                )));
            }
            let next_ev = &document.history[(k + 1 - prev_through_seq - 1) as usize];
            let expected_line = render_event_jsonl_line(next_ev);
            if !expected_line.as_bytes().starts_with(trailing_partial) {
                return Err(Error::Refusal(
                    "the archive file is not canonical JSON Lines".to_string(),
                ));
            }
            tail_bytes.extend_from_slice(&expected_line.as_bytes()[trailing_partial.len()..]);
            tail_bytes.push(b'\n');
            k + 2
        } else {
            k + 1
        };
        for seq_num in tail_start_seq..=through_seq {
            let ev = &document.history[(seq_num - prev_through_seq - 1) as usize];
            let line = render_event_jsonl_line(ev);
            tail_bytes.extend_from_slice(line.as_bytes());
            tail_bytes.push(b'\n');
        }

        let split_count = (through_seq - prev_through_seq) as usize;
        let events_to_archive = &document.history[..split_count];
        let remaining_events = document.history[split_count..].to_vec();

        let mut baseline_map: BTreeMap<String, BaselineEntry> = BTreeMap::new();
        if let Some(prev) = &document.compaction {
            for entry in &prev.baseline {
                baseline_map.insert(entry.id.clone(), entry.clone());
            }
        } else {
            let mut events_by_item: BTreeMap<&str, Vec<&Event>> = BTreeMap::new();
            for ev in &document.history {
                events_by_item.entry(ev.item.as_str()).or_default().push(ev);
            }
            // Seed pre-history state for any uncompacted item whose recorded
            // history is empty or begins mid-life (without an initial offered event).
            for item in &document.items {
                let item_events = events_by_item
                    .get(item.core.id.as_str())
                    .map(Vec::as_slice)
                    .unwrap_or(&[]);
                match item_events.first().copied() {
                    None => {
                        baseline_map.insert(
                            item.core.id.clone(),
                            BaselineEntry {
                                id: item.core.id.clone(),
                                status: item.core.status.clone(),
                                holder: item.core.holder.clone(),
                                epoch: item.core.epoch,
                                takeover_count: item.takeover_count,
                            },
                        );
                    }
                    Some(ev0) if ev0.kind == "offered" => {}
                    Some(ev0) => {
                        let initial_takeover_count = if let Some(reset_pos) =
                            item_events.iter().position(|ev| ev.kind == "reset")
                        {
                            let cap = item.max_takeovers.unwrap_or(0);
                            let takeovers_before_reset = item_events[..reset_pos]
                                .iter()
                                .filter(|ev| ev.kind == "taken_over")
                                .count()
                                as u64;
                            cap.saturating_sub(takeovers_before_reset)
                        } else {
                            let total_takeovers = item_events
                                .iter()
                                .filter(|ev| ev.kind == "taken_over")
                                .count() as u64;
                            item.takeover_count.saturating_sub(total_takeovers)
                        };
                        let (status, holder, epoch) = match ev0.kind.as_str() {
                            "claimed" => ("open".to_string(), None, ev0.epoch.saturating_sub(1)),
                            "taken_over" => (
                                "claimed".to_string(),
                                ev0.previous_holder.clone(),
                                ev0.epoch.saturating_sub(1),
                            ),
                            "reset" => {
                                if ev0.previous_holder.is_some() {
                                    (
                                        "claimed".to_string(),
                                        ev0.previous_holder.clone(),
                                        ev0.epoch,
                                    )
                                } else {
                                    ("open".to_string(), None, ev0.epoch)
                                }
                            }
                            _ => ("claimed".to_string(), ev0.holder.clone(), ev0.epoch),
                        };
                        baseline_map.insert(
                            item.core.id.clone(),
                            BaselineEntry {
                                id: item.core.id.clone(),
                                status,
                                holder,
                                epoch,
                                takeover_count: initial_takeover_count,
                            },
                        );
                    }
                }
            }
        }
        for ev in events_to_archive {
            match baseline_map.get_mut(&ev.item) {
                None => {
                    let entry = BaselineEntry {
                        id: ev.item.clone(),
                        status: "open".to_string(),
                        holder: None,
                        epoch: 0,
                        takeover_count: 0,
                    };
                    baseline_map.insert(ev.item.clone(), entry);
                }
                Some(entry) => match ev.kind.as_str() {
                    "claimed" => {
                        entry.status = "claimed".to_string();
                        entry.holder = ev.holder.clone();
                        entry.epoch = ev.epoch;
                    }
                    "taken_over" => {
                        entry.status = "claimed".to_string();
                        entry.holder = ev.holder.clone();
                        entry.epoch = ev.epoch;
                        entry.takeover_count += 1;
                    }
                    "renewed" => {
                        entry.status = "claimed".to_string();
                        entry.holder = ev.holder.clone();
                        entry.epoch = ev.epoch;
                    }
                    "released" => {
                        entry.status = "open".to_string();
                        entry.holder = None;
                        entry.epoch = ev.epoch;
                    }
                    "completed" => {
                        entry.status = "completed".to_string();
                        entry.holder = None;
                        entry.epoch = ev.epoch;
                    }
                    "reset" => {
                        entry.status = "open".to_string();
                        entry.holder = None;
                        entry.epoch = ev.epoch;
                        entry.takeover_count = 0;
                    }
                    _ => {}
                },
            }
        }

        let archived_events_sha256 = sha256_hex_slices(&existing_bytes, &tail_bytes);
        let new_compaction = Compaction {
            through_seq,
            archived_events_sha256: archived_events_sha256.clone(),
            compacted_at: format_instant(now),
            baseline: baseline_map.into_values().collect(),
        };
        let mut rendered_comp = String::new();
        render_compaction(&new_compaction, 0, &mut rendered_comp);
        let comp_val: Value = serde_json::from_str(&rendered_comp).map_err(|_| {
            corrupt("the synthesized compaction baseline could not be parsed".to_string())
        })?;
        validate_compaction(&comp_val)?;
        validate_history_against_items_v2(
            &document.items,
            Some(&new_compaction),
            &remaining_events,
        )?;

        let mut archive_file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(archive_path)
            .map_err(|err| Error::Failure(format!("the archive file cannot be written, {err}")))?;
        if !tail_bytes.is_empty() {
            archive_file.write_all(&tail_bytes).map_err(|err| {
                Error::Failure(format!("the archive file cannot be written, {err}"))
            })?;
        }
        archive_file
            .sync_all()
            .map_err(|err| Error::Failure(format!("the archive file cannot be synced, {err}")))?;
        drop(archive_file);
        #[cfg(unix)]
        if let Some(dir) = archive_parent {
            let handle = File::open(&dir).map_err(|err| {
                Error::Failure(format!(
                    "the archive directory cannot be opened for sync, {err}"
                ))
            })?;
            handle.sync_all().map_err(|err| {
                Error::Failure(format!("the archive directory cannot be synced, {err}"))
            })?;
        }

        let archived_count = through_seq - prev_through_seq;
        let remaining_count = remaining_events.len() as u64;
        document.is_v2 = true;
        document.compaction = Some(new_compaction);
        document.history = remaining_events;

        reg_guard.verify()?;
        arch_guard.verify()?;
        self.publish(&document)?;

        Ok(CompactSummary {
            through_seq,
            archived_count,
            remaining_count,
            archived_events_sha256,
            archive_path: archive_path.to_path_buf(),
        })
    }
}

/// Check a presented holder and token and epoch against the
/// standing lease of an item, in the order the protocol checks
/// them, holder first and epoch second and token last. The
/// presented epoch arrives as a value so a presentation that is
/// not an integer at all refuses with the same message the
/// integer mismatch raises.
pub fn check_possession(item: &Item, holder: &str, token: &str, epoch: &Value) -> Result<()> {
    if Some(holder) != item.holder.as_deref() {
        return Err(Error::Refusal(match &item.holder {
            Some(standing) => format!(
                "item {} is held by {}, not by {}",
                py_repr(&item.id),
                py_repr(standing),
                py_repr(holder)
            ),
            None => format!(
                "item {} is held by no one, not by {}",
                py_repr(&item.id),
                py_repr(holder)
            ),
        }));
    }
    let epoch_matches = match epoch {
        Value::Int(number) => *number == item.epoch,
        _ => false,
    };
    if !epoch_matches {
        return Err(Error::Refusal(format!(
            "the epoch {} presented for item {} does not match the standing epoch {}",
            py_repr_value(epoch),
            py_repr(&item.id),
            item.epoch
        )));
    }
    let stored = item.token_hash.as_deref().unwrap_or_default();
    if !token_matches(token, stored) {
        return Err(Error::Refusal(format!(
            "the token presented for item {} does not match the lease",
            py_repr(&item.id)
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parent_dir_names_the_current_directory_for_a_bare_name() {
        let registry = Registry::new(PathBuf::from("registry.json"));
        assert_eq!(registry.parent_dir(), Some(PathBuf::from(".")));
    }

    #[test]
    fn parent_dir_returns_a_nested_parent_unchanged() {
        let registry = Registry::new(PathBuf::from("a/b/registry.json"));
        assert_eq!(registry.parent_dir(), Some(PathBuf::from("a/b")));
    }

    #[test]
    fn parent_dir_returns_the_root_for_a_root_level_file() {
        let registry = Registry::new(PathBuf::from("/registry.json"));
        assert_eq!(registry.parent_dir(), Some(PathBuf::from("/")));
    }

    /// The publish temporary name carries the process id and a
    /// drawn suffix, so no outside test can know the name a
    /// publish will choose and plant a file at it. The open the
    /// publish uses is pinned here instead, at the helper the
    /// publish itself calls.
    ///
    /// DB-S1 gates this pin to Unix targets (and scopes `Read`
    /// inside it) because it stages a symlink via
    /// `std::os::unix::fs::symlink`, which does not exist on the
    /// Windows CI target.
    #[cfg(unix)]
    #[test]
    fn publish_temp_open_refuses_a_planted_symlink() {
        use std::io::Read;

        let dir = std::env::temp_dir().join(format!("dibs-unit-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let victim = dir.join("victim.txt");
        std::fs::write(&victim, "victim bytes").unwrap();
        let temp = dir.join("staged.tmp");
        std::os::unix::fs::symlink(&victim, &temp).unwrap();
        // A plant at the name fails the open, and the victim
        // keeps its bytes.
        assert!(open_publish_temp(&temp).is_err());
        let mut text = String::new();
        File::open(&victim)
            .unwrap()
            .read_to_string(&mut text)
            .unwrap();
        assert_eq!(text, "victim bytes");
        // A name nothing stands at opens and takes the write.
        std::fs::remove_file(&temp).unwrap();
        let mut handle = open_publish_temp(&temp).unwrap();
        handle.write_all(b"staged bytes").unwrap();
        drop(handle);
        assert_eq!(std::fs::read(&temp).unwrap(), b"staged bytes");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A publish whose drawn temporary name already stands
    /// redraws instead of failing. The name a real publish
    /// chooses cannot be known outside it, so the draw is
    /// scripted here, at the helper the publish itself calls.
    #[test]
    fn publish_temp_create_redraws_on_a_collision() {
        let dir =
            std::env::temp_dir().join(format!("dibs-unit-test-collision-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let registry = dir.join("registry.json");
        let planted_suffix = [0xab; 4];
        let planted_hex: String = planted_suffix
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        let planted = dir.join(format!(
            ".registry.json.tmp-{}-{planted_hex}",
            std::process::id()
        ));
        std::fs::write(&planted, "planted bytes").unwrap();
        // Two draws land on the plant, the third lands clear,
        // and the create succeeds at the third name with the
        // plant untouched.
        let mut draws = vec![planted_suffix, planted_suffix, [0xcd; 4]].into_iter();
        let mut draw = || Ok(draws.next().unwrap());
        let (temp, mut handle) =
            create_publish_temp(&registry, "registry.json", &mut draw).unwrap();
        assert_ne!(temp, planted);
        handle.write_all(b"staged bytes").unwrap();
        drop(handle);
        assert_eq!(std::fs::read(&temp).unwrap(), b"staged bytes");
        assert_eq!(std::fs::read(&planted).unwrap(), b"planted bytes");
        // A draw that never leaves the plant exhausts the bound
        // and fails as a failure, with the plant still untouched.
        let mut calls = 0;
        let error = {
            let mut stuck = || {
                calls += 1;
                Ok(planted_suffix)
            };
            create_publish_temp(&registry, "registry.json", &mut stuck)
                .err()
                .unwrap()
        };
        assert!(matches!(error, Error::Failure(_)), "{error:?}");
        assert_eq!(calls, 8);
        assert_eq!(std::fs::read(&planted).unwrap(), b"planted bytes");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The DB-14 table in CONTRACT.md states the key constants
    /// of this crate exactly, in construction order, with the
    /// status and kind vocabularies beside them. The pin stands
    /// inside the crate because the constants are private.
    #[test]
    fn contract_field_table_matches_the_key_constants() {
        let text = std::fs::read_to_string(
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("CONTRACT.md"),
        )
        .unwrap();
        let start = text.find("Obligation DB-14 covers").unwrap();
        let after = &text[start..];
        let end = after.find("The cost of working a registry").unwrap();
        let db14 = after[..end].replace('\n', " ");
        let field_list = |anchor: &str| -> Vec<String> {
            let at = db14.find(anchor).unwrap();
            let rest = &db14[at + anchor.len()..];
            let stop = rest.find(", in that construction order").unwrap();
            let phrase = rest[..stop].strip_prefix("exactly the fields ").unwrap();
            phrase
                .split(" and ")
                .flat_map(|part| part.split(", "))
                .map(|name| name.to_string())
                .collect()
        };
        let vocabulary = |anchor: &str| -> Vec<String> {
            let at = db14.find(anchor).unwrap();
            let rest = &db14[at + anchor.len()..];
            let stop = rest.find('.').unwrap();
            rest[..stop]
                .split(" and ")
                .map(|name| name.to_string())
                .collect()
        };
        let assert_order = |stated: Vec<String>, constants: &[&str]| {
            let expected: Vec<String> = constants.iter().map(|c| c.to_string()).collect();
            assert_eq!(stated, expected);
        };
        assert_order(
            field_list("document is one JSON object carrying "),
            &DOCUMENT_KEYS,
        );
        assert_order(field_list("Each item object carries "), &ITEM_KEYS);
        assert_order(field_list("Each event object carries "), &EVENT_KEYS);
        assert_order(
            vocabulary("The status field carries one of "),
            &ITEM_STATUSES,
        );
        assert_order(vocabulary("The kind field carries one of "), &EVENT_KINDS);
    }

    #[test]
    fn a_clock_standing_before_the_epoch_answers_the_failure() {
        let read = (std::time::UNIX_EPOCH - std::time::Duration::from_secs(1))
            .duration_since(std::time::UNIX_EPOCH);
        assert!(read.is_err());
        let error = Instant::from_clock_read(read).unwrap_err();
        assert!(matches!(error, Error::Failure(_)));
        assert_eq!(error.message(), "the system clock stands before the epoch");
        assert_eq!(error.exit_code(), 1);
    }

    #[test]
    fn a_clock_at_or_after_the_epoch_builds_its_instant() {
        let at_epoch =
            Instant::from_clock_read(Ok(std::time::Duration::ZERO)).expect("the epoch read builds");
        assert_eq!(
            at_epoch,
            Instant::from_epoch_micros(0).expect("the epoch count builds")
        );
        let after = Instant::from_clock_read(Ok(std::time::Duration::from_micros(90_000_000)))
            .expect("a later read builds");
        assert_eq!(
            after,
            Instant::from_epoch_micros(90_000_000).expect("the later count builds")
        );
    }
}
