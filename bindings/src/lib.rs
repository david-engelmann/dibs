//! The thin Python binding over the dibs crate.
//!
//! Every rule lives in the library this crate wraps. The functions
//! here translate Python values into the library arguments, call
//! the one library function each one names, and translate the
//! result or the refusal back, and nothing else. Each call runs
//! detached from the interpreter, so the filesystem and lock work
//! inside a call never stalls the other threads of the calling
//! process while it waits. A library refusal becomes a Python
//! exception carrying the same message text the command line
//! prints, under a subclass of DibsError picked by the error
//! class of the refusal, so a Python caller can tell the classes
//! apart the way a shell caller tells them apart by exit code.
//!
//! The command line reads the wall clock in UTC and offers no
//! override, and the Rust library takes the current instant as an
//! explicit argument on every operation that judges a lease. Under
//! the slice DB-S2 contract amendment, this binding defaults now
//! to None on every lease evaluating operation, reading the UTC
//! wall clock through Instant::now_utc when now is omitted or None
//! and parsing an explicit ISO 8601 string through the library
//! parser when supplied. History judges no lease and takes no
//! instant, here as in the library.

use std::path::PathBuf;

use pyo3::create_exception;
use pyo3::exceptions::PyTypeError;
use pyo3::prelude::*;
use pyo3::types::{PyBool, PyByteArray, PyBytes, PyDict, PyInt, PyString};

use dibs_core::{
    BoardRow, ClaimGranted, Error, Event, Instant, ItemDetail, OfferSpec, Registry, Value,
};

create_exception!(dibs, DibsError, pyo3::exceptions::PyException);
create_exception!(dibs, InvalidNameError, DibsError);
create_exception!(dibs, InvalidTtlError, DibsError);
create_exception!(dibs, InvalidRequestError, DibsError);
create_exception!(dibs, NotFoundError, DibsError);
create_exception!(dibs, RefusalError, DibsError);
create_exception!(dibs, CorruptError, DibsError);
create_exception!(dibs, FailureError, DibsError);

/// Translate a library refusal into the Python exception of its
/// error class, carrying the message the command line prints.
fn to_py_error(error: Error) -> PyErr {
    let message = error.message().to_string();
    match error {
        Error::InvalidName(_) => InvalidNameError::new_err(message),
        Error::InvalidTtl(_) => InvalidTtlError::new_err(message),
        Error::InvalidRequest(_) => InvalidRequestError::new_err(message),
        Error::NotFound(_) => NotFoundError::new_err(message),
        Error::Refusal(_) => RefusalError::new_err(message),
        Error::Corrupt(_) => CorruptError::new_err(message),
        Error::Failure(_) => FailureError::new_err(message),
    }
}

/// Refuse a boolean where an integer belongs, with a TypeError
/// naming the argument, because Python spells True and False as
/// the integers 1 and 0 and neither a time to live nor an epoch
/// is ever meant as a truth value.
fn refuse_boolean(value: &Bound<'_, PyAny>, name: &str) -> PyResult<()> {
    if value.is_instance_of::<PyBool>() {
        Err(PyTypeError::new_err(format!(
            "{name} must be an integer, not a boolean"
        )))
    } else {
        Ok(())
    }
}

/// The canonical decimal text of an integer too wide for the
/// 128 bit width, taken from the value arithmetically.
fn decimal_text(value: &Bound<'_, PyAny>) -> PyResult<String> {
    let int_type = value.py().get_type::<PyInt>();
    let bits: usize = int_type.call_method1("bit_length", (value,))?.extract()?;
    let byte_count = bits / 8 + 2;
    let signed = PyDict::new(value.py());
    signed.set_item("signed", true)?;
    let bytes: Vec<u8> = int_type
        .call_method("to_bytes", (value, byte_count, "big"), Some(&signed))?
        .extract()?;
    let negative = bytes.first().is_some_and(|byte| byte & 0x80 != 0);
    let mut magnitude = bytes;
    if negative {
        for byte in magnitude.iter_mut() {
            *byte = !*byte;
        }
        let mut carry = 1u16;
        for byte in magnitude.iter_mut().rev() {
            let widened = *byte as u16 + carry;
            *byte = widened as u8;
            carry = widened >> 8;
        }
    }
    let first = match magnitude.iter().position(|byte| *byte != 0) {
        Some(index) => index,
        None => return Ok("0".to_string()),
    };
    let mut current: Vec<u8> = magnitude[first..].to_vec();
    let mut digits: Vec<u8> = Vec::new();
    while !current.is_empty() {
        let mut remainder = 0u32;
        let mut next: Vec<u8> = Vec::with_capacity(current.len());
        for byte in &current {
            let acc = (remainder << 8) | *byte as u32;
            let quotient = acc / 10;
            remainder = acc % 10;
            if !next.is_empty() || quotient != 0 {
                next.push(quotient as u8);
            }
        }
        digits.push(b'0' + remainder as u8);
        current = next;
    }
    if digits.is_empty() {
        digits.push(b'0');
    }
    digits.reverse();
    let mut text: String = digits.iter().map(|byte| *byte as char).collect();
    if negative && text != "0" {
        text.insert(0, '-');
    }
    Ok(text)
}

/// Keep an integer argument whole, at 128 bit width when it fits
/// and as its canonical decimal text when it does not.
fn integer_value(value: &Bound<'_, PyAny>) -> PyResult<Value> {
    match value.extract::<i128>() {
        Ok(number) => Ok(Value::Int(number)),
        Err(err) => {
            if value.is_instance_of::<PyInt>() {
                Ok(Value::BigInt(decimal_text(value)?))
            } else {
                Err(err)
            }
        }
    }
}

/// Resolve the time to live argument of a claim or a renew.
fn ttl_argument(item: &str, holder: &str, value: &Bound<'_, PyAny>) -> PyResult<i64> {
    refuse_boolean(value, "ttl")?;
    let wide = integer_value(value)?;
    match &wide {
        Value::Int(number) if *number >= i64::MIN as i128 && *number <= i64::MAX as i128 => {
            Ok(*number as i64)
        }
        _ => {
            dibs_core::validate_name(&Value::Str(item.to_string()), "item").map_err(to_py_error)?;
            dibs_core::validate_name(&Value::Str(holder.to_string()), "holder")
                .map_err(to_py_error)?;
            dibs_core::validate_ttl(&wide).map_err(to_py_error)
        }
    }
}

/// Resolve the time to live argument of claim_next, which names
/// no item upfront.
fn ttl_argument_holder(holder: &str, value: &Bound<'_, PyAny>) -> PyResult<i64> {
    refuse_boolean(value, "ttl")?;
    let wide = integer_value(value)?;
    match &wide {
        Value::Int(number) if *number >= i64::MIN as i128 && *number <= i64::MAX as i128 => {
            Ok(*number as i64)
        }
        _ => {
            dibs_core::validate_name(&Value::Str(holder.to_string()), "holder")
                .map_err(to_py_error)?;
            dibs_core::validate_ttl(&wide).map_err(to_py_error)
        }
    }
}

/// Resolve the presented epoch argument of a guarded operation.
fn epoch_argument(value: &Bound<'_, PyAny>) -> PyResult<i128> {
    refuse_boolean(value, "epoch")?;
    match value.extract::<i128>() {
        Ok(number) => Ok(number),
        Err(err) => {
            if value.is_instance_of::<PyInt>() {
                Ok(i128::MAX)
            } else {
                Err(err)
            }
        }
    }
}

/// Parse an explicit instant argument with the library parser.
fn instant_argument(text: &str) -> PyResult<Instant> {
    dibs_core::parse_instant(text).map_err(|_| {
        to_py_error(Error::InvalidRequest(format!(
            "the current instant holds unparseable text {}",
            dibs_core::py_repr(text)
        )))
    })
}

/// Resolve the optional now argument, defaulting to the UTC wall
/// clock when None is passed or omitted (amended DB-3).
fn resolve_now(now: Option<&str>) -> PyResult<Instant> {
    match now {
        Some(text) => instant_argument(text),
        None => Instant::now_utc().map_err(to_py_error),
    }
}

fn extract_offer_specs(specs: &Bound<'_, PyAny>) -> PyResult<Vec<OfferSpec>> {
    if specs.is_instance_of::<PyString>()
        || specs.is_instance_of::<PyBytes>()
        || specs.is_instance_of::<PyByteArray>()
        || specs.is_instance_of::<PyDict>()
    {
        return Err(PyTypeError::new_err(
            "specs must be an iterable of item ids or offer dicts, not a single str, bytes, or dict",
        ));
    }
    let mut out = Vec::new();
    for item_res in specs.try_iter()? {
        let entry = item_res?;
        if entry.is_instance_of::<PyString>() {
            let id: String = entry.extract()?;
            out.push(OfferSpec {
                id,
                note: String::new(),
            });
        } else if entry.is_instance_of::<PyDict>() {
            let dict: Bound<'_, PyDict> = entry.extract()?;
            for (k, _) in dict.iter() {
                let key: String = k.extract().map_err(|_| {
                    to_py_error(Error::InvalidRequest(
                        "offer spec dictionary keys must be strings".to_string(),
                    ))
                })?;
                if key != "id" && key != "note" {
                    return Err(to_py_error(Error::InvalidRequest(format!(
                        "offer spec carries unknown field {}",
                        dibs_core::py_repr(&key)
                    ))));
                }
            }
            let Some(id_any) = dict.get_item("id")? else {
                return Err(to_py_error(Error::InvalidRequest(
                    "offer spec dictionary is missing required field 'id'".to_string(),
                )));
            };
            let id: String = id_any
                .extract()
                .map_err(|_| PyTypeError::new_err("offer spec 'id' must be a string"))?;
            let note = match dict.get_item("note")? {
                None => String::new(),
                Some(note_any) if note_any.is_none() => String::new(),
                Some(note_any) => note_any.extract().map_err(|_| {
                    PyTypeError::new_err("offer spec 'note' must be a string or None")
                })?,
            };
            out.push(OfferSpec { id, note });
        } else {
            return Err(PyTypeError::new_err(
                "each offer spec must be a string item id or a dictionary",
            ));
        }
    }
    Ok(out)
}

/// Translate the one return of a claim into a plain dictionary
/// carrying the issued token and the lease facts.
fn claim_dict(py: Python<'_>, granted: &ClaimGranted) -> PyResult<Py<PyDict>> {
    let dict = PyDict::new(py);
    dict.set_item("item", &granted.item)?;
    dict.set_item("holder", &granted.holder)?;
    dict.set_item("token", &granted.token)?;
    dict.set_item("epoch", granted.epoch)?;
    dict.set_item("expires_at", dibs_core::format_instant(&granted.expires_at))?;
    Ok(dict.unbind())
}

/// Translate one board row into a plain dictionary carrying the
/// six v1 board fields, with no token and no token hash.
fn board_dict(py: Python<'_>, row: &BoardRow) -> PyResult<Py<PyDict>> {
    let dict = PyDict::new(py);
    dict.set_item("id", &row.id)?;
    dict.set_item("note", &row.note)?;
    dict.set_item("status", &row.status)?;
    dict.set_item("holder", &row.holder)?;
    dict.set_item("expires_at", &row.expires_at)?;
    dict.set_item("epoch", row.epoch)?;
    Ok(dict.unbind())
}

/// Translate one ItemDetail into the twelve key summary dictionary,
/// with no token and no token hash.
fn item_detail_dict(py: Python<'_>, detail: &ItemDetail) -> PyResult<Py<PyDict>> {
    let dict = PyDict::new(py);
    dict.set_item("id", &detail.id)?;
    dict.set_item("note", &detail.note)?;
    dict.set_item("status", &detail.status)?;
    dict.set_item("effective_status", &detail.effective_status)?;
    dict.set_item("priority", detail.priority)?;
    dict.set_item("tags", &detail.tags)?;
    dict.set_item("depends_on", &detail.depends_on)?;
    dict.set_item("holder", &detail.holder)?;
    dict.set_item("expires_at", &detail.expires_at)?;
    dict.set_item("epoch", detail.epoch)?;
    dict.set_item("takeover_count", detail.takeover_count)?;
    dict.set_item("max_takeovers", detail.max_takeovers)?;
    Ok(dict.unbind())
}

/// Translate one history event into a plain dictionary carrying
/// the facts the history prints for the event, and no token and
/// no token hash.
fn event_dict(py: Python<'_>, event: &Event) -> PyResult<Py<PyDict>> {
    let dict = PyDict::new(py);
    dict.set_item("seq", event.seq)?;
    dict.set_item("kind", &event.kind)?;
    dict.set_item("item", &event.item)?;
    dict.set_item("holder", &event.holder)?;
    dict.set_item("at", &event.at)?;
    dict.set_item("epoch", event.epoch)?;
    dict.set_item("previous_holder", &event.previous_holder)?;
    Ok(dict.unbind())
}

/// Add an item to the backlog and record the offered event.
#[pyfunction]
#[pyo3(signature = (registry, item, now = None, note = None))]
fn offer(
    py: Python<'_>,
    registry: PathBuf,
    item: &str,
    now: Option<&str>,
    note: Option<&str>,
) -> PyResult<()> {
    let now = resolve_now(now)?;
    let note = note.unwrap_or_default();
    let registry = Registry::new(registry);
    py.detach(|| registry.offer(item, note, &now))
        .map_err(to_py_error)
}

/// Add a batch of items to the backlog all or nothing under one
/// exclusive lock and return the offered item ids in order.
#[pyfunction]
#[pyo3(signature = (registry, specs, now = None))]
fn offer_many(
    py: Python<'_>,
    registry: PathBuf,
    specs: &Bound<'_, PyAny>,
    now: Option<&str>,
) -> PyResult<Vec<String>> {
    let parsed_specs = extract_offer_specs(specs)?;
    let now = resolve_now(now)?;
    let registry = Registry::new(registry);
    py.detach(|| registry.offer_many(&parsed_specs, &now))
        .map_err(to_py_error)
}

/// Lease an open or expired item and return the issued token
/// and the lease facts as a plain dictionary.
#[pyfunction]
#[pyo3(signature = (registry, item, holder, ttl, now = None))]
fn claim(
    py: Python<'_>,
    registry: PathBuf,
    item: &str,
    holder: &str,
    ttl: &Bound<'_, PyAny>,
    now: Option<&str>,
) -> PyResult<Py<PyDict>> {
    let ttl = ttl_argument(item, holder, ttl)?;
    let now = resolve_now(now)?;
    let registry = Registry::new(registry);
    let granted = py
        .detach(|| registry.claim(item, holder, ttl, &now))
        .map_err(to_py_error)?;
    claim_dict(py, &granted)
}

/// Lease the first open or expired item in offer order and return
/// the issued token and the lease facts as a plain dictionary.
#[pyfunction]
#[pyo3(signature = (registry, holder, ttl, now = None))]
fn claim_next(
    py: Python<'_>,
    registry: PathBuf,
    holder: &str,
    ttl: &Bound<'_, PyAny>,
    now: Option<&str>,
) -> PyResult<Py<PyDict>> {
    let ttl = ttl_argument_holder(holder, ttl)?;
    let now = resolve_now(now)?;
    let registry = Registry::new(registry);
    let granted = py
        .detach(|| registry.claim_next(holder, ttl, &now))
        .map_err(to_py_error)?;
    claim_dict(py, &granted)
}

/// Extend the standing lease from its current expiry and return
/// the new expiry as a plain dictionary.
#[pyfunction]
#[pyo3(signature = (registry, item, holder, token, epoch, ttl, now = None))]
#[allow(clippy::too_many_arguments)]
fn renew(
    py: Python<'_>,
    registry: PathBuf,
    item: &str,
    holder: &str,
    token: &str,
    epoch: &Bound<'_, PyAny>,
    ttl: &Bound<'_, PyAny>,
    now: Option<&str>,
) -> PyResult<Py<PyDict>> {
    let epoch = epoch_argument(epoch)?;
    let ttl = ttl_argument(item, holder, ttl)?;
    let now = resolve_now(now)?;
    let registry = Registry::new(registry);
    let expires = py
        .detach(|| registry.renew(item, holder, token, epoch, ttl, &now))
        .map_err(to_py_error)?;
    let dict = PyDict::new(py);
    dict.set_item("expires_at", dibs_core::format_instant(&expires))?;
    Ok(dict.unbind())
}

/// End the standing lease early and return the item to open.
#[pyfunction]
#[pyo3(signature = (registry, item, holder, token, epoch, now = None))]
fn release(
    py: Python<'_>,
    registry: PathBuf,
    item: &str,
    holder: &str,
    token: &str,
    epoch: &Bound<'_, PyAny>,
    now: Option<&str>,
) -> PyResult<()> {
    let epoch = epoch_argument(epoch)?;
    let now = resolve_now(now)?;
    let registry = Registry::new(registry);
    py.detach(|| registry.release(item, holder, token, epoch, &now))
        .map_err(to_py_error)
}

/// Mark the item done under its standing lease.
#[pyfunction]
#[pyo3(signature = (registry, item, holder, token, epoch, now = None))]
fn complete(
    py: Python<'_>,
    registry: PathBuf,
    item: &str,
    holder: &str,
    token: &str,
    epoch: &Bound<'_, PyAny>,
    now: Option<&str>,
) -> PyResult<()> {
    let epoch = epoch_argument(epoch)?;
    let now = resolve_now(now)?;
    let registry = Registry::new(registry);
    py.detach(|| registry.complete(item, holder, token, epoch, &now))
        .map_err(to_py_error)
}

/// Return the items standing on the board in offer order as plain
/// six key dictionaries evaluated at now.
#[pyfunction]
#[pyo3(signature = (registry, now = None))]
fn board(py: Python<'_>, registry: PathBuf, now: Option<&str>) -> PyResult<Vec<Py<PyDict>>> {
    let now = resolve_now(now)?;
    let registry = Registry::new(registry);
    let rows = py.detach(|| registry.board(&now)).map_err(to_py_error)?;
    rows.iter().map(|row| board_dict(py, row)).collect()
}

/// Return the twelve key detail dictionary for one item evaluated
/// at now.
#[pyfunction]
#[pyo3(signature = (registry, item, now = None))]
fn show(py: Python<'_>, registry: PathBuf, item: &str, now: Option<&str>) -> PyResult<Py<PyDict>> {
    let now = resolve_now(now)?;
    let registry = Registry::new(registry);
    let detail = py
        .detach(|| registry.show(item, &now))
        .map_err(to_py_error)?;
    item_detail_dict(py, &detail)
}

/// Return the recorded events in order as plain dictionaries,
/// or only the events of the one item the item argument names.
#[pyfunction]
#[pyo3(signature = (registry, item = None))]
fn history(py: Python<'_>, registry: PathBuf, item: Option<&str>) -> PyResult<Vec<Py<PyDict>>> {
    let registry = Registry::new(registry);
    let events = py.detach(|| registry.history(item)).map_err(to_py_error)?;
    events.iter().map(|event| event_dict(py, event)).collect()
}

#[pymodule]
fn _dibs(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add("DibsError", m.py().get_type::<DibsError>())?;
    m.add("InvalidNameError", m.py().get_type::<InvalidNameError>())?;
    m.add("InvalidTtlError", m.py().get_type::<InvalidTtlError>())?;
    m.add(
        "InvalidRequestError",
        m.py().get_type::<InvalidRequestError>(),
    )?;
    m.add("NotFoundError", m.py().get_type::<NotFoundError>())?;
    m.add("RefusalError", m.py().get_type::<RefusalError>())?;
    m.add("CorruptError", m.py().get_type::<CorruptError>())?;
    m.add("FailureError", m.py().get_type::<FailureError>())?;
    m.add_function(wrap_pyfunction!(offer, m)?)?;
    m.add_function(wrap_pyfunction!(offer_many, m)?)?;
    m.add_function(wrap_pyfunction!(claim, m)?)?;
    m.add_function(wrap_pyfunction!(claim_next, m)?)?;
    m.add_function(wrap_pyfunction!(renew, m)?)?;
    m.add_function(wrap_pyfunction!(release, m)?)?;
    m.add_function(wrap_pyfunction!(complete, m)?)?;
    m.add_function(wrap_pyfunction!(board, m)?)?;
    m.add_function(wrap_pyfunction!(show, m)?)?;
    m.add_function(wrap_pyfunction!(history, m)?)?;
    Ok(())
}
