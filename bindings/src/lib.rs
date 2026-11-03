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
//! The one surface the command line does not have is the instant.
//! The command line reads the wall clock and offers no override,
//! and the library takes the current instant as an argument to
//! every operation that judges a lease. The binding mirrors the
//! library exactly and takes the instant as ISO 8601 text on
//! every such operation, parsed by the library parser, and it
//! never invents a clock of its own. History judges no lease and
//! takes no instant, here as in the library.

use std::path::PathBuf;

use pyo3::create_exception;
use pyo3::exceptions::PyTypeError;
use pyo3::prelude::*;
use pyo3::types::{PyBool, PyDict, PyInt};

use dibs_core::{BoardRow, ClaimGranted, Error, Event, Instant, Registry, Value};

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

/// Keep an integer argument whole, at 128 bit width when it fits
/// and as its canonical decimal text when it does not, the way
/// the command line converter keeps it, so a later library
/// validation prints the number the caller typed rather than a
/// conversion failure at this layer. A value that is no integer
/// at all fails extraction as a plain type error.
fn integer_value(value: &Bound<'_, PyAny>) -> PyResult<Value> {
    match value.extract::<i128>() {
        Ok(number) => Ok(Value::Int(number)),
        Err(err) => {
            if value.is_instance_of::<PyInt>() {
                let text = value.str()?.extract::<String>()?;
                Ok(Value::BigInt(text))
            } else {
                Err(err)
            }
        }
    }
}

/// Resolve the time to live argument of a claim or a renew.
///
/// A time to live that fits the typed call passes straight to
/// it. A wider one is validated through the library validators
/// here, after the names have been validated in the order the
/// library validates them, so the refusal and its wording are
/// the library's own. This is the resolution the command line
/// performs for its own wider values, performed here with the
/// same library calls.
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

/// Resolve the presented epoch argument of a guarded operation.
///
/// The value passes at the width the typed call carries. A value
/// wider than that width is presented as a count no standing
/// epoch can equal, and the library possession check refuses it
/// in its standing order, exactly as the command line presents
/// its own wider values.
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

/// Parse the instant argument with the library parser.
///
/// The text is ISO 8601 in the family the library accepts, and
/// the parse is the library's own, so a spelling the library
/// takes is a spelling this surface takes. Text the parser
/// refuses draws the invalid request error of the library, in
/// the wording the stored instant refusal uses, and a parsed
/// instant with no offset is refused downstream by the library
/// itself as a naive current instant.
fn instant_argument(text: &str) -> PyResult<Instant> {
    dibs_core::parse_instant(text).map_err(|_| {
        to_py_error(Error::InvalidRequest(format!(
            "the current instant holds unparseable text {}",
            dibs_core::py_repr(text)
        )))
    })
}

/// Translate the one return of a claim into a plain dictionary
/// carrying the issued token and the lease facts. This return is
/// the only place a token appears on this surface, exactly as
/// the claim output is the only place one appears on the
/// command line.
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
/// facts the board prints for the item, and no token and no
/// token hash, as the board itself carries neither.
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

/// Translate one history event into a plain dictionary carrying
/// the facts the history prints for the event, and no token and
/// no token hash, as the history itself carries neither.
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
///
/// The note argument is the offer note and defaults to the empty
/// note the command line records when no note is given. The now
/// argument is the current instant as ISO 8601 text.
#[pyfunction]
#[pyo3(signature = (registry, item, now, note = None))]
fn offer(
    py: Python<'_>,
    registry: PathBuf,
    item: &str,
    now: &str,
    note: Option<&str>,
) -> PyResult<()> {
    let now = instant_argument(now)?;
    let note = note.unwrap_or_default();
    let registry = Registry::new(registry);
    py.detach(|| registry.offer(item, note, &now))
        .map_err(to_py_error)
}

/// Lease an open or expired item and return the issued token
/// and the lease facts as a plain dictionary.
///
/// The ttl argument is the lease length in seconds and the now
/// argument is the current instant as ISO 8601 text.
#[pyfunction]
fn claim(
    py: Python<'_>,
    registry: PathBuf,
    item: &str,
    holder: &str,
    ttl: &Bound<'_, PyAny>,
    now: &str,
) -> PyResult<Py<PyDict>> {
    let ttl = ttl_argument(item, holder, ttl)?;
    let now = instant_argument(now)?;
    let registry = Registry::new(registry);
    let granted = py
        .detach(|| registry.claim(item, holder, ttl, &now))
        .map_err(to_py_error)?;
    claim_dict(py, &granted)
}

/// Extend the standing lease from its current expiry and return
/// the new expiry as a plain dictionary.
///
/// The epoch argument is the standing epoch the claim returned,
/// and the token argument is the issued token, and the ttl
/// argument is the added lease length in seconds, and the now
/// argument is the current instant as ISO 8601 text.
#[pyfunction]
#[allow(clippy::too_many_arguments)]
fn renew(
    py: Python<'_>,
    registry: PathBuf,
    item: &str,
    holder: &str,
    token: &str,
    epoch: &Bound<'_, PyAny>,
    ttl: &Bound<'_, PyAny>,
    now: &str,
) -> PyResult<Py<PyDict>> {
    let epoch = epoch_argument(epoch)?;
    let ttl = ttl_argument(item, holder, ttl)?;
    let now = instant_argument(now)?;
    let registry = Registry::new(registry);
    let expires = py
        .detach(|| registry.renew(item, holder, token, epoch, ttl, &now))
        .map_err(to_py_error)?;
    let dict = PyDict::new(py);
    dict.set_item("expires_at", dibs_core::format_instant(&expires))?;
    Ok(dict.unbind())
}

/// End the standing lease early and return the item to open.
///
/// The epoch argument is the standing epoch the claim returned,
/// and the token argument is the issued token, and the now
/// argument is the current instant as ISO 8601 text.
#[pyfunction]
fn release(
    py: Python<'_>,
    registry: PathBuf,
    item: &str,
    holder: &str,
    token: &str,
    epoch: &Bound<'_, PyAny>,
    now: &str,
) -> PyResult<()> {
    let epoch = epoch_argument(epoch)?;
    let now = instant_argument(now)?;
    let registry = Registry::new(registry);
    py.detach(|| registry.release(item, holder, token, epoch, &now))
        .map_err(to_py_error)
}

/// Mark the item done under its standing lease. Completion is
/// terminal, and the item accepts no claim afterwards.
///
/// The epoch argument is the standing epoch the claim returned,
/// and the token argument is the issued token, and the now
/// argument is the current instant as ISO 8601 text.
#[pyfunction]
fn complete(
    py: Python<'_>,
    registry: PathBuf,
    item: &str,
    holder: &str,
    token: &str,
    epoch: &Bound<'_, PyAny>,
    now: &str,
) -> PyResult<()> {
    let epoch = epoch_argument(epoch)?;
    let now = instant_argument(now)?;
    let registry = Registry::new(registry);
    py.detach(|| registry.complete(item, holder, token, epoch, &now))
        .map_err(to_py_error)
}

/// Return the open items in offer order as plain dictionaries,
/// with each lease state evaluated at the instant the now
/// argument names, as ISO 8601 text.
#[pyfunction]
fn board(py: Python<'_>, registry: PathBuf, now: &str) -> PyResult<Vec<Py<PyDict>>> {
    let now = instant_argument(now)?;
    let registry = Registry::new(registry);
    let rows = py.detach(|| registry.board(&now)).map_err(to_py_error)?;
    rows.iter().map(|row| board_dict(py, row)).collect()
}

/// Return the recorded events in order as plain dictionaries,
/// or only the events of the one item the item argument names.
///
/// History judges no lease and takes no instant, here as in the
/// library.
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
    m.add_function(wrap_pyfunction!(claim, m)?)?;
    m.add_function(wrap_pyfunction!(renew, m)?)?;
    m.add_function(wrap_pyfunction!(release, m)?)?;
    m.add_function(wrap_pyfunction!(complete, m)?)?;
    m.add_function(wrap_pyfunction!(board, m)?)?;
    m.add_function(wrap_pyfunction!(history, m)?)?;
    Ok(())
}
