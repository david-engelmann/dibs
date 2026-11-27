//! Command line for dibs, a thin shell over the dibs library.
//!
//! The registry file may be named before or after the subcommand
//! with --registry, or supplied through DIBS_REGISTRY when omitted.
//! The clock the command line uses is the wall clock in UTC, and no
//! flag overrides it, so the expiry a caller sees is the expiry the
//! machine clock gives. Errors the protocol raises are printed as
//! one line on stderr with the exit code the error carries, and no
//! traceback is ever printed for them. The argument parser mirrors
//! the Python argparse surface of the Round 1 command line: the same
//! parser error shapes at exit 2, unambiguous prefix abbreviations
//! of long options, the equals form, and parser extras reported by
//! the top level parser. Arguments are read from the operating
//! system unconverted: path arguments (--registry and --file) are
//! carried as raw paths, and an argument that is not Unicode text in
//! any other position is refused as a parser error, never a panic.

use std::ffi::OsString;
use std::path::PathBuf;
use std::process::ExitCode;

use dibs::{
    escape_json_string, parse_offer_batch_jsonl, validate_max_takeovers, validate_name,
    validate_priority, validate_through_seq, ClaimGranted, CompactSummary, Error, Event, Instant,
    ItemDetail, OfferSpec, Registry, Value, PRIORITY_DEFAULT,
};

const MAIN_USAGE: &str = "usage: dibs [-h] [-V] [-j] [--registry REGISTRY]\n            {offer,offer-batch,claim,claim-next,renew,release,complete,reset,board,ready,show,history,compact,completions} ...";
const COMMANDS: [&str; 14] = [
    "offer",
    "offer-batch",
    "claim",
    "claim-next",
    "renew",
    "release",
    "complete",
    "reset",
    "board",
    "ready",
    "show",
    "history",
    "compact",
    "completions",
];

const MAIN_HELP: &str = "usage: dibs [-h] [-V] [-j] [--registry REGISTRY]
            {offer,offer-batch,claim,claim-next,renew,release,complete,reset,board,ready,show,history,compact,completions} ...

Claim tasks from a shared backlog under leases that expire unless they are
renewed.

positional arguments:
  {offer,offer-batch,claim,claim-next,renew,release,complete,reset,board,ready,show,history,compact,completions}
    offer               add an item to the backlog
    offer-batch         add a batch of items from JSON Lines input
    claim               lease an item under a new claim
    claim-next          lease the first ready item in the backlog
    renew               extend the standing lease from its current expiry
    release             end the standing lease early
    complete            mark the item done under its standing lease
    reset               reset an exhausted item back to open
    board               show the open and claimed items
    ready               show ready items ordered by priority and offer order
    show                show the standing detail of one item
    history             show the recorded claim history
    compact             archive history events through a sequence number
    completions         print shell completion script

options:
  -h, --help            show this help message and exit
  -V, --version         show program version and exit
  -j, --json            emit deterministic JSON on stdout
  --registry REGISTRY   the registry file holding the backlog, or DIBS_REGISTRY
                        when omitted, created by the first mutation";

fn sub_usage(command: &str) -> &'static str {
    match command {
        "offer" => {
            "usage: dibs offer [-h] [-j] [--registry REGISTRY] [--note NOTE]\n                  [--priority PRIORITY] [--tag TAG] [--depends-on ITEM]\n                  [--max-takeovers MAX_TAKEOVERS] item"
        }
        "offer-batch" => "usage: dibs offer-batch [-h] [-j] [--registry REGISTRY] [--file FILE]",
        "claim" => {
            "usage: dibs claim [-h] [-j] [--registry REGISTRY] --holder HOLDER --ttl TTL\n                  item"
        }
        "claim-next" => {
            "usage: dibs claim-next [-h] [-j] [--registry REGISTRY] --holder HOLDER\n                       --ttl TTL [--tag TAG]"
        }
        "renew" => {
            "usage: dibs renew [-h] [-j] [--registry REGISTRY] --holder HOLDER\n                  [--token TOKEN] --epoch EPOCH --ttl TTL item"
        }
        "release" => {
            "usage: dibs release [-h] [-j] [--registry REGISTRY] --holder HOLDER\n                    [--token TOKEN] --epoch EPOCH item"
        }
        "complete" => {
            "usage: dibs complete [-h] [-j] [--registry REGISTRY] --holder HOLDER\n                     [--token TOKEN] --epoch EPOCH item"
        }
        "reset" => "usage: dibs reset [-h] [-j] [--registry REGISTRY] item",
        "board" => "usage: dibs board [-h] [-j] [--registry REGISTRY]",
        "ready" => "usage: dibs ready [-h] [-j] [--registry REGISTRY] [--tag TAG]",
        "show" => "usage: dibs show [-h] [-j] [--registry REGISTRY] item",
        "compact" => {
            "usage: dibs compact [-h] [-j] [--registry REGISTRY] --through-seq THROUGH_SEQ\n                    --archive ARCHIVE"
        }
        "completions" => "usage: dibs completions [-h] {bash,zsh,fish}",
        _ => "usage: dibs history [-h] [-j] [--registry REGISTRY] [item]",
    }
}

fn sub_help(command: &str) -> String {
    let body = match command {
        "offer" => concat!(
            "positional arguments:\n",
            "  item\n\n",
            "options:\n",
            "  -h, --help            show this help message and exit\n",
            "  -j, --json            emit deterministic JSON on stdout\n",
            "  --registry REGISTRY   the registry file holding the backlog\n",
            "  --note NOTE           a short payload note for the item\n",
            "  --priority PRIORITY   task priority from 0 critical to 3 low\n",
            "  --tag TAG             repeatable capability tag for the item\n",
            "  --depends-on ITEM     repeatable prerequisite item id\n",
            "  --max-takeovers MAX_TAKEOVERS\n",
            "                        maximum takeovers before item becomes exhausted",
        ),
        "offer-batch" => concat!(
            "options:\n",
            "  -h, --help            show this help message and exit\n",
            "  -j, --json            emit deterministic JSON on stdout\n",
            "  --registry REGISTRY   the registry file holding the backlog\n",
            "  --file FILE           JSON Lines batch file, or - for standard input",
        ),
        "claim" => concat!(
            "positional arguments:\n",
            "  item\n\n",
            "options:\n",
            "  -h, --help            show this help message and exit\n",
            "  -j, --json            emit deterministic JSON on stdout\n",
            "  --registry REGISTRY   the registry file holding the backlog\n",
            "  --holder HOLDER\n",
            "  --ttl TTL             the lease length in seconds",
        ),
        "claim-next" => concat!(
            "options:\n",
            "  -h, --help            show this help message and exit\n",
            "  -j, --json            emit deterministic JSON on stdout\n",
            "  --registry REGISTRY   the registry file holding the backlog\n",
            "  --holder HOLDER\n",
            "  --ttl TTL             the lease length in seconds\n",
            "  --tag TAG             repeatable required capability tag",
        ),
        "renew" => concat!(
            "positional arguments:\n",
            "  item\n\n",
            "options:\n",
            "  -h, --help            show this help message and exit\n",
            "  -j, --json            emit deterministic JSON on stdout\n",
            "  --registry REGISTRY   the registry file holding the backlog\n",
            "  --holder HOLDER\n",
            "  --token TOKEN         lease bearer token, - for stdin, or DIBS_TOKEN\n",
            "  --epoch EPOCH\n",
            "  --ttl TTL             the added lease length in seconds",
        ),
        "release" | "complete" => concat!(
            "positional arguments:\n",
            "  item\n\n",
            "options:\n",
            "  -h, --help            show this help message and exit\n",
            "  -j, --json            emit deterministic JSON on stdout\n",
            "  --registry REGISTRY   the registry file holding the backlog\n",
            "  --holder HOLDER\n",
            "  --token TOKEN         lease bearer token, - for stdin, or DIBS_TOKEN\n",
            "  --epoch EPOCH",
        ),
        "reset" => concat!(
            "positional arguments:\n",
            "  item\n\n",
            "options:\n",
            "  -h, --help            show this help message and exit\n",
            "  -j, --json            emit deterministic JSON on stdout\n",
            "  --registry REGISTRY   the registry file holding the backlog",
        ),
        "board" => concat!(
            "options:\n",
            "  -h, --help            show this help message and exit\n",
            "  -j, --json            emit deterministic JSON on stdout\n",
            "  --registry REGISTRY   the registry file holding the backlog",
        ),
        "ready" => concat!(
            "options:\n",
            "  -h, --help            show this help message and exit\n",
            "  -j, --json            emit deterministic JSON on stdout\n",
            "  --registry REGISTRY   the registry file holding the backlog\n",
            "  --tag TAG             repeatable required capability tag",
        ),
        "show" => concat!(
            "positional arguments:\n",
            "  item\n\n",
            "options:\n",
            "  -h, --help            show this help message and exit\n",
            "  -j, --json            emit deterministic JSON on stdout\n",
            "  --registry REGISTRY   the registry file holding the backlog",
        ),
        "compact" => concat!(
            "options:\n",
            "  -h, --help            show this help message and exit\n",
            "  -j, --json            emit deterministic JSON on stdout\n",
            "  --registry REGISTRY   the registry file holding the backlog\n",
            "  --through-seq THROUGH_SEQ\n",
            "                        inclusive sequence number to compact through\n",
            "  --archive ARCHIVE     JSON Lines archive file path",
        ),
        "completions" => concat!(
            "positional arguments:\n",
            "  {bash,zsh,fish}\n\n",
            "options:\n",
            "  -h, --help  show this help message and exit",
        ),
        _ => concat!(
            "positional arguments:\n",
            "  item\n\n",
            "options:\n",
            "  -h, --help            show this help message and exit\n",
            "  -j, --json            emit deterministic JSON on stdout\n",
            "  --registry REGISTRY   the registry file holding the backlog",
        ),
    };
    format!("{}\n\n{body}", sub_usage(command))
}

/// The value options of one subcommand in the order the parser
/// defines them, the order its required argument reports and its
/// ambiguity messages follow.
fn value_options(command: &str) -> &'static [&'static str] {
    match command {
        "offer" => &[
            "--note",
            "--priority",
            "--tag",
            "--depends-on",
            "--max-takeovers",
        ],
        "offer-batch" => &["--file"],
        "claim" => &["--holder", "--ttl"],
        "claim-next" => &["--holder", "--ttl", "--tag"],
        "renew" => &["--holder", "--token", "--epoch", "--ttl"],
        "release" | "complete" => &["--holder", "--token", "--epoch"],
        "ready" => &["--tag"],
        "compact" => &["--through-seq", "--archive"],
        _ => &[],
    }
}

fn required_options(command: &str) -> &'static [&'static str] {
    match command {
        "claim" | "claim-next" => &["--holder", "--ttl"],
        "renew" => &["--holder", "--token", "--epoch", "--ttl"],
        "release" | "complete" => &["--holder", "--token", "--epoch"],
        "compact" => &["--through-seq", "--archive"],
        _ => &[],
    }
}

fn positional_names(command: &str) -> &'static [&'static str] {
    match command {
        "offer-batch" | "claim-next" | "board" | "ready" | "compact" => &[],
        "completions" => &["shell"],
        _ => &["item"],
    }
}

/// The positionals a subcommand requires, as opposed to the
/// ones it merely accepts: history takes an optional item.
fn required_positionals(command: &str) -> &'static [&'static str] {
    match command {
        "offer-batch" | "claim-next" | "board" | "ready" | "compact" | "history" => &[],
        "completions" => &["shell"],
        _ => &["item"],
    }
}

fn is_int_option(command: &str, option: &str) -> bool {
    matches!(
        (command, option),
        ("offer", "--priority")
            | ("offer", "--max-takeovers")
            | ("claim", "--ttl")
            | ("claim-next", "--ttl")
            | ("renew", "--epoch")
            | ("renew", "--ttl")
            | ("release", "--epoch")
            | ("complete", "--epoch")
            | ("compact", "--through-seq")
    )
}

struct ParserError {
    usage: &'static str,
    prog: String,
    message: String,
}

fn parser_error(usage: &'static str, prog: &str, message: String) -> ParserError {
    ParserError {
        usage,
        prog: prog.to_string(),
        message,
    }
}

fn is_base64url_bearer_token(token: &str) -> bool {
    token.len() == 43
        && token
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

fn is_option_like(token: &str) -> bool {
    if token.len() < 2 || !token.starts_with('-') || token == "-" {
        return false;
    }
    // A token in the shape of a negative number is positional,
    // as argparse's negative number matcher holds when no
    // option looks like a negative number. The digits of that
    // shape are the decimal digits of the Python grammar, the
    // same set the integer conversion below accepts.
    let body = &token[1..];
    let digits: String = body.chars().filter(|c| *c != '.').collect();
    if !digits.is_empty() && digits.chars().all(|c| py_digit_value(c).is_some()) {
        return false;
    }
    true
}

/// Resolve a long option token against the candidates of one
/// parser, the argparse way: an exact match wins, an
/// unambiguous prefix matches, an ambiguous prefix is an
/// error, and no match means the token is an extra.
fn resolve_long<'a>(name: &str, candidates: &[&'a str]) -> Result<Option<&'a str>, String> {
    if let Some(&exact) = candidates.iter().find(|&&candidate| candidate == name) {
        return Ok(Some(exact));
    }
    let matches: Vec<&'a str> = candidates
        .iter()
        .filter(|candidate| candidate.starts_with(name))
        .copied()
        .collect();
    match matches.len() {
        0 => Ok(None),
        1 => Ok(Some(matches[0])),
        _ => Err(format!(
            "ambiguous option: {name} could match {}",
            matches.join(", ")
        )),
    }
}

enum SubValue {
    Text(String),
    Int(Value),
}

struct SubArgs {
    positionals: Vec<String>,
    options: Vec<(String, SubValue)>,
    registry_path: Option<PathBuf>,
    file_path: Option<OsString>,
    archive_path: Option<PathBuf>,
    json: bool,
    extras: Vec<String>,
}

/// The decimal digit ranges of the Unicode character set,
/// each ten codepoints wide with the digit value standing at
/// the offset inside its range, as probed against the Python
/// runtime the conversion mirrors, Python 3.12.3 carrying
/// Unicode 15.0.0. The Python int conversion accepts a digit
/// from any of these families and no other numeric character.
const DECIMAL_DIGIT_RANGES: [(u32, u32); 68] = [
    (0x0030, 0x0039),
    (0x0660, 0x0669),
    (0x06f0, 0x06f9),
    (0x07c0, 0x07c9),
    (0x0966, 0x096f),
    (0x09e6, 0x09ef),
    (0x0a66, 0x0a6f),
    (0x0ae6, 0x0aef),
    (0x0b66, 0x0b6f),
    (0x0be6, 0x0bef),
    (0x0c66, 0x0c6f),
    (0x0ce6, 0x0cef),
    (0x0d66, 0x0d6f),
    (0x0de6, 0x0def),
    (0x0e50, 0x0e59),
    (0x0ed0, 0x0ed9),
    (0x0f20, 0x0f29),
    (0x1040, 0x1049),
    (0x1090, 0x1099),
    (0x17e0, 0x17e9),
    (0x1810, 0x1819),
    (0x1946, 0x194f),
    (0x19d0, 0x19d9),
    (0x1a80, 0x1a89),
    (0x1a90, 0x1a99),
    (0x1b50, 0x1b59),
    (0x1bb0, 0x1bb9),
    (0x1c40, 0x1c49),
    (0x1c50, 0x1c59),
    (0xa620, 0xa629),
    (0xa8d0, 0xa8d9),
    (0xa900, 0xa909),
    (0xa9d0, 0xa9d9),
    (0xa9f0, 0xa9f9),
    (0xaa50, 0xaa59),
    (0xabf0, 0xabf9),
    (0xff10, 0xff19),
    (0x104a0, 0x104a9),
    (0x10d30, 0x10d39),
    (0x11066, 0x1106f),
    (0x110f0, 0x110f9),
    (0x11136, 0x1113f),
    (0x111d0, 0x111d9),
    (0x112f0, 0x112f9),
    (0x11450, 0x11459),
    (0x114d0, 0x114d9),
    (0x11650, 0x11659),
    (0x116c0, 0x116c9),
    (0x11730, 0x11739),
    (0x118e0, 0x118e9),
    (0x11950, 0x11959),
    (0x11c50, 0x11c59),
    (0x11d50, 0x11d59),
    (0x11da0, 0x11da9),
    (0x11f50, 0x11f59),
    (0x16a60, 0x16a69),
    (0x16ac0, 0x16ac9),
    (0x16b50, 0x16b59),
    (0x1d7ce, 0x1d7d7),
    (0x1d7d8, 0x1d7e1),
    (0x1d7e2, 0x1d7eb),
    (0x1d7ec, 0x1d7f5),
    (0x1d7f6, 0x1d7ff),
    (0x1e140, 0x1e149),
    (0x1e2f0, 0x1e2f9),
    (0x1e4f0, 0x1e4f9),
    (0x1e950, 0x1e959),
    (0x1fbf0, 0x1fbf9),
];

/// The value of one character under the Python int conversion,
/// a decimal digit in any family of the table above, or None
/// for any other character.
fn py_digit_value(c: char) -> Option<u32> {
    let n = c as u32;
    DECIMAL_DIGIT_RANGES
        .iter()
        .find(|&&(low, high)| n >= low && n <= high)
        .map(|&(low, _)| n - low)
}

/// Python's int conversion for the numeric arguments: optional
/// surrounding whitespace from the Unicode whitespace set, an
/// optional sign, decimal digits from any family of the table
/// above with single underscores between them. A digit string
/// past the conversion's own limit of 4300 digits is refused
/// here, as the conversion refuses it, so the argument draws
/// the parser's invalid int value error rather than a
/// validation over a number the conversion never produced.
/// The value is kept whole, at 128 bit width when it fits and
/// as its canonical digit text when it does not, so a later
/// validation prints the number the caller typed rather than a
/// parser failure.
fn python_int(text: &str) -> Option<Value> {
    let trimmed = text.trim_matches(|c: char| c.is_whitespace());
    let (negative, body) = if let Some(rest) = trimmed.strip_prefix('-') {
        (true, rest)
    } else if let Some(rest) = trimmed.strip_prefix('+') {
        (false, rest)
    } else {
        (false, trimmed)
    };
    if body.is_empty() {
        return None;
    }
    let mut digits = String::new();
    let mut previous_digit = false;
    let mut previous_underscore = false;
    let mut seen_digit = false;
    for ch in body.chars() {
        if ch == '_' {
            if !previous_digit || previous_underscore {
                return None;
            }
            previous_underscore = true;
            previous_digit = false;
        } else {
            let value = py_digit_value(ch)?;
            digits.push(char::from_digit(value, 10).unwrap());
            previous_digit = true;
            previous_underscore = false;
            seen_digit = true;
        }
    }
    if previous_underscore || !seen_digit {
        return None;
    }
    if digits.len() > 4300 {
        return None;
    }
    let canonical_digits = {
        let stripped = digits.trim_start_matches('0');
        if stripped.is_empty() {
            "0".to_string()
        } else {
            stripped.to_string()
        }
    };
    let signed = if negative && canonical_digits != "0" {
        format!("-{canonical_digits}")
    } else {
        canonical_digits.clone()
    };
    match signed.parse::<i128>() {
        Ok(number) => Some(Value::Int(number)),
        Err(_) => Some(Value::BigInt(signed)),
    }
}

/// Extract the raw OsString value of a path option in `--opt=value`
/// form on Unix before Unicode conversion so non UTF-8 path bytes
/// survive intact both before and after `<subcommand>`.
#[cfg(unix)]
fn path_equals_value<'a>(
    token: &OsString,
    candidates: &[&'a str],
    path_options: &[&'a str],
) -> Option<(&'a str, OsString)> {
    use std::os::unix::ffi::{OsStrExt, OsStringExt};
    let bytes = token.as_bytes();
    if !bytes.starts_with(b"--") {
        return None;
    }
    let pos = bytes.iter().position(|&b| b == b'=')?;
    if pos <= 2 {
        return None;
    }
    let name = std::str::from_utf8(&bytes[..pos]).ok()?;
    let matched = resolve_long(name, candidates).ok()??;
    if path_options.contains(&matched) {
        Some((matched, OsString::from_vec(bytes[pos + 1..].to_vec())))
    } else {
        None
    }
}

/// Parse one subcommand's raw OsString arguments. Path options
/// (--registry and --file) keep raw operating system bytes; every
/// other token must be valid UTF-8 text. Option values are drawn
/// from the following token unless it is option like, numeric
/// options are converted as they are consumed, and tokens the
/// subcommand does not consume are collected as extras for the
/// top level parser to report.
fn parse_sub(command: &str, tokens: &[OsString]) -> Result<SubArgs, ParserError> {
    let mut candidates: Vec<&str> = if command == "completions" {
        vec!["--help"]
    } else {
        vec!["--help", "--json", "--registry"]
    };
    candidates.extend_from_slice(value_options(command));
    #[cfg(unix)]
    let path_options: &[&str] = match command {
        "completions" => &[],
        "offer-batch" => &["--registry", "--file"],
        "compact" => &["--registry", "--archive"],
        _ => &["--registry"],
    };
    let positional_count = positional_names(command).len();
    let mut result = SubArgs {
        positionals: Vec::new(),
        options: Vec::new(),
        registry_path: None,
        file_path: None,
        archive_path: None,
        json: false,
        extras: Vec::new(),
    };
    let mut index = 0;
    let mut after_separator = false;
    let mut separator_pending = false;
    while index < tokens.len() {
        #[cfg(unix)]
        if !after_separator {
            if let Some((opt, raw_val)) =
                path_equals_value(&tokens[index], &candidates, path_options)
            {
                if opt == "--registry" {
                    result.registry_path = Some(PathBuf::from(raw_val));
                } else if opt == "--archive" {
                    result.archive_path = Some(PathBuf::from(raw_val));
                } else {
                    result.file_path = Some(raw_val);
                }
                index += 1;
                continue;
            }
        }
        let Some(token) = tokens[index].to_str() else {
            return Err(parser_error(
                sub_usage(command),
                &format!("dibs {command}"),
                "an argument is not valid Unicode text".to_string(),
            ));
        };
        if !after_separator && token == "--" {
            after_separator = true;
            separator_pending = true;
            index += 1;
            continue;
        }
        if !after_separator && (token == "-h" || token == "--help") {
            out_line(format_args!("{}", sub_help(command)));
            std::process::exit(0);
        }
        if !after_separator && command != "completions" && token == "-j" {
            result.json = true;
            index += 1;
            continue;
        }
        if !after_separator && token.starts_with("--") && token.len() > 2 {
            let (name, inline_value) = match token.find('=') {
                Some(pos) => (&token[..pos], Some(token[pos + 1..].to_string())),
                None => (token, None),
            };
            match resolve_long(name, &candidates) {
                Ok(Some("--help")) => {
                    out_line(format_args!("{}", sub_help(command)));
                    std::process::exit(0);
                }
                Ok(Some("--json")) => {
                    if let Some(value) = inline_value {
                        return Err(parser_error(
                            sub_usage(command),
                            &format!("dibs {command}"),
                            format!(
                                "argument -j/--json: ignored explicit argument {}",
                                dibs::py_repr(&value)
                            ),
                        ));
                    }
                    result.json = true;
                    index += 1;
                    continue;
                }
                Ok(Some("--registry")) => {
                    if let Some(value) = inline_value {
                        result.registry_path = Some(PathBuf::from(value));
                        index += 1;
                    } else if index + 1 < tokens.len()
                        && !tokens[index + 1].to_str().is_some_and(is_option_like)
                    {
                        result.registry_path = Some(PathBuf::from(&tokens[index + 1]));
                        index += 2;
                    } else {
                        return Err(parser_error(
                            sub_usage(command),
                            &format!("dibs {command}"),
                            "argument --registry: expected one argument".to_string(),
                        ));
                    }
                    continue;
                }
                Ok(Some("--file")) => {
                    if let Some(value) = inline_value {
                        result.file_path = Some(OsString::from(value));
                        index += 1;
                    } else if index + 1 < tokens.len()
                        && !tokens[index + 1].to_str().is_some_and(is_option_like)
                    {
                        result.file_path = Some(tokens[index + 1].clone());
                        index += 2;
                    } else {
                        return Err(parser_error(
                            sub_usage(command),
                            &format!("dibs {command}"),
                            "argument --file: expected one argument".to_string(),
                        ));
                    }
                    continue;
                }
                Ok(Some("--archive")) => {
                    if let Some(value) = inline_value {
                        result.archive_path = Some(PathBuf::from(value));
                        index += 1;
                    } else if index + 1 < tokens.len()
                        && !tokens[index + 1].to_str().is_some_and(is_option_like)
                    {
                        result.archive_path = Some(PathBuf::from(&tokens[index + 1]));
                        index += 2;
                    } else {
                        return Err(parser_error(
                            sub_usage(command),
                            &format!("dibs {command}"),
                            "argument --archive: expected one argument".to_string(),
                        ));
                    }
                    continue;
                }
                Ok(Some(option)) => {
                    let raw = if let Some(value) = inline_value {
                        index += 1;
                        value
                    } else if index + 1 < tokens.len() {
                        let Some(next_str) = tokens[index + 1].to_str() else {
                            return Err(parser_error(
                                sub_usage(command),
                                &format!("dibs {command}"),
                                "an argument is not valid Unicode text".to_string(),
                            ));
                        };
                        if !(option == "--token" && is_base64url_bearer_token(next_str))
                            && is_option_like(next_str)
                        {
                            return Err(parser_error(
                                sub_usage(command),
                                &format!("dibs {command}"),
                                format!("argument {option}: expected one argument"),
                            ));
                        }
                        index += 2;
                        next_str.to_string()
                    } else {
                        return Err(parser_error(
                            sub_usage(command),
                            &format!("dibs {command}"),
                            format!("argument {option}: expected one argument"),
                        ));
                    };
                    if is_int_option(command, option) {
                        match python_int(&raw) {
                            Some(value) => {
                                result
                                    .options
                                    .push((option.to_string(), SubValue::Int(value)));
                            }
                            None => {
                                return Err(parser_error(
                                    sub_usage(command),
                                    &format!("dibs {command}"),
                                    format!(
                                        "argument {option}: invalid int value: {}",
                                        dibs::py_repr(&raw)
                                    ),
                                ));
                            }
                        }
                    } else {
                        result
                            .options
                            .push((option.to_string(), SubValue::Text(raw)));
                    }
                    continue;
                }
                Ok(None) => {
                    if separator_pending {
                        result.extras.push("--".to_string());
                        separator_pending = false;
                    }
                    result.extras.push(token.to_string());
                    index += 1;
                    continue;
                }
                Err(message) => {
                    return Err(parser_error(
                        sub_usage(command),
                        &format!("dibs {command}"),
                        message,
                    ));
                }
            }
        }
        if !after_separator && is_option_like(token) {
            if separator_pending {
                result.extras.push("--".to_string());
                separator_pending = false;
            }
            result.extras.push(token.to_string());
            index += 1;
            continue;
        }
        if result.positionals.len() < positional_count {
            result.positionals.push(token.to_string());
        } else {
            if separator_pending {
                result.extras.push("--".to_string());
                separator_pending = false;
            }
            result.extras.push(token.to_string());
        }
        index += 1;
    }
    Ok(result)
}

fn option_text<'a>(args: &'a SubArgs, name: &str) -> Option<&'a str> {
    args.options.iter().rev().find_map(|(seen, value)| {
        if seen == name {
            match value {
                SubValue::Text(text) => Some(text.as_str()),
                SubValue::Int(_) => None,
            }
        } else {
            None
        }
    })
}

fn option_texts(args: &SubArgs, name: &str) -> Vec<String> {
    args.options
        .iter()
        .filter_map(|(seen, value)| {
            if seen == name {
                match value {
                    SubValue::Text(text) => Some(text.clone()),
                    SubValue::Int(_) => None,
                }
            } else {
                None
            }
        })
        .collect()
}

fn option_int<'a>(args: &'a SubArgs, name: &str) -> Option<&'a Value> {
    args.options.iter().rev().find_map(|(seen, value)| {
        if seen == name {
            match value {
                SubValue::Int(value) => Some(value),
                SubValue::Text(_) => None,
            }
        } else {
            None
        }
    })
}

fn require(command: &str, args: &SubArgs) -> Result<(), ParserError> {
    let mut missing: Vec<String> = Vec::new();
    for (index, name) in required_positionals(command).iter().enumerate() {
        if args.positionals.len() <= index {
            missing.push(name.to_string());
        }
    }
    for name in required_options(command) {
        let present = if is_int_option(command, name) {
            option_int(args, name).is_some()
        } else if *name == "--token" {
            option_text(args, name).is_some() || std::env::var_os("DIBS_TOKEN").is_some()
        } else if *name == "--archive" {
            args.archive_path.is_some()
        } else {
            option_text(args, name).is_some()
        };
        if !present {
            missing.push(name.to_string());
        }
    }
    if missing.is_empty() {
        Ok(())
    } else {
        Err(parser_error(
            sub_usage(command),
            &format!("dibs {command}"),
            format!(
                "the following arguments are required: {}",
                missing.join(", ")
            ),
        ))
    }
}

/// Write one line of command output through a locked stdout
/// handle, and end the process quietly at exit 1 when the
/// write fails (DB-A8).
fn out_line(args: std::fmt::Arguments) {
    use std::io::Write;
    let stdout = std::io::stdout();
    let mut lock = stdout.lock();
    if writeln!(lock, "{args}").is_err() {
        std::process::exit(1);
    }
}

/// Write raw text without appending an extra newline through a
/// locked stdout handle, exiting quietly at exit 1 on failure.
fn out_raw(text: &str) {
    use std::io::Write;
    let stdout = std::io::stdout();
    let mut lock = stdout.lock();
    if lock.write_all(text.as_bytes()).is_err() {
        std::process::exit(1);
    }
}

/// Write one line of error output through a locked stderr
/// handle, with the same failure shape as out_line.
fn err_line(args: std::fmt::Arguments) {
    use std::io::Write;
    let stderr = std::io::stderr();
    let mut lock = stderr.lock();
    if writeln!(lock, "{args}").is_err() {
        std::process::exit(1);
    }
}

fn parser_fail(usage: &str, prog: &str, message: &str) -> ExitCode {
    err_line(format_args!("{usage}"));
    err_line(format_args!("{prog}: error: {message}"));
    ExitCode::from(2)
}

fn report_error(error: &Error) -> ExitCode {
    err_line(format_args!("error: {}", error.message()));
    ExitCode::from(error.exit_code() as u8)
}

fn ttl_argument(item: &str, holder: &str, value: &Value) -> Result<i64, Error> {
    match value {
        Value::Int(number) if *number >= i64::MIN as i128 && *number <= i64::MAX as i128 => {
            Ok(*number as i64)
        }
        _ => {
            dibs::validate_name(&Value::Str(item.to_string()), "item")?;
            dibs::validate_name(&Value::Str(holder.to_string()), "holder")?;
            dibs::validate_ttl(value)
        }
    }
}

fn ttl_argument_holder(holder: &str, value: &Value) -> Result<i64, Error> {
    match value {
        Value::Int(number) if *number >= i64::MIN as i128 && *number <= i64::MAX as i128 => {
            Ok(*number as i64)
        }
        _ => {
            dibs::validate_name(&Value::Str(holder.to_string()), "holder")?;
            dibs::validate_ttl(value)
        }
    }
}

fn epoch_argument(value: &Value) -> i128 {
    match value {
        Value::Int(number) => *number,
        _ => i128::MAX,
    }
}

fn stdin_read_error(err: std::io::Error) -> Error {
    Error::Failure(format!("standard input cannot be read, {err}"))
}

fn read_stdin_token() -> Result<String, Error> {
    use std::io::Read;
    let stdin = std::io::stdin();
    let mut buf = Vec::with_capacity(257);
    stdin
        .lock()
        .take(257)
        .read_to_end(&mut buf)
        .map_err(stdin_read_error)?;
    if buf.len() > 256 {
        return Err(Error::InvalidRequest(
            "stdin token exceeds 256 bytes".to_string(),
        ));
    }
    let text = std::str::from_utf8(&buf)
        .map_err(|_| Error::InvalidRequest("stdin token is not valid UTF-8".to_string()))?;
    let trimmed = text.trim_end_matches(['\r', '\n']);
    if trimmed.is_empty() {
        return Err(Error::InvalidRequest("stdin token is empty".to_string()));
    }
    Ok(trimmed.to_string())
}

fn resolve_token(args: &SubArgs) -> Result<String, Error> {
    if let Some(cli_token) = option_text(args, "--token") {
        if cli_token == "-" {
            return read_stdin_token();
        }
        return Ok(cli_token.to_string());
    }
    let Some(raw) = std::env::var_os("DIBS_TOKEN") else {
        return Err(Error::InvalidRequest(
            "missing token, pass --token or set DIBS_TOKEN".to_string(),
        ));
    };
    let text = raw
        .into_string()
        .map_err(|_| Error::InvalidRequest("DIBS_TOKEN is not valid UTF-8".to_string()))?;
    if text.is_empty() {
        return Err(Error::InvalidRequest("DIBS_TOKEN is empty".to_string()));
    }
    Ok(text)
}

fn read_offer_batch_input(file_arg: Option<&OsString>) -> Result<String, Error> {
    let bytes = match file_arg {
        Some(path) if path.to_str() != Some("-") => std::fs::read(PathBuf::from(path))
            .map_err(|err| Error::Failure(format!("the offer batch file cannot be read, {err}")))?,
        _ => {
            use std::io::Read;
            let mut buf = Vec::new();
            std::io::stdin()
                .lock()
                .read_to_end(&mut buf)
                .map_err(stdin_read_error)?;
            buf
        }
    };
    String::from_utf8(bytes).map_err(|err| {
        Error::InvalidRequest(format!("offer batch input is not valid UTF-8, {err}"))
    })
}

fn push_opt_json_str(value: &Option<String>, out: &mut String) {
    match value {
        Some(text) => escape_json_string(text, out),
        None => out.push_str("null"),
    }
}

fn push_json_str_list(items: &[String], out: &mut String) {
    out.push('[');
    for (idx, item) in items.iter().enumerate() {
        if idx > 0 {
            out.push_str(", ");
        }
        escape_json_string(item, out);
    }
    out.push(']');
}

fn render_claim_json(granted: &ClaimGranted) -> String {
    let mut out = String::from("{\"item\": ");
    escape_json_string(&granted.item, &mut out);
    out.push_str(", \"holder\": ");
    escape_json_string(&granted.holder, &mut out);
    out.push_str(", \"token\": ");
    escape_json_string(&granted.token, &mut out);
    out.push_str(&format!(", \"epoch\": {}, \"expires_at\": ", granted.epoch));
    escape_json_string(&dibs::format_instant(&granted.expires_at), &mut out);
    out.push('}');
    out
}

fn render_item_detail_json(detail: &ItemDetail, out: &mut String) {
    out.push_str("{\"id\": ");
    escape_json_string(&detail.id, out);
    out.push_str(", \"note\": ");
    escape_json_string(&detail.note, out);
    out.push_str(", \"status\": ");
    escape_json_string(&detail.status, out);
    out.push_str(", \"effective_status\": ");
    escape_json_string(&detail.effective_status, out);
    out.push_str(&format!(", \"priority\": {}, \"tags\": ", detail.priority));
    push_json_str_list(&detail.tags, out);
    out.push_str(", \"depends_on\": ");
    push_json_str_list(&detail.depends_on, out);
    out.push_str(", \"holder\": ");
    push_opt_json_str(&detail.holder, out);
    out.push_str(", \"expires_at\": ");
    push_opt_json_str(&detail.expires_at, out);
    out.push_str(&format!(
        ", \"epoch\": {}, \"takeover_count\": {}, \"max_takeovers\": ",
        detail.epoch, detail.takeover_count
    ));
    match detail.max_takeovers {
        Some(cap) => out.push_str(&cap.to_string()),
        None => out.push_str("null"),
    }
    out.push('}');
}

fn render_event_json(event: &Event, out: &mut String) {
    out.push_str(&format!("{{\"seq\": {}, \"at\": ", event.seq));
    escape_json_string(&event.at, out);
    out.push_str(", \"kind\": ");
    escape_json_string(&event.kind, out);
    out.push_str(", \"item\": ");
    escape_json_string(&event.item, out);
    out.push_str(", \"holder\": ");
    push_opt_json_str(&event.holder, out);
    out.push_str(&format!(
        ", \"epoch\": {}, \"expires_at\": null, \"note\": null, \"previous_holder\": ",
        event.epoch
    ));
    push_opt_json_str(&event.previous_holder, out);
    out.push('}');
}

fn render_compact_json(summary: &CompactSummary, archive_utf8: &str) -> String {
    let mut out = format!(
        "{{\"through_seq\": {}, \"archived_count\": {}, \"remaining_count\": {}, \"archived_events_sha256\": ",
        summary.through_seq, summary.archived_count, summary.remaining_count
    );
    escape_json_string(&summary.archived_events_sha256, &mut out);
    out.push_str(", \"archive_path\": ");
    escape_json_string(archive_utf8, &mut out);
    out.push('}');
    out
}

fn escape_note_line(note: &str) -> String {
    let mut out = String::with_capacity(note.len());
    for ch in note.chars() {
        match ch {
            '\r' => out.push_str("\\r"),
            '\n' => out.push_str("\\n"),
            c => out.push(c),
        }
    }
    out
}

fn print_item_detail_human(detail: &ItemDetail) {
    let tags = if detail.tags.is_empty() {
        "-".to_string()
    } else {
        detail.tags.join(",")
    };
    let deps = if detail.depends_on.is_empty() {
        "-".to_string()
    } else {
        detail.depends_on.join(",")
    };
    let max_takeovers = detail
        .max_takeovers
        .map(|cap| cap.to_string())
        .unwrap_or_else(|| "-".to_string());
    out_line(format_args!("id {}", detail.id));
    out_line(format_args!("note {}", escape_note_line(&detail.note)));
    out_line(format_args!("status {}", detail.status));
    out_line(format_args!("effective_status {}", detail.effective_status));
    out_line(format_args!("priority {}", detail.priority));
    out_line(format_args!("tags {tags}"));
    out_line(format_args!("depends_on {deps}"));
    out_line(format_args!(
        "holder {}",
        detail.holder.as_deref().unwrap_or("-")
    ));
    out_line(format_args!(
        "expires_at {}",
        detail.expires_at.as_deref().unwrap_or("-")
    ));
    out_line(format_args!("epoch {}", detail.epoch));
    out_line(format_args!("takeover_count {}", detail.takeover_count));
    out_line(format_args!("max_takeovers {max_takeovers}"));
}

fn completion_script(shell: &str) -> &'static str {
    match shell {
        "bash" => concat!(
            "_dibs_completions() {\n",
            "    local cur prev words cword subcmd=\"\"\n",
            "    cur=\"${COMP_WORDS[COMP_CWORD]}\"\n",
            "    prev=\"${COMP_WORDS[COMP_CWORD-1]}\"\n",
            "    local subcommands=\"offer offer-batch claim claim-next renew release complete reset board ready show history compact completions\"\n",
            "    local global_opts=\"-h --help -V --version -j --json --registry\"\n",
            "    local i\n",
            "    for ((i=1; i<COMP_CWORD; i++)); do\n",
            "        case \"${COMP_WORDS[i]}\" in\n",
            "            --registry|--file|--archive|--note|--priority|--tag|--depends-on|--max-takeovers|--holder|--ttl|--token|--epoch|--through-seq)\n",
            "                ((i++))\n",
            "                ;;\n",
            "            offer|offer-batch|claim|claim-next|renew|release|complete|reset|board|ready|show|history|compact|completions)\n",
            "                subcmd=\"${COMP_WORDS[i]}\"\n",
            "                break\n",
            "                ;;\n",
            "        esac\n",
            "    done\n",
            "    if [[ \"$prev\" == \"--registry\" || \"$prev\" == \"--file\" || \"$prev\" == \"--archive\" ]]; then\n",
            "        local IFS=$'\\n'\n",
            "        COMPREPLY=($(compgen -f -- \"$cur\"))\n",
            "        return 0\n",
            "    fi\n",
            "    if [[ -z \"$subcmd\" ]]; then\n",
            "        if [[ \"$cur\" == -* ]]; then\n",
            "            COMPREPLY=($(compgen -W \"$global_opts\" -- \"$cur\"))\n",
            "        else\n",
            "            COMPREPLY=($(compgen -W \"$subcommands\" -- \"$cur\"))\n",
            "        fi\n",
            "        return 0\n",
            "    fi\n",
            "    case \"$subcmd\" in\n",
            "        offer)\n",
            "            COMPREPLY=($(compgen -W \"-h --help -j --json --registry --note --priority --tag --depends-on --max-takeovers\" -- \"$cur\")) ;;\n",
            "        offer-batch)\n",
            "            COMPREPLY=($(compgen -W \"-h --help -j --json --registry --file\" -- \"$cur\")) ;;\n",
            "        claim)\n",
            "            COMPREPLY=($(compgen -W \"-h --help -j --json --registry --holder --ttl\" -- \"$cur\")) ;;\n",
            "        claim-next)\n",
            "            COMPREPLY=($(compgen -W \"-h --help -j --json --registry --holder --ttl --tag\" -- \"$cur\")) ;;\n",
            "        renew)\n",
            "            COMPREPLY=($(compgen -W \"-h --help -j --json --registry --holder --token --epoch --ttl\" -- \"$cur\")) ;;\n",
            "        release|complete)\n",
            "            COMPREPLY=($(compgen -W \"-h --help -j --json --registry --holder --token --epoch\" -- \"$cur\")) ;;\n",
            "        ready)\n",
            "            COMPREPLY=($(compgen -W \"-h --help -j --json --registry --tag\" -- \"$cur\")) ;;\n",
            "        reset|board|show|history)\n",
            "            COMPREPLY=($(compgen -W \"-h --help -j --json --registry\" -- \"$cur\")) ;;\n",
            "        compact)\n",
            "            COMPREPLY=($(compgen -W \"-h --help -j --json --registry --through-seq --archive\" -- \"$cur\")) ;;\n",
            "        completions)\n",
            "            COMPREPLY=($(compgen -W \"-h --help bash zsh fish\" -- \"$cur\")) ;;\n",
            "    esac\n",
            "}\n",
            "complete -F _dibs_completions dibs\n",
        ),
        "zsh" => concat!(
            "#compdef dibs\n",
            "_dibs() {\n",
            "    local -a subcommands\n",
            "    subcommands=(\n",
            "        'offer:add an item to the backlog'\n",
            "        'offer-batch:add a batch of items from JSON Lines input'\n",
            "        'claim:lease an item under a new claim'\n",
            "        'claim-next:lease the first ready item in the backlog'\n",
            "        'renew:extend the standing lease from its current expiry'\n",
            "        'release:end the standing lease early'\n",
            "        'complete:mark the item done under its standing lease'\n",
            "        'reset:reset an exhausted item back to open'\n",
            "        'board:show the open and claimed items'\n",
            "        'ready:show ready items ordered by priority and offer order'\n",
            "        'show:show the standing detail of one item'\n",
            "        'history:show the recorded claim history'\n",
            "        'compact:archive history events through a sequence number'\n",
            "        'completions:print shell completion script'\n",
            "    )\n",
            "    _arguments -C \\\n",
            "        '(-h --help)'{-h,--help}'[show help message and exit]' \\\n",
            "        '(-V --version)'{-V,--version}'[show program version and exit]' \\\n",
            "        '(-j --json)'{-j,--json}'[emit deterministic JSON on stdout]' \\\n",
            "        '--registry=[the registry file holding the backlog]:registry:_files' \\\n",
            "        '1:command:->cmd' \\\n",
            "        '*::arg:->args'\n",
            "    case \"$state\" in\n",
            "        cmd)\n",
            "            _describe -t commands 'dibs command' subcommands\n",
            "            ;;\n",
            "        args)\n",
            "            case \"$words[1]\" in\n",
            "                offer) _arguments '(-h --help)'{-h,--help} '(-j --json)'{-j,--json} '--registry=:registry:_files' '--note=:note:' '--priority=:priority:(0 1 2 3)' '*--tag=:tag:' '*--depends-on=:item:' '--max-takeovers=:max_takeovers:' '1:item:' ;;\n",
            "                offer-batch) _arguments '(-h --help)'{-h,--help} '(-j --json)'{-j,--json} '--registry=:registry:_files' '--file=:file:_files' ;;\n",
            "                claim) _arguments '(-h --help)'{-h,--help} '(-j --json)'{-j,--json} '--registry=:registry:_files' '--holder=:holder:' '--ttl=:ttl:' '1:item:' ;;\n",
            "                claim-next) _arguments '(-h --help)'{-h,--help} '(-j --json)'{-j,--json} '--registry=:registry:_files' '--holder=:holder:' '--ttl=:ttl:' '*--tag=:tag:' ;;\n",
            "                renew) _arguments '(-h --help)'{-h,--help} '(-j --json)'{-j,--json} '--registry=:registry:_files' '--holder=:holder:' '--token=:token:' '--epoch=:epoch:' '--ttl=:ttl:' '1:item:' ;;\n",
            "                release|complete) _arguments '(-h --help)'{-h,--help} '(-j --json)'{-j,--json} '--registry=:registry:_files' '--holder=:holder:' '--token=:token:' '--epoch=:epoch:' '1:item:' ;;\n",
            "                board) _arguments '(-h --help)'{-h,--help} '(-j --json)'{-j,--json} '--registry=:registry:_files' ;;\n",
            "                ready) _arguments '(-h --help)'{-h,--help} '(-j --json)'{-j,--json} '--registry=:registry:_files' '*--tag=:tag:' ;;\n",
            "                reset|show|history) _arguments '(-h --help)'{-h,--help} '(-j --json)'{-j,--json} '--registry=:registry:_files' '1:item:' ;;\n",
            "                compact) _arguments '(-h --help)'{-h,--help} '(-j --json)'{-j,--json} '--registry=:registry:_files' '--through-seq=:through_seq:' '--archive=:archive:_files' ;;\n",
            "                completions) _arguments '(-h --help)'{-h,--help} '1:shell:(bash zsh fish)' ;;\n",
            "            esac\n",
            "            ;;\n",
            "    esac\n",
            "}\n",
            "_dibs \"$@\"\n",
        ),
        _ => concat!(
            "complete -c dibs -f\n",
            "complete -c dibs -s h -l help -d 'show --help message and exit'\n",
            "complete -c dibs -n 'not __fish_seen_subcommand_from completions' -s j -l json -d 'emit deterministic --json on stdout'\n",
            "complete -c dibs -n '__fish_use_subcommand' -s V -l version -d 'show program --version and exit'\n",
            "complete -c dibs -n 'not __fish_seen_subcommand_from completions' -l registry -r -F -d 'the --registry file holding the backlog'\n",
            "complete -c dibs -n '__fish_use_subcommand' -a offer -d 'add an item to the backlog'\n",
            "complete -c dibs -n '__fish_use_subcommand' -a offer-batch -d 'add a batch of items from JSON Lines input'\n",
            "complete -c dibs -n '__fish_use_subcommand' -a claim -d 'lease an item under a new claim'\n",
            "complete -c dibs -n '__fish_use_subcommand' -a claim-next -d 'lease the first ready item in the backlog'\n",
            "complete -c dibs -n '__fish_use_subcommand' -a renew -d 'extend the standing lease from its current expiry'\n",
            "complete -c dibs -n '__fish_use_subcommand' -a release -d 'end the standing lease early'\n",
            "complete -c dibs -n '__fish_use_subcommand' -a complete -d 'mark the item done under its standing lease'\n",
            "complete -c dibs -n '__fish_use_subcommand' -a reset -d 'reset an exhausted item back to open'\n",
            "complete -c dibs -n '__fish_use_subcommand' -a board -d 'show the open and claimed items'\n",
            "complete -c dibs -n '__fish_use_subcommand' -a ready -d 'show ready items ordered by priority and offer order'\n",
            "complete -c dibs -n '__fish_use_subcommand' -a show -d 'show the standing detail of one item'\n",
            "complete -c dibs -n '__fish_use_subcommand' -a history -d 'show the recorded claim history'\n",
            "complete -c dibs -n '__fish_use_subcommand' -a compact -d 'archive history events through a sequence number'\n",
            "complete -c dibs -n '__fish_use_subcommand' -a completions -d 'print shell completion script'\n",
            "complete -c dibs -n '__fish_seen_subcommand_from offer' -l note -r -d 'a short payload --note for the item'\n",
            "complete -c dibs -n '__fish_seen_subcommand_from offer' -l priority -r -d 'task --priority from 0 to 3'\n",
            "complete -c dibs -n '__fish_seen_subcommand_from offer claim-next ready' -l tag -r -d 'capability --tag'\n",
            "complete -c dibs -n '__fish_seen_subcommand_from offer' -l depends-on -r -d 'prerequisite item id for --depends-on'\n",
            "complete -c dibs -n '__fish_seen_subcommand_from offer' -l max-takeovers -r -d 'maximum takeovers for --max-takeovers'\n",
            "complete -c dibs -n '__fish_seen_subcommand_from offer-batch' -l file -r -F -d 'JSON Lines batch --file, or - for standard input'\n",
            "complete -c dibs -n '__fish_seen_subcommand_from claim claim-next renew release complete' -l holder -r -d 'lease --holder id'\n",
            "complete -c dibs -n '__fish_seen_subcommand_from claim claim-next renew' -l ttl -r -d 'lease --ttl length in seconds'\n",
            "complete -c dibs -n '__fish_seen_subcommand_from renew release complete' -l token -r -d 'lease bearer --token'\n",
            "complete -c dibs -n '__fish_seen_subcommand_from renew release complete' -l epoch -r -d 'standing fencing --epoch'\n",
            "complete -c dibs -n '__fish_seen_subcommand_from compact' -l through-seq -r -d 'sequence bound for --through-seq'\n",
            "complete -c dibs -n '__fish_seen_subcommand_from compact' -l archive -r -F -d 'JSON Lines --archive file'\n",
            "complete -c dibs -n '__fish_seen_subcommand_from completions' -a 'bash zsh fish'\n",
        ),
    }
}

fn has_json_flag_in_tail(tail: &[OsString]) -> bool {
    tail.iter().any(|arg| {
        arg.to_str().is_some_and(|text| {
            text == "-j"
                || resolve_long(
                    text.split('=').next().unwrap_or(text),
                    &["--help", "--version", "--json", "--registry"],
                )
                .ok()
                .flatten()
                    == Some("--json")
        })
    })
}

fn run() -> ExitCode {
    let argv: Vec<OsString> = std::env::args_os().skip(1).collect();
    match dispatch(&argv) {
        Ok(code) => code,
        Err(error) => report_error(&error),
    }
}

fn dispatch(argv: &[OsString]) -> Result<ExitCode, Error> {
    let top_candidates = ["--help", "--version", "--json", "--registry"];
    let mut top_registry: Option<PathBuf> = None;
    let mut top_json = false;
    let mut index = 0;
    let mut command: Option<String> = None;
    let mut sub_slice: &[OsString] = &[];
    let mut extras: Vec<String> = Vec::new();
    while index < argv.len() {
        #[cfg(unix)]
        if let Some((_, value)) = path_equals_value(&argv[index], &top_candidates, &["--registry"])
        {
            top_registry = Some(PathBuf::from(value));
            index += 1;
            continue;
        }
        let Some(token) = argv[index].to_str() else {
            return Ok(parser_fail(
                MAIN_USAGE,
                "dibs",
                "an argument is not valid Unicode text",
            ));
        };
        if token == "-h" || token == "--help" {
            out_line(format_args!("{MAIN_HELP}"));
            return Ok(ExitCode::from(0));
        }
        if token == "-V" {
            if top_json || has_json_flag_in_tail(&argv[index + 1..]) {
                return Ok(parser_fail(
                    MAIN_USAGE,
                    "dibs",
                    "argument -j/--json: not allowed with argument -V/--version",
                ));
            }
            out_line(format_args!("dibs {}", env!("CARGO_PKG_VERSION")));
            return Ok(ExitCode::from(0));
        }
        if token == "-j" {
            top_json = true;
            index += 1;
            continue;
        }
        if token.starts_with("--") && token.len() > 2 {
            let (name, inline_value) = match token.find('=') {
                Some(pos) => (&token[..pos], Some(token[pos + 1..].to_string())),
                None => (token, None),
            };
            match resolve_long(name, &top_candidates) {
                Ok(Some("--help")) => {
                    out_line(format_args!("{MAIN_HELP}"));
                    return Ok(ExitCode::from(0));
                }
                Ok(Some("--version")) => {
                    if let Some(value) = inline_value {
                        return Ok(parser_fail(
                            MAIN_USAGE,
                            "dibs",
                            &format!(
                                "argument -V/--version: ignored explicit argument {}",
                                dibs::py_repr(&value)
                            ),
                        ));
                    }
                    if top_json || has_json_flag_in_tail(&argv[index + 1..]) {
                        return Ok(parser_fail(
                            MAIN_USAGE,
                            "dibs",
                            "argument -j/--json: not allowed with argument -V/--version",
                        ));
                    }
                    out_line(format_args!("dibs {}", env!("CARGO_PKG_VERSION")));
                    return Ok(ExitCode::from(0));
                }
                Ok(Some("--json")) => {
                    if let Some(value) = inline_value {
                        return Ok(parser_fail(
                            MAIN_USAGE,
                            "dibs",
                            &format!(
                                "argument -j/--json: ignored explicit argument {}",
                                dibs::py_repr(&value)
                            ),
                        ));
                    }
                    top_json = true;
                    index += 1;
                    continue;
                }
                Ok(Some("--registry")) => {
                    if let Some(value) = inline_value {
                        top_registry = Some(PathBuf::from(value));
                        index += 1;
                    } else if index + 1 < argv.len()
                        && !argv[index + 1].to_str().is_some_and(is_option_like)
                    {
                        top_registry = Some(PathBuf::from(&argv[index + 1]));
                        index += 2;
                    } else {
                        return Ok(parser_fail(
                            MAIN_USAGE,
                            "dibs",
                            "argument --registry: expected one argument",
                        ));
                    }
                    continue;
                }
                Ok(Some(_)) => unreachable!(),
                Ok(None) => {
                    extras.push(token.to_string());
                    index += 1;
                    continue;
                }
                Err(message) => {
                    return Ok(parser_fail(MAIN_USAGE, "dibs", &message));
                }
            }
        }
        if token != "--" && is_option_like(token) {
            extras.push(token.to_string());
            index += 1;
            continue;
        }
        if !COMMANDS.contains(&token) {
            return Ok(parser_fail(
                MAIN_USAGE,
                "dibs",
                &format!(
                    "argument command: invalid choice: {} (choose from 'offer', 'offer-batch', 'claim', 'claim-next', 'renew', 'release', 'complete', 'reset', 'board', 'ready', 'show', 'history', 'compact', 'completions')",
                    dibs::py_repr(token)
                ),
            ));
        }
        command = Some(token.to_string());
        sub_slice = &argv[index + 1..];
        break;
    }
    let Some(command) = command else {
        let mut missing = Vec::new();
        if top_registry.is_none() && std::env::var_os("DIBS_REGISTRY").is_none() {
            missing.push("--registry");
        }
        missing.push("command");
        return Ok(parser_fail(
            MAIN_USAGE,
            "dibs",
            &format!(
                "the following arguments are required: {}",
                missing.join(", ")
            ),
        ));
    };
    let args = match parse_sub(&command, sub_slice) {
        Ok(args) => args,
        Err(failure) => {
            return Ok(parser_fail(failure.usage, &failure.prog, &failure.message));
        }
    };
    if let Err(failure) = require(&command, &args) {
        return Ok(parser_fail(failure.usage, &failure.prog, &failure.message));
    }
    let json_mode = top_json || args.json;
    if command == "completions" {
        if json_mode {
            return Ok(parser_fail(
                sub_usage("completions"),
                "dibs completions",
                "argument -j/--json: not allowed with completions",
            ));
        }
        extras.extend(args.extras.iter().cloned());
        if !extras.is_empty() {
            return Ok(parser_fail(
                MAIN_USAGE,
                "dibs",
                &format!("unrecognized arguments: {}", extras.join(" ")),
            ));
        }
        let shell = &args.positionals[0];
        if !matches!(shell.as_str(), "bash" | "zsh" | "fish") {
            return Ok(parser_fail(
                sub_usage("completions"),
                "dibs completions",
                &format!(
                    "argument shell: invalid choice: {} (choose from 'bash', 'zsh', 'fish')",
                    dibs::py_repr(shell)
                ),
            ));
        }
        out_raw(completion_script(shell));
        return Ok(ExitCode::from(0));
    }
    let registry_path = args
        .registry_path
        .clone()
        .or(top_registry)
        .or_else(|| std::env::var_os("DIBS_REGISTRY").map(PathBuf::from));
    let Some(registry_path) = registry_path else {
        return Ok(parser_fail(
            MAIN_USAGE,
            "dibs",
            "the following arguments are required: --registry",
        ));
    };
    extras.extend(args.extras.iter().cloned());
    if !extras.is_empty() {
        return Ok(parser_fail(
            MAIN_USAGE,
            "dibs",
            &format!("unrecognized arguments: {}", extras.join(" ")),
        ));
    }
    let registry = Registry::new(registry_path);
    let item = args.positionals.first().cloned();
    match command.as_str() {
        "offer" => {
            let item = item.unwrap();
            validate_name(&Value::Str(item.clone()), "item")?;
            let note = option_text(&args, "--note").unwrap_or("");
            let priority = match option_int(&args, "--priority") {
                Some(val) => validate_priority(val)?,
                None => PRIORITY_DEFAULT,
            };
            let tags = option_texts(&args, "--tag");
            let depends_on = option_texts(&args, "--depends-on");
            let max_takeovers = match option_int(&args, "--max-takeovers") {
                Some(val) => validate_max_takeovers(val)?,
                None => None,
            };
            let spec = OfferSpec::new(&item, note)
                .with_priority(priority)
                .with_tags(tags)
                .with_depends_on(depends_on)
                .with_max_takeovers(max_takeovers);
            let now = Instant::now_utc()?;
            registry.offer_with(&spec, &now)?;
            if json_mode {
                let mut out = String::from("{\"item\": ");
                escape_json_string(&item, &mut out);
                out.push_str(", \"status\": \"open\", \"epoch\": 0}");
                out_line(format_args!("{out}"));
            } else {
                out_line(format_args!("offered {item}"));
            }
        }
        "offer-batch" => {
            registry.validate_registry_path()?;
            let raw_input = read_offer_batch_input(args.file_path.as_ref())?;
            let specs = parse_offer_batch_jsonl(&raw_input)?;
            let now = Instant::now_utc()?;
            let offered = registry.offer_many(&specs, &now)?;
            if json_mode {
                let mut out = format!("{{\"count\": {}, \"offered\": ", offered.len());
                push_json_str_list(&offered, &mut out);
                out.push('}');
                out_line(format_args!("{out}"));
            } else {
                out_line(format_args!("offered {}", offered.len()));
            }
        }
        "claim" => {
            let item = item.unwrap();
            let holder = option_text(&args, "--holder").unwrap();
            let ttl = ttl_argument(&item, holder, option_int(&args, "--ttl").unwrap())?;
            let now = Instant::now_utc()?;
            let granted = registry.claim(&item, holder, ttl, &now)?;
            if json_mode {
                out_line(format_args!("{}", render_claim_json(&granted)));
            } else {
                out_line(format_args!("token {}", granted.token));
                out_line(format_args!("epoch {}", granted.epoch));
                out_line(format_args!(
                    "expires_at {}",
                    dibs::format_instant(&granted.expires_at)
                ));
            }
        }
        "claim-next" => {
            let holder = option_text(&args, "--holder").unwrap();
            let ttl = ttl_argument_holder(holder, option_int(&args, "--ttl").unwrap())?;
            let tags = option_texts(&args, "--tag");
            let now = Instant::now_utc()?;
            let granted = registry.claim_next_with_tags(holder, ttl, &tags, &now)?;
            if json_mode {
                out_line(format_args!("{}", render_claim_json(&granted)));
            } else {
                out_line(format_args!("item {}", granted.item));
                out_line(format_args!("token {}", granted.token));
                out_line(format_args!("epoch {}", granted.epoch));
                out_line(format_args!(
                    "expires_at {}",
                    dibs::format_instant(&granted.expires_at)
                ));
            }
        }
        "renew" => {
            let item = item.unwrap();
            let holder = option_text(&args, "--holder").unwrap();
            let token = resolve_token(&args)?;
            let epoch = epoch_argument(option_int(&args, "--epoch").unwrap());
            let ttl = ttl_argument(&item, holder, option_int(&args, "--ttl").unwrap())?;
            let now = Instant::now_utc()?;
            let expires = registry.renew(&item, holder, &token, epoch, ttl, &now)?;
            let expires_str = dibs::format_instant(&expires);
            if json_mode {
                let mut out = String::from("{\"item\": ");
                escape_json_string(&item, &mut out);
                out.push_str(", \"holder\": ");
                escape_json_string(holder, &mut out);
                out.push_str(&format!(", \"epoch\": {epoch}, \"expires_at\": "));
                escape_json_string(&expires_str, &mut out);
                out.push('}');
                out_line(format_args!("{out}"));
            } else {
                out_line(format_args!("expires_at {expires_str}"));
            }
        }
        "release" => {
            let item = item.unwrap();
            let holder = option_text(&args, "--holder").unwrap();
            let token = resolve_token(&args)?;
            let epoch = epoch_argument(option_int(&args, "--epoch").unwrap());
            let now = Instant::now_utc()?;
            registry.release(&item, holder, &token, epoch, &now)?;
            if json_mode {
                let mut out = String::from("{\"item\": ");
                escape_json_string(&item, &mut out);
                out.push_str(&format!(", \"status\": \"open\", \"epoch\": {epoch}}}"));
                out_line(format_args!("{out}"));
            } else {
                out_line(format_args!("released {item}"));
            }
        }
        "complete" => {
            let item = item.unwrap();
            let holder = option_text(&args, "--holder").unwrap();
            let token = resolve_token(&args)?;
            let epoch = epoch_argument(option_int(&args, "--epoch").unwrap());
            let now = Instant::now_utc()?;
            registry.complete(&item, holder, &token, epoch, &now)?;
            if json_mode {
                let mut out = String::from("{\"item\": ");
                escape_json_string(&item, &mut out);
                out.push_str(&format!(
                    ", \"status\": \"completed\", \"epoch\": {epoch}}}"
                ));
                out_line(format_args!("{out}"));
            } else {
                out_line(format_args!("completed {item}"));
            }
        }
        "reset" => {
            let item = item.unwrap();
            let now = Instant::now_utc()?;
            let detail = registry.reset(&item, &now)?;
            if json_mode {
                let mut out = String::from("{\"item\": ");
                escape_json_string(&item, &mut out);
                out.push_str(&format!(
                    ", \"status\": \"open\", \"epoch\": {}, \"takeover_count\": 0}}",
                    detail.epoch
                ));
                out_line(format_args!("{out}"));
            } else {
                out_line(format_args!("reset {item}"));
            }
        }
        "board" => {
            let now = Instant::now_utc()?;
            if json_mode {
                let details = registry.board_details(&now)?;
                let mut out = String::from("[");
                for (idx, detail) in details.iter().enumerate() {
                    if idx > 0 {
                        out.push_str(", ");
                    }
                    render_item_detail_json(detail, &mut out);
                }
                out.push(']');
                out_line(format_args!("{out}"));
            } else {
                for row in registry.board(&now)? {
                    if row.status == "claimed" {
                        out_line(format_args!(
                            "{} claimed holder={} expires_at={} epoch={}",
                            row.id,
                            row.holder.as_deref().unwrap_or_default(),
                            row.expires_at.as_deref().unwrap_or_default(),
                            row.epoch
                        ));
                    } else {
                        out_line(format_args!(
                            "{} {} epoch={}",
                            row.id, row.status, row.epoch
                        ));
                    }
                }
            }
        }
        "ready" => {
            let tags = option_texts(&args, "--tag");
            let now = Instant::now_utc()?;
            let ready_items = registry.ready(&tags, &now)?;
            if json_mode {
                let mut out = String::from("[");
                for (idx, detail) in ready_items.iter().enumerate() {
                    if idx > 0 {
                        out.push_str(", ");
                    }
                    render_item_detail_json(detail, &mut out);
                }
                out.push(']');
                out_line(format_args!("{out}"));
            } else {
                for detail in ready_items {
                    out_line(format_args!("{} open epoch={}", detail.id, detail.epoch));
                }
            }
        }
        "show" => {
            let item = item.unwrap();
            let now = Instant::now_utc()?;
            let detail = registry.show(&item, &now)?;
            if json_mode {
                let mut out = String::new();
                render_item_detail_json(&detail, &mut out);
                out_line(format_args!("{out}"));
            } else {
                print_item_detail_human(&detail);
            }
        }
        "compact" => {
            let through_seq = validate_through_seq(option_int(&args, "--through-seq").unwrap())?;
            let archive_path = args.archive_path.as_ref().unwrap();
            registry.validate_registry_path()?;
            registry.validate_archive_path(archive_path)?;
            let archive_utf8 = if json_mode {
                match archive_path.to_str() {
                    Some(text) => Some(text),
                    None => {
                        return Err(Error::InvalidRequest(
                            "--json requires a UTF-8 --archive path".to_string(),
                        ));
                    }
                }
            } else {
                None
            };
            let now = Instant::now_utc()?;
            let summary = registry.compact(through_seq, archive_path, &now)?;
            if json_mode {
                out_line(format_args!(
                    "{}",
                    render_compact_json(&summary, archive_utf8.unwrap())
                ));
            } else {
                out_line(format_args!(
                    "compacted {} events through seq {}",
                    summary.archived_count, summary.through_seq
                ));
            }
        }
        _ => {
            let events = registry.history(item.as_deref())?;
            if json_mode {
                let mut out = String::from("[");
                for (idx, event) in events.iter().enumerate() {
                    if idx > 0 {
                        out.push_str(", ");
                    }
                    render_event_json(event, &mut out);
                }
                out.push(']');
                out_line(format_args!("{out}"));
            } else {
                for event in events {
                    let mut line = format!(
                        "{} {} item={} holder={} at={} epoch={}",
                        event.seq,
                        event.kind,
                        event.item,
                        event.holder.as_deref().unwrap_or("-"),
                        event.at,
                        event.epoch
                    );
                    if let Some(previous) = &event.previous_holder {
                        line.push_str(&format!(" previous_holder={previous}"));
                    }
                    out_line(format_args!("{line}"));
                }
            }
        }
    }
    Ok(ExitCode::from(0))
}

fn main() -> ExitCode {
    run()
}
