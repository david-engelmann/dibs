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

use std::collections::HashSet;
use std::fmt;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::os::unix::fs::MetadataExt;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use fs2::FileExt;
use sha2::{Digest, Sha256};

pub const FORMAT_NAME: &str = "dibs-registry/1";
pub const LOCK_SUFFIX: &str = ".lock";
pub const NAME_MAX: usize = 64;
pub const NOTE_MAX: usize = 200;
pub const TOKEN_BYTES: usize = 32;
pub const TTL_MAX: i64 = 31_536_000;

const DOCUMENT_KEYS: [&str; 3] = ["format", "items", "history"];
const ITEM_KEYS: [&str; 7] = [
    "id",
    "note",
    "status",
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
/// plain space are not.
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

    /// The instant the system clock reports now, in UTC. Only the
    /// command line uses this, and it offers no override.
    pub fn now_utc() -> Instant {
        let elapsed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default();
        let total_micros = elapsed.as_secs() as i128 * 1_000_000 + elapsed.subsec_micros() as i128;
        let days = total_micros.div_euclid(DAY_MICROS);
        let rest = total_micros.rem_euclid(DAY_MICROS);
        Instant {
            days: days as i64,
            secs: (rest / 1_000_000) as i64,
            micros: (rest % 1_000_000) as i64,
            offset_micros: Some(0),
        }
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
    /// no fraction. The fraction in the Python grammar always
    /// lands on the seconds, whatever unit it follows.
    fn fraction_micros(&mut self) -> Option<i64> {
        match self.peek() {
            Some('.') | Some(',') => {
                self.take();
                let mut micros: i64 = 0;
                let mut place: i64 = 100_000;
                while let Some(c) = self.peek() {
                    if !c.is_ascii_digit() {
                        break;
                    }
                    self.take();
                    if place > 0 {
                        micros += (c as i64 - '0' as i64) * place;
                        place /= 10;
                    }
                }
                Some(micros)
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
        if !(1..=9999).contains(&year)
            || !(1..=12).contains(&month)
            || day < 1
            || day > days_in_month(year, month)
        {
            return None;
        }
        Some(days_from_civil(year, month, day))
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
        if !(1..=9999).contains(&year)
            || !(1..=12).contains(&month)
            || day < 1
            || day > days_in_month(year, month)
        {
            return None;
        }
        Some(days_from_civil(year, month, day))
    }
}

/// Parse the time and offset after a date and its separator, or
/// the bare end of the string, and build the instant. The
/// separator has already been consumed by the caller.
fn parse_time_and_offset(cursor: &mut Cursor, days: i64) -> Option<Instant> {
    let hour = cursor.digits(2)?;
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
            let mut minutes = 0i64;
            let mut seconds = 0i64;
            if cursor.peek() == Some(':') {
                cursor.take();
                minutes = cursor.digits(2)?;
                if cursor.peek() == Some(':') {
                    cursor.take();
                    seconds = cursor.digits(2)?;
                }
            } else if cursor.peek().is_some_and(|c| c.is_ascii_digit()) {
                minutes = cursor.digits(2)?;
                if cursor.peek().is_some_and(|c| c.is_ascii_digit()) {
                    seconds = cursor.digits(2)?;
                }
            }
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
    let end = rest.find(" at line ").unwrap_or(rest.len());
    Some(rest[..end].to_string())
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

fn empty_document() -> Document {
    Document {
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

fn validate_item(value: &Value) -> Result<Item> {
    let pairs = match value {
        Value::Object(pairs) if keys_exact(pairs, &ITEM_KEYS) => pairs,
        _ => {
            return Err(corrupt(
                "an item does not carry exactly the recorded fields".to_string(),
            ))
        }
    };
    let id_value = object_get(pairs, "id").unwrap();
    validate_stored_name(id_value, "item id", "an item")?;
    let id = match id_value {
        Value::Str(text) => text.clone(),
        _ => unreachable!(),
    };
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
            match other {
                Value::Str(text) => Some(text.clone()),
                _ => unreachable!(),
            }
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
            match other {
                Value::Str(text) => Some(text.clone()),
                _ => unreachable!(),
            }
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

fn is_token_hash(text: &str) -> bool {
    text.len() == 64
        && text
            .chars()
            .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c))
}

fn validate_event(value: &Value, position: usize) -> Result<Event> {
    let pairs = match value {
        Value::Object(pairs) if keys_exact(pairs, &EVENT_KEYS) => pairs,
        _ => {
            return Err(corrupt(
                "a history event does not carry exactly the recorded fields".to_string(),
            ))
        }
    };
    let seq = match object_get(pairs, "seq").unwrap() {
        Value::Int(number) if *number == position as i128 + 1 => *number,
        _ => {
            return Err(corrupt(
                "the history sequence does not grow by 1 from 1 without gaps".to_string(),
            ))
        }
    };
    let kind = match object_get(pairs, "kind").unwrap() {
        Value::Str(text) if EVENT_KINDS.contains(&text.as_str()) => text.clone(),
        other => {
            return Err(corrupt(format!(
                "a history event holds an unknown kind {}",
                py_repr_value(other)
            )))
        }
    };
    let item_value = object_get(pairs, "item").unwrap();
    validate_stored_name(item_value, "item id", "a history event")?;
    let item = match item_value {
        Value::Str(text) => text.clone(),
        _ => unreachable!(),
    };
    let holder_value = object_get(pairs, "holder").unwrap();
    let holder = if kind == "offered" {
        if holder_value != &Value::Null {
            return Err(corrupt("an offered event names a holder".to_string()));
        }
        None
    } else {
        if holder_value == &Value::Null {
            return Err(corrupt(format!("a {kind} event names no holder")));
        }
        validate_stored_name(holder_value, "holder id", "a history event")?;
        match holder_value {
            Value::Str(text) => Some(text.clone()),
            _ => unreachable!(),
        }
    };
    let previous_value = object_get(pairs, "previous_holder").unwrap();
    let previous_holder = if kind == "taken_over" {
        if previous_value == &Value::Null {
            return Err(corrupt(
                "a taken over event names no previous holder".to_string(),
            ));
        }
        validate_stored_name(previous_value, "holder id", "a history event")?;
        match previous_value {
            Value::Str(text) => Some(text.clone()),
            _ => unreachable!(),
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
    let at = match at_raw {
        Value::Str(text) => text.clone(),
        _ => unreachable!(),
    };
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

fn validate_document(value: &Value) -> Result<Document> {
    let pairs = match value {
        Value::Object(pairs) if keys_exact(pairs, &DOCUMENT_KEYS) => pairs,
        _ => {
            return Err(corrupt(
                "the document does not carry exactly the recorded fields".to_string(),
            ))
        }
    };
    let format_value = object_get(pairs, "format").unwrap();
    if format_value != &Value::Str(FORMAT_NAME.to_string()) {
        return Err(corrupt(format!(
            "the format name is {}, not {}",
            py_repr_value(format_value),
            py_repr(FORMAT_NAME)
        )));
    }
    let item_values = match object_get(pairs, "items").unwrap() {
        Value::Array(items) => items,
        _ => return Err(corrupt("the items field is not a list".to_string())),
    };
    let event_values = match object_get(pairs, "history").unwrap() {
        Value::Array(events) => events,
        _ => return Err(corrupt("the history field is not a list".to_string())),
    };
    let mut items = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for item_value in item_values {
        let item = validate_item(item_value)?;
        if !seen.insert(item.id.clone()) {
            return Err(corrupt(format!(
                "item {} stands in the backlog twice",
                py_repr(&item.id)
            )));
        }
        items.push(item);
    }
    let mut history = Vec::new();
    for (position, event_value) in event_values.iter().enumerate() {
        history.push(validate_event(event_value, position)?);
    }
    Ok(Document { items, history })
}

/// Escape a string the way Python's json module escapes it with
/// its default ensure_ascii, so a registry the port writes is
/// byte identical to the registry the Python implementation wrote
/// for the same content: the quote, the backslash, and every
/// character outside the printable ASCII range is escaped, with a
/// surrogate pair for a character past the basic plane.
fn escape_json_string(text: &str, out: &mut String) {
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

/// One field of a rendered object, its key and the closure
/// that writes its value.
type FieldRenderer<'a> = (&'a str, &'a dyn Fn(&mut String));

fn render_item(item: &Item, level: usize, out: &mut String) {
    out.push_str("{\n");
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

fn render_event(event: &Event, level: usize, out: &mut String) {
    out.push_str("{\n");
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

/// Render a document exactly as the Python implementation's two
/// space indented json rendering renders it, without the
/// trailing newline the stored file adds.
fn render_document(document: &Document) -> String {
    let mut out = String::new();
    out.push_str("{\n");
    write_indent(&mut out, 1);
    escape_json_string("format", &mut out);
    out.push_str(": ");
    escape_json_string(FORMAT_NAME, &mut out);
    out.push_str(",\n");
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
            render_item(item, 2, &mut out);
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
/// UTF-16 and UTF-32, and refusing what does not decode.
fn decode_document_bytes(bytes: &[u8]) -> std::result::Result<String, String> {
    if bytes.starts_with(&[0xff, 0xfe, 0x00, 0x00]) {
        decode_utf32(&bytes[4..], true)
    } else if bytes.starts_with(&[0x00, 0x00, 0xfe, 0xff]) {
        decode_utf32(&bytes[4..], false)
    } else if bytes.starts_with(&[0xff, 0xfe]) {
        decode_utf16(&bytes[2..], true)
    } else if bytes.starts_with(&[0xfe, 0xff]) {
        decode_utf16(&bytes[2..], false)
    } else if bytes.starts_with(&[0xef, 0xbb, 0xbf]) {
        String::from_utf8(bytes[3..].to_vec())
            .map_err(|err| format!("the document does not decode as UTF-8, {err}"))
    } else {
        String::from_utf8(bytes.to_vec())
            .map_err(|err| format!("the document does not decode as UTF-8, {err}"))
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

/// One backlog in one registry file, opened fresh on every call.
pub struct Registry {
    path: PathBuf,
    lock_path: PathBuf,
}

/// The lock a command holds, with the path it locked. Holding
/// the descriptor is not enough on its own, because the file at
/// the lock path can be deleted and replaced while a command
/// works, and a later command then locks the replacement and
/// works concurrently against the same registry. The guard
/// therefore knows how to check that the file it holds is still
/// the file at the path, by device and inode.
struct LockGuard {
    file: File,
    lock_path: PathBuf,
}

impl LockGuard {
    /// Whether the file this guard holds is the file standing
    /// at the lock path now.
    fn identity_holds(&self) -> bool {
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

    /// Refuse to go on when the held lock is not the file at
    /// the lock path, because anything this command published
    /// could then be overwritten by the command that locked the
    /// standing file, and a success of this command could be
    /// erased without a trace.
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
}

impl Registry {
    pub fn new(path: PathBuf) -> Registry {
        let lock_path = match path.file_name() {
            Some(name) => {
                let mut lock_name = name.to_os_string();
                lock_name.push(LOCK_SUFFIX);
                path.with_file_name(lock_name)
            }
            None => {
                let mut raw = path.clone().into_os_string();
                raw.push(LOCK_SUFFIX);
                PathBuf::from(raw)
            }
        };
        Registry { path, lock_path }
    }

    fn locked(&self, exclusive: bool) -> Result<LockGuard> {
        // The identity of the locked file is checked as soon as
        // the lock stands. A mismatch means the file at the path
        // was deleted and replaced under this command, so the
        // command locks the standing file on a bounded retry
        // instead of working under a file no other command can
        // see, and refuses when the path will not settle.
        for attempt in 1..=5 {
            let file = OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .mode(0o644)
                .open(&self.lock_path)
                .map_err(|err| {
                    Error::Failure(format!(
                        "the lock file beside the registry cannot be opened, {err}"
                    ))
                })?;
            let result = if exclusive {
                file.lock_exclusive()
            } else {
                file.lock_shared()
            };
            result.map_err(|err| {
                Error::Failure(format!(
                    "the lock file beside the registry cannot be locked, {err}"
                ))
            })?;
            let guard = LockGuard {
                file,
                lock_path: self.lock_path.clone(),
            };
            if guard.identity_holds() {
                return Ok(guard);
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

    /// Read and strictly validate the registry, or start empty.
    ///
    /// A missing file reads as the empty document. A file that
    /// stands but cannot be read as recorded raises the corrupt
    /// error and is never reset and never written over by a read.
    fn read_document(&self) -> Result<Document> {
        let raw = match std::fs::read(&self.path) {
            Ok(raw) => raw,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(empty_document()),
            Err(err) => return Err(corrupt(format!("the registry file cannot be read, {err}"))),
        };
        let text = decode_document_bytes(&raw)
            .map_err(|detail| corrupt(format!("the document does not parse, {detail}")))?;
        let value: Value = serde_json::from_str(&text).map_err(|err| {
            let text = err.to_string();
            if let Some(key) = duplicate_key_of(&text) {
                corrupt(format!("a key stands twice in the document, {key}"))
            } else {
                corrupt(format!("the document does not parse, {text}"))
            }
        })?;
        validate_document(&value)
    }

    fn publish(&self, document: &Document) -> Result<()> {
        let text = render_document(document) + "\n";
        let name = self
            .path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let mut random = [0u8; 4];
        getrandom::fill(&mut random)
            .map_err(|err| Error::Failure(format!("the registry could not be published, {err}")))?;
        let hex: String = random.iter().map(|byte| format!("{byte:02x}")).collect();
        let temp = self
            .path
            .with_file_name(format!(".{name}.tmp-{}-{hex}", std::process::id()));
        let mut created = false;
        let staged = (|| -> std::io::Result<()> {
            let mut handle = open_publish_temp(&temp)?;
            created = true;
            handle.write_all(text.as_bytes())?;
            handle.sync_all()?;
            std::fs::rename(&temp, &self.path)?;
            Ok(())
        })();
        if let Err(err) = staged {
            // Only the temporary file this publish created is
            // removed. A failure to create it means whatever
            // stands at the name belongs to someone else.
            if created {
                let _ = std::fs::remove_file(&temp);
            }
            return Err(Error::Failure(format!(
                "the registry could not be published, {err}"
            )));
        }
        if let Some(parent) = self.path.parent() {
            let dir = if parent.as_os_str().is_empty() {
                Path::new(".")
            } else {
                parent
            };
            if let Ok(handle) = File::open(dir) {
                let _ = handle.sync_all();
            }
        }
        Ok(())
    }

    fn mutate<T>(&self, action: impl FnOnce(&mut Document) -> Result<T>) -> Result<T> {
        let parent = self.path.parent().map(|parent| {
            if parent.as_os_str().is_empty() {
                PathBuf::from(".")
            } else {
                parent.to_path_buf()
            }
        });
        if let Some(parent) = parent {
            std::fs::create_dir_all(&parent).map_err(|err| {
                Error::Failure(format!("the registry directory cannot be created, {err}"))
            })?;
        }
        let guard = self.locked(true)?;
        let mut document = self.read_document()?;
        let result = action(&mut document)?;
        // The publish is the one state change, and it happens
        // only while the lock this command holds is still the
        // file at the lock path.
        guard.verify()?;
        self.publish(&document)?;
        Ok(result)
    }

    fn read<T>(&self, action: impl FnOnce(&Document) -> Result<T>) -> Result<T> {
        if !self.path.exists() {
            return action(&empty_document());
        }
        let _guard = self.locked(false)?;
        let document = self.read_document()?;
        action(&document)
    }

    fn append_event(
        document: &mut Document,
        kind: &str,
        item: &str,
        holder: Option<&str>,
        now: &Instant,
        epoch: i128,
        previous: Option<&str>,
    ) {
        document.history.push(Event {
            seq: document.history.len() as i128 + 1,
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

    fn standing_lease<'a>(
        document: &'a Document,
        item_id: &str,
        now: &Instant,
    ) -> Result<&'a Item> {
        let item = match document.items.iter().find(|item| item.id == item_id) {
            Some(item) => item,
            None => {
                return Err(Error::NotFound(format!(
                    "item {} does not stand in the backlog",
                    py_repr(item_id)
                )))
            }
        };
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
        Ok(item)
    }

    /// Add an item to the backlog and record the offered event.
    pub fn offer(&self, item_id: &str, note: &str, now: &Instant) -> Result<()> {
        validate_name_str(item_id, "item")?;
        validate_note_str(note)?;
        validate_now(now)?;
        self.mutate(|document| {
            if document.items.iter().any(|item| item.id == item_id) {
                return Err(Error::Refusal(format!(
                    "item {} already stands in the backlog",
                    py_repr(item_id)
                )));
            }
            document.items.push(Item {
                id: item_id.to_string(),
                note: note.to_string(),
                status: "open".to_string(),
                holder: None,
                token_hash: None,
                epoch: 0,
                expires_at: None,
            });
            Self::append_event(document, "offered", item_id, None, now, 0, None);
            Ok(())
        })
    }

    /// Lease an open or expired item and return the one
    /// ClaimGranted.
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
            let index = match document.items.iter().position(|item| item.id == item_id) {
                Some(index) => index,
                None => {
                    return Err(Error::NotFound(format!(
                        "item {} does not stand in the backlog",
                        py_repr(item_id)
                    )))
                }
            };
            if document.items[index].status == "completed" {
                return Err(Error::Refusal(format!(
                    "item {} is completed and accepts no claim",
                    py_repr(item_id)
                )));
            }
            let mut kind = "claimed";
            let mut previous: Option<String> = None;
            if document.items[index].status == "claimed" {
                if Self::lease_live(&document.items[index], now)? {
                    return Err(Error::Refusal(format!(
                        "item {} is already claimed by {}",
                        py_repr(item_id),
                        py_repr(document.items[index].holder.as_deref().unwrap_or_default())
                    )));
                }
                kind = "taken_over";
                previous = document.items[index].holder.clone();
            }
            // The epoch this claim assigns must be an epoch the
            // reader can take back. A stored epoch at the
            // unsigned top is a document the reader accepts, and
            // assigning the next epoch would write a document no
            // reader accepts, so the claim refuses here, before
            // any state changes.
            if document.items[index].epoch >= u64::MAX as i128 {
                return Err(Error::Refusal(format!(
                    "item {} stands at epoch {}, and no epoch past it can be \
                     assigned, so the claim is refused",
                    py_repr(item_id),
                    document.items[index].epoch
                )));
            }
            let token = draw_token()?;
            let expires = expiry_after(now, ttl)?;
            let item = &mut document.items[index];
            item.status = "claimed".to_string();
            item.holder = Some(holder.to_string());
            item.token_hash = Some(token_hash(&token));
            item.epoch = match item.epoch.checked_add(1) {
                Some(epoch) => epoch,
                None => {
                    return Err(corrupt(format!(
                        "item {} holds an invalid epoch",
                        py_repr(item_id)
                    )))
                }
            };
            item.expires_at = Some(format_instant(&expires));
            let epoch = item.epoch;
            Self::append_event(
                document,
                kind,
                item_id,
                Some(holder),
                now,
                epoch,
                previous.as_deref(),
            );
            Ok(ClaimGranted {
                item: item_id.to_string(),
                holder: holder.to_string(),
                token,
                epoch,
                expires_at: expires,
            })
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
            let item = Self::standing_lease(document, item_id, now)?;
            check_possession(item, holder, token, &Value::Int(epoch))?;
            let standing =
                parse_stored_instant(&Value::Str(item.expires_at.clone().unwrap_or_default()))?;
            let expires = expiry_after(&standing, ttl)?;
            let rendered = format_instant(&expires);
            let standing_epoch = item.epoch;
            let index = document
                .items
                .iter()
                .position(|item| item.id == item_id)
                .unwrap();
            document.items[index].expires_at = Some(rendered);
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
            let item = Self::standing_lease(document, item_id, now)?;
            check_possession(item, holder, token, &Value::Int(epoch))?;
            let standing_epoch = item.epoch;
            let index = document
                .items
                .iter()
                .position(|item| item.id == item_id)
                .unwrap();
            Self::end_lease(&mut document.items[index], "open");
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

    /// Mark the item done under its standing lease. Terminal in
    /// slice 1.
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
            let item = Self::standing_lease(document, item_id, now)?;
            check_possession(item, holder, token, &Value::Int(epoch))?;
            let standing_epoch = item.epoch;
            let index = document
                .items
                .iter()
                .position(|item| item.id == item_id)
                .unwrap();
            Self::end_lease(&mut document.items[index], "completed");
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

    fn board_row(item: &Item, now: &Instant) -> Result<BoardRow> {
        if Self::lease_live(item, now)? {
            Ok(BoardRow {
                id: item.id.clone(),
                note: item.note.clone(),
                status: "claimed".to_string(),
                holder: item.holder.clone(),
                expires_at: item.expires_at.clone(),
                epoch: item.epoch,
            })
        } else {
            Ok(BoardRow {
                id: item.id.clone(),
                note: item.note.clone(),
                status: "open".to_string(),
                holder: None,
                expires_at: None,
                epoch: item.epoch,
            })
        }
    }

    /// Return the open items in offer order, evaluated at the
    /// instant.
    pub fn board(&self, now: &Instant) -> Result<Vec<BoardRow>> {
        validate_now(now)?;
        self.read(|document| {
            let mut rows = Vec::new();
            for item in &document.items {
                if item.status != "completed" {
                    rows.push(Self::board_row(item, now)?);
                }
            }
            Ok(rows)
        })
    }

    /// Return the recorded events in order, or those of one item.
    pub fn history(&self, item_id: Option<&str>) -> Result<Vec<Event>> {
        if let Some(item_id) = item_id {
            validate_name_str(item_id, "item")?;
        }
        self.read(|document| {
            if let Some(item_id) = item_id {
                if !document.items.iter().any(|item| item.id == item_id) {
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
}

/// Check a presented holder and token and epoch against the
/// standing lease of an item, in the order the protocol checks
/// them, holder first and epoch second and token last. The
/// presented epoch arrives as a value so a presentation that is
/// not an integer at all refuses with the same message the
/// integer mismatch raises.
pub fn check_possession(item: &Item, holder: &str, token: &str, epoch: &Value) -> Result<()> {
    if Some(holder) != item.holder.as_deref() {
        return Err(Error::Refusal(format!(
            "item {} is held by {}, not by {}",
            py_repr(&item.id),
            py_repr(item.holder.as_deref().unwrap_or_default()),
            py_repr(holder)
        )));
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
    use std::io::Read;

    /// The publish temporary name carries the process id and a
    /// drawn suffix, so no outside test can know the name a
    /// publish will choose and plant a file at it. The open the
    /// publish uses is pinned here instead, at the helper the
    /// publish itself calls.
    #[test]
    fn publish_temp_open_refuses_a_planted_symlink() {
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
}
