//! Command line for dibs, a thin shell over the dibs library.
//!
//! Every command names the registry file first. The clock the
//! command line uses is the wall clock in UTC, and no flag
//! overrides it, so the expiry a caller sees is the expiry the
//! machine clock gives. Errors the protocol raises are printed
//! as one line on stderr with the exit code the error carries,
//! and no traceback is ever printed for them. The argument parser
//! mirrors the Python argparse surface of the Round 1 command
//! line: the same usage texts, the same parser error messages at
//! exit 2, unambiguous prefix abbreviations of long options, the
//! equals form, and parser extras reported by the top level
//! parser.

use std::path::PathBuf;
use std::process::ExitCode;

use dibs::{Error, Instant, Registry, Value};

const MAIN_USAGE: &str = "usage: dibs [-h] --registry REGISTRY\n            {offer,claim,renew,release,complete,board,history} ...";
const COMMANDS: [&str; 7] = [
    "offer", "claim", "renew", "release", "complete", "board", "history",
];

const MAIN_HELP: &str = "usage: dibs [-h] --registry REGISTRY
            {offer,claim,renew,release,complete,board,history} ...

Claim tasks from a shared backlog under leases that expire unless they are
renewed.

positional arguments:
  {offer,claim,renew,release,complete,board,history}
    offer               add an item to the backlog
    claim               lease an item under a new claim
    renew               extend the standing lease from its current expiry
    release             end the standing lease early
    complete            mark the item done under its standing lease
    board               show the open items and their standing leases
    history             show the recorded claim history

options:
  -h, --help            show this help message and exit
  --registry REGISTRY   the registry file holding the backlog, created by the
                        first mutation";

fn sub_usage(command: &str) -> &'static str {
    match command {
        "offer" => "usage: dibs offer [-h] [--note NOTE] item",
        "claim" => "usage: dibs claim [-h] --holder HOLDER --ttl TTL item",
        "renew" => {
            "usage: dibs renew [-h] --holder HOLDER --token TOKEN --epoch EPOCH --ttl TTL\n                  item"
        }
        "release" => "usage: dibs release [-h] --holder HOLDER --token TOKEN --epoch EPOCH item",
        "complete" => "usage: dibs complete [-h] --holder HOLDER --token TOKEN --epoch EPOCH item",
        "board" => "usage: dibs board [-h]",
        _ => "usage: dibs history [-h] [item]",
    }
}

fn sub_help(command: &str) -> String {
    let body = match command {
        "offer" => {
            "positional arguments:\n  item\n\noptions:\n  -h, --help   show this help message and exit\n  --note NOTE  a short payload note for the item"
        }
        "claim" => {
            "positional arguments:\n  item\n\noptions:\n  -h, --help       show this help message and exit\n  --holder HOLDER\n  --ttl TTL        the lease length in seconds"
        }
        "renew" => {
            "positional arguments:\n  item\n\noptions:\n  -h, --help       show this help message and exit\n  --holder HOLDER\n  --token TOKEN\n  --epoch EPOCH\n  --ttl TTL        the added lease length in seconds"
        }
        "release" | "complete" => {
            "positional arguments:\n  item\n\noptions:\n  -h, --help       show this help message and exit\n  --holder HOLDER\n  --token TOKEN\n  --epoch EPOCH"
        }
        "board" => "options:\n  -h, --help  show this help message and exit",
        _ => {
            "positional arguments:\n  item\n\noptions:\n  -h, --help  show this help message and exit"
        }
    };
    format!("{}\n\n{body}", sub_usage(command))
}

/// The value options of one subcommand in the order the Python
/// parser defined them, the order its required argument reports
/// and its ambiguity messages follow.
fn value_options(command: &str) -> &'static [&'static str] {
    match command {
        "offer" => &["--note"],
        "claim" => &["--holder", "--ttl"],
        "renew" => &["--holder", "--token", "--epoch", "--ttl"],
        "release" | "complete" => &["--holder", "--token", "--epoch"],
        _ => &[],
    }
}

fn required_options(command: &str) -> &'static [&'static str] {
    match command {
        "claim" => &["--holder", "--ttl"],
        "renew" => &["--holder", "--token", "--epoch", "--ttl"],
        "release" | "complete" => &["--holder", "--token", "--epoch"],
        _ => &[],
    }
}

fn positional_names(command: &str) -> &'static [&'static str] {
    match command {
        "board" => &[],
        _ => &["item"],
    }
}

/// The positionals a subcommand requires, as opposed to the
/// ones it merely accepts: history takes an optional item.
fn required_positionals(command: &str) -> &'static [&'static str] {
    match command {
        "board" | "history" => &[],
        _ => &["item"],
    }
}

fn is_int_option(command: &str, option: &str) -> bool {
    matches!(
        (command, option),
        ("claim", "--ttl")
            | ("renew", "--epoch")
            | ("renew", "--ttl")
            | ("release", "--epoch")
            | ("complete", "--epoch")
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

fn is_option_like(token: &str) -> bool {
    if token.len() < 2 || !token.starts_with('-') || token == "-" {
        return false;
    }
    // A token in the shape of a negative number is positional,
    // as argparse's negative number matcher holds when no
    // option looks like a negative number.
    let body = &token[1..];
    let digits: String = body.chars().filter(|c| *c != '.').collect();
    if !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit()) {
        return false;
    }
    true
}

/// Resolve a long option token against the candidates of one
/// parser, the argparse way: an exact match wins, an
/// unambiguous prefix matches, an ambiguous prefix is an
/// error, and no match means the token is an extra.
fn resolve_long<'a>(name: &'a str, candidates: &[&'a str]) -> Result<Option<&'a str>, String> {
    if candidates.contains(&name) {
        return Ok(Some(name));
    }
    let matches: Vec<&str> = candidates
        .iter()
        .filter(|candidate| candidate.starts_with(name))
        .cloned()
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
    extras: Vec<String>,
}

/// Python's int conversion for the numeric arguments: optional
/// surrounding whitespace, an optional sign, digits with single
/// underscores between them. The value is kept whole, at 128 bit
/// width when it fits and as its canonical digit text when it
/// does not, so a later validation prints the number the caller
/// typed rather than a parser failure.
fn python_int(text: &str) -> Option<Value> {
    let trimmed = text.trim_matches(|c| matches!(c, ' ' | '\t' | '\n' | '\r' | '\x0b' | '\x0c'));
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
        } else if ch.is_ascii_digit() {
            digits.push(ch);
            previous_digit = true;
            previous_underscore = false;
            seen_digit = true;
        } else {
            return None;
        }
    }
    if previous_underscore || !seen_digit {
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

/// Parse one subcommand's arguments. Option values are drawn
/// from the following token unless it is option like, numeric
/// options are converted as they are consumed, and tokens the
/// subcommand does not consume are collected as extras for the
/// top level parser to report. A double dash stops option
/// handling, and when an extra follows it the dash itself is
/// reported with the extras, as the Python parser reported it.
fn parse_sub(command: &str, tokens: &[String]) -> Result<SubArgs, ParserError> {
    let mut candidates: Vec<&str> = vec!["--help"];
    candidates.extend_from_slice(value_options(command));
    let positional_count = positional_names(command).len();
    let mut result = SubArgs {
        positionals: Vec::new(),
        options: Vec::new(),
        extras: Vec::new(),
    };
    let mut index = 0;
    let mut after_separator = false;
    let mut separator_pending = false;
    while index < tokens.len() {
        let token = &tokens[index];
        if !after_separator && token == "--" {
            after_separator = true;
            separator_pending = true;
            index += 1;
            continue;
        }
        if !after_separator && (token == "-h" || token == "--help") {
            print!("{}", sub_help(command));
            println!();
            std::process::exit(0);
        }
        if !after_separator && token.starts_with("--") && token.len() > 2 {
            let (name, inline_value) = match token.find('=') {
                Some(pos) => (&token[..pos], Some(token[pos + 1..].to_string())),
                None => (&token[..], None),
            };
            match resolve_long(name, &candidates) {
                Ok(Some("--help")) => {
                    print!("{}", sub_help(command));
                    println!();
                    std::process::exit(0);
                }
                Ok(Some(option)) => {
                    let raw = if let Some(value) = inline_value {
                        index += 1;
                        value
                    } else if index + 1 < tokens.len() && !is_option_like(&tokens[index + 1]) {
                        let value = tokens[index + 1].clone();
                        index += 2;
                        value
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
                    result.extras.push(token.clone());
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
            result.extras.push(token.clone());
            index += 1;
            continue;
        }
        if result.positionals.len() < positional_count {
            result.positionals.push(token.clone());
        } else {
            if separator_pending {
                result.extras.push("--".to_string());
                separator_pending = false;
            }
            result.extras.push(token.clone());
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

fn parser_fail(usage: &str, prog: &str, message: &str) -> ExitCode {
    eprintln!("{usage}");
    eprintln!("{prog}: error: {message}");
    ExitCode::from(2)
}

fn report_error(error: &Error) -> ExitCode {
    eprintln!("error: {}", error.message());
    ExitCode::from(error.exit_code() as u8)
}

/// The time to live of a claim or renew, as a plain integer. The
/// value the parser produced is validated through the library
/// validator here when it is wider than the typed call carries,
/// after the names have been validated in the order the library
/// validates them, so the refusal and its wording are the
/// library's own.
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

/// The presented epoch of a guarded operation, as the integer
/// the typed call carries. A value wider than the integer width
/// is presented as a count no standing epoch can equal, and the
/// possession check refuses it in its standing order.
fn epoch_argument(value: &Value) -> i128 {
    match value {
        Value::Int(number) => *number,
        _ => i128::MAX,
    }
}

fn run() -> ExitCode {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    match dispatch(&argv) {
        Ok(code) => code,
        Err(error) => report_error(&error),
    }
}

fn dispatch(argv: &[String]) -> Result<ExitCode, Error> {
    // Parse the top level: the registry option and the subcommand
    // word, with the remaining tokens handed to the subcommand.
    // The first token that is not an option is the subcommand,
    // and a double dash before it is taken as the subcommand word
    // itself, as the Python parser took it.
    let mut registry_path: Option<String> = None;
    let mut index = 0;
    let mut command: Option<String> = None;
    let mut rest: Vec<String> = Vec::new();
    let mut extras: Vec<String> = Vec::new();
    while index < argv.len() {
        let token = &argv[index];
        if token == "-h" || token == "--help" {
            println!("{MAIN_HELP}");
            return Ok(ExitCode::from(0));
        }
        if token.starts_with("--") && token.len() > 2 {
            let (name, inline_value) = match token.find('=') {
                Some(pos) => (&token[..pos], Some(token[pos + 1..].to_string())),
                None => (&token[..], None),
            };
            match resolve_long(name, &["--help", "--registry"]) {
                Ok(Some("--help")) => {
                    println!("{MAIN_HELP}");
                    return Ok(ExitCode::from(0));
                }
                Ok(Some("--registry")) => {
                    if let Some(value) = inline_value {
                        registry_path = Some(value);
                        index += 1;
                    } else if index + 1 < argv.len() && !is_option_like(&argv[index + 1]) {
                        registry_path = Some(argv[index + 1].clone());
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
                    extras.push(token.clone());
                    index += 1;
                    continue;
                }
                Err(message) => {
                    return Ok(parser_fail(MAIN_USAGE, "dibs", &message));
                }
            }
        }
        if token != "--" && is_option_like(token) {
            extras.push(token.clone());
            index += 1;
            continue;
        }
        command = Some(token.clone());
        rest = argv[index + 1..].to_vec();
        break;
    }
    let Some(command) = command else {
        let mut missing = Vec::new();
        if registry_path.is_none() {
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
    if !COMMANDS.contains(&command.as_str()) {
        return Ok(parser_fail(
            MAIN_USAGE,
            "dibs",
            &format!(
                "argument command: invalid choice: {} (choose from 'offer', 'claim', 'renew', 'release', 'complete', 'board', 'history')",
                dibs::py_repr(&command)
            ),
        ));
    }
    let args = match parse_sub(&command, &rest) {
        Ok(args) => args,
        Err(failure) => {
            return Ok(parser_fail(failure.usage, &failure.prog, &failure.message));
        }
    };
    if let Err(failure) = require(&command, &args) {
        return Ok(parser_fail(failure.usage, &failure.prog, &failure.message));
    }
    if registry_path.is_none() {
        return Ok(parser_fail(
            MAIN_USAGE,
            "dibs",
            "the following arguments are required: --registry",
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
    let registry = Registry::new(PathBuf::from(registry_path.unwrap()));
    let now = Instant::now_utc();
    let item = args.positionals.first().cloned();
    match command.as_str() {
        "offer" => {
            let item = item.unwrap();
            let note = option_text(&args, "--note").unwrap_or("");
            registry.offer(&item, note, &now)?;
            println!("offered {item}");
        }
        "claim" => {
            let item = item.unwrap();
            let holder = option_text(&args, "--holder").unwrap();
            let ttl = ttl_argument(&item, holder, option_int(&args, "--ttl").unwrap())?;
            let granted = registry.claim(&item, holder, ttl, &now)?;
            println!("token {}", granted.token);
            println!("epoch {}", granted.epoch);
            println!("expires_at {}", dibs::format_instant(&granted.expires_at));
        }
        "renew" => {
            let item = item.unwrap();
            let holder = option_text(&args, "--holder").unwrap();
            let token = option_text(&args, "--token").unwrap();
            let epoch = epoch_argument(option_int(&args, "--epoch").unwrap());
            let ttl = ttl_argument(&item, holder, option_int(&args, "--ttl").unwrap())?;
            let expires = registry.renew(&item, holder, token, epoch, ttl, &now)?;
            println!("expires_at {}", dibs::format_instant(&expires));
        }
        "release" => {
            let item = item.unwrap();
            let holder = option_text(&args, "--holder").unwrap();
            let token = option_text(&args, "--token").unwrap();
            let epoch = epoch_argument(option_int(&args, "--epoch").unwrap());
            registry.release(&item, holder, token, epoch, &now)?;
            println!("released {item}");
        }
        "complete" => {
            let item = item.unwrap();
            let holder = option_text(&args, "--holder").unwrap();
            let token = option_text(&args, "--token").unwrap();
            let epoch = epoch_argument(option_int(&args, "--epoch").unwrap());
            registry.complete(&item, holder, token, epoch, &now)?;
            println!("completed {item}");
        }
        "board" => {
            for row in registry.board(&now)? {
                if row.status == "claimed" {
                    println!(
                        "{} claimed holder={} expires_at={} epoch={}",
                        row.id,
                        row.holder.as_deref().unwrap_or_default(),
                        row.expires_at.as_deref().unwrap_or_default(),
                        row.epoch
                    );
                } else {
                    println!("{} open epoch={}", row.id, row.epoch);
                }
            }
        }
        _ => {
            for event in registry.history(item.as_deref())? {
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
                println!("{line}");
            }
        }
    }
    Ok(ExitCode::from(0))
}

fn main() -> ExitCode {
    run()
}
