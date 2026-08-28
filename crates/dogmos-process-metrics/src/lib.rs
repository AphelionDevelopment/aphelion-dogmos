#![deny(unsafe_op_in_unsafe_fn)]

#[cfg(target_os = "linux")]
mod linux;
#[cfg(windows)]
mod windows;

pub const PROCESS_PRIVATE_BYTES_AVAILABLE: u32 = 1 << 0;
pub const PROCESS_VIRTUAL_BYTES_AVAILABLE: u32 = 1 << 1;
pub const PROCESS_WORKING_SET_AVAILABLE: u32 = 1 << 2;
pub const PROCESS_CPU_AVAILABLE: u32 = 1 << 3;
pub const PROCESS_ALL_AVAILABLE: u32 = PROCESS_PRIVATE_BYTES_AVAILABLE
	| PROCESS_VIRTUAL_BYTES_AVAILABLE
	| PROCESS_WORKING_SET_AVAILABLE
	| PROCESS_CPU_AVAILABLE;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct CurrentProcessMetrics {
	pub available_flags: u32,
	pub private_bytes: u64,
	pub virtual_bytes: u64,
	pub working_set_bytes: u64,
	pub cpu_total_milliseconds: u64,
}

fn normalize_process_metrics(
	private_bytes: Option<u64>,
	virtual_bytes: Option<u64>,
	working_set_bytes: Option<u64>,
	cpu_total_milliseconds: Option<u64>,
) -> CurrentProcessMetrics {
	let mut metrics = CurrentProcessMetrics::default();
	if let Some(value) = private_bytes {
		metrics.available_flags |= PROCESS_PRIVATE_BYTES_AVAILABLE;
		metrics.private_bytes = value;
	}
	if let Some(value) = virtual_bytes {
		metrics.available_flags |= PROCESS_VIRTUAL_BYTES_AVAILABLE;
		metrics.virtual_bytes = value;
	}
	if let Some(value) = working_set_bytes {
		metrics.available_flags |= PROCESS_WORKING_SET_AVAILABLE;
		metrics.working_set_bytes = value;
	}
	if let Some(value) = cpu_total_milliseconds {
		metrics.available_flags |= PROCESS_CPU_AVAILABLE;
		metrics.cpu_total_milliseconds = value;
	}
	metrics
}

#[cfg(any(windows, test))]
const fn hundred_nanoseconds_to_milliseconds(value: u64) -> u64 {
	value / 10_000
}

#[cfg(any(target_os = "linux", test))]
fn seconds_and_microseconds_to_milliseconds(seconds: i64, microseconds: i64) -> Option<u64> {
	if seconds < 0 || !(0..1_000_000).contains(&microseconds) {
		return None;
	}
	Some(
		(seconds as u64)
			.saturating_mul(1_000)
			.saturating_add(microseconds as u64 / 1_000),
	)
}

#[cfg(target_os = "linux")]
pub fn sample_current_process() -> CurrentProcessMetrics {
	linux::sample_current_process()
}

#[cfg(windows)]
pub fn sample_current_process() -> CurrentProcessMetrics {
	windows::sample_current_process()
}

#[cfg(not(any(target_os = "linux", windows)))]
pub fn sample_current_process() -> CurrentProcessMetrics {
	CurrentProcessMetrics::default()
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn unavailable_counters_clear_only_their_values_and_flags() {
		let metrics = normalize_process_metrics(Some(11), None, Some(33), None);

		assert_eq!(
			metrics.available_flags,
			PROCESS_PRIVATE_BYTES_AVAILABLE | PROCESS_WORKING_SET_AVAILABLE
		);
		assert_eq!(metrics.private_bytes, 11);
		assert_eq!(metrics.virtual_bytes, 0);
		assert_eq!(metrics.working_set_bytes, 33);
		assert_eq!(metrics.cpu_total_milliseconds, 0);
	}

	#[test]
	fn windows_cpu_ticks_convert_to_milliseconds_without_overflow() {
		assert_eq!(hundred_nanoseconds_to_milliseconds(49_999), 4);
		assert_eq!(
			hundred_nanoseconds_to_milliseconds(u64::MAX),
			u64::MAX / 10_000
		);
	}

	#[test]
	fn linux_cpu_time_validates_and_saturates() {
		assert_eq!(
			seconds_and_microseconds_to_milliseconds(2, 345_678),
			Some(2_345)
		);
		assert_eq!(seconds_and_microseconds_to_milliseconds(-1, 0), None);
		assert_eq!(seconds_and_microseconds_to_milliseconds(0, -1), None);
		assert_eq!(seconds_and_microseconds_to_milliseconds(0, 1_000_000), None);
		assert_eq!(
			seconds_and_microseconds_to_milliseconds(i64::MAX, 999_999),
			Some(u64::MAX)
		);
	}

	#[test]
	fn every_available_counter_has_a_known_flag() {
		let metrics = normalize_process_metrics(Some(1), Some(2), Some(3), Some(4));

		assert_eq!(metrics.available_flags, 15);
	}

	#[test]
	fn current_process_sample_is_canonical() {
		let metrics = sample_current_process();

		assert_eq!(metrics.available_flags & !PROCESS_ALL_AVAILABLE, 0);
		for (flag, value) in [
			(PROCESS_PRIVATE_BYTES_AVAILABLE, metrics.private_bytes),
			(PROCESS_VIRTUAL_BYTES_AVAILABLE, metrics.virtual_bytes),
			(PROCESS_WORKING_SET_AVAILABLE, metrics.working_set_bytes),
			(PROCESS_CPU_AVAILABLE, metrics.cpu_total_milliseconds),
		] {
			if metrics.available_flags & flag == 0 {
				assert_eq!(value, 0);
			}
		}
		if metrics.available_flags & PROCESS_VIRTUAL_BYTES_AVAILABLE != 0 {
			assert!(metrics.virtual_bytes > 0);
		}
		if metrics.available_flags & PROCESS_WORKING_SET_AVAILABLE != 0 {
			assert!(metrics.working_set_bytes > 0);
		}
	}
}
