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

#[cfg(test)]
mod tests {
	use super::guard_with_arity;

	#[test]
	fn guarded_boundary_contains_panics_with_the_binding_name() {
		let error =
			guard_with_arity::<()>("/proc/dogmos_test", 0, || panic!("hostile panic payload"))
				.unwrap_err();
		let message = error.to_string();
		assert!(message.contains("/proc/dogmos_test"));
		assert!(message.contains("hostile panic payload"));
	}
}
