use eyre::Result;
use std::{
	any::Any,
	panic::{catch_unwind, AssertUnwindSafe},
};

pub(crate) fn guard_with_arity<T>(
	binding: &'static str,
	_request_values: u64,
	call: impl FnOnce() -> Result<T>,
) -> Result<T> {
	match catch_unwind(AssertUnwindSafe(call)) {
		Ok(result) => result,
		Err(payload) => Err(eyre::eyre!(
			"Dogmos shim panic in {binding}: {}",
			panic_payload_message(payload.as_ref())
		)),
	}
}

fn panic_payload_message(payload: &(dyn Any + Send)) -> &str {
	if let Some(message) = payload.downcast_ref::<String>() {
		message
	} else if let Some(message) = payload.downcast_ref::<&str>() {
		message
	} else {
		"<non-string panic payload>"
	}
}
