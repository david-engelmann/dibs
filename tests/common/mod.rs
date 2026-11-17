//! Helpers shared by the dibs test suites.
//!
//! Each file under tests/ is its own crate, so the helpers
//! more than one suite needs stand here once: the fixed
//! instant the suites stage against, the scratch directory
//! logic each suite stages in, the error extractor, the
//! digest formatting, and the builder for a registry
//! document of offered items. A helper only one suite uses
//! stands in that suite's own file. A suite is not expected
//! to use every helper here, which is what the allow below
//! records.

#![allow(dead_code)]

use std::path::PathBuf;

use dibs::{Error, Instant};
use sha2::{Digest, Sha256};

pub fn t0() -> Instant {
    Instant::from_civil(2031, 5, 4, 12, 0, 0, 0, Some(0)).unwrap()
}

pub fn at(seconds: i64) -> Instant {
    t0().plus_seconds(seconds)
}

pub fn temp_dir(prefix: &str, tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("{prefix}{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

pub fn expect_error<T>(result: dibs::Result<T>) -> Error {
    match result {
        Ok(_) => panic!("the operation succeeded where a refusal was expected"),
        Err(error) => error,
    }
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

pub fn offered_document(ids: &[String]) -> String {
    let mut out = String::from("{\"format\": \"dibs-registry/1\", \"items\": [");
    for (index, id) in ids.iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        out.push_str(&format!(
            "{{\"id\": \"{id}\", \"note\": \"\", \"status\": \"open\", \
             \"holder\": null, \"token_hash\": null, \"epoch\": 0, \"expires_at\": null}}"
        ));
    }
    out.push_str("], \"history\": [");
    for (index, id) in ids.iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        out.push_str(&format!(
            "{{\"seq\": {}, \"kind\": \"offered\", \"item\": \"{id}\", \
             \"holder\": null, \"at\": \"2031-05-04T12:00:00+00:00\", \"epoch\": 0, \
             \"previous_holder\": null}}",
            index + 1
        ));
    }
    out.push_str("]}");
    out
}
