use super::{
	normalize_process_metrics, seconds_and_microseconds_to_milliseconds, CurrentProcessMetrics,
};
use std::{fs, mem};

pub(super) fn sample_current_process() -> CurrentProcessMetrics {
	let status = fs::read_to_string("/proc/self/status").ok();
	let virtual_bytes = status
		.as_deref()
		.and_then(|contents| status_kib_value(contents, "VmSize:"));
	let working_set_bytes = status
		.as_deref()
		.and_then(|contents| status_kib_value(contents, "VmRSS:"));
	normalize_process_metrics(None, virtual_bytes, working_set_bytes, process_cpu_time())
}

fn status_kib_value(contents: &str, field: &str) -> Option<u64> {
	let line = contents.lines().find(|line| line.starts_with(field))?;
	let mut parts = line[field.len()..].split_whitespace();
	let kibibytes = parts.next()?.parse::<u64>().ok()?;
	if parts.next()? != "kB" || parts.next().is_some() {
		return None;
	}
	kibibytes.checked_mul(1024)
}

fn process_cpu_time() -> Option<u64> {
	// SAFETY: an all-zero `rusage` is a valid output buffer for `getrusage`, which initializes it on
	// success. The pointer remains live for the duration of the call.
	let mut usage: libc::rusage = unsafe { mem::zeroed() };
	// SAFETY: `usage` is a live `rusage` output buffer and `RUSAGE_SELF` requests only this process.
	if unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut usage) } != 0 {
		return None;
	}
	let user = seconds_and_microseconds_to_milliseconds(
		usage.ru_utime.tv_sec as i64,
		usage.ru_utime.tv_usec as i64,
	)?;
	let system = seconds_and_microseconds_to_milliseconds(
		usage.ru_stime.tv_sec as i64,
		usage.ru_stime.tv_usec as i64,
	)?;
	Some(user.saturating_add(system))
}

#[cfg(test)]
mod tests {
	use super::status_kib_value;

	#[test]
	fn proc_status_parser_requires_exact_field_units() {
		let status = "Name:\tdogmosd\nVmSize:\t123 kB\nVmRSS:\t45 kB\n";

		assert_eq!(status_kib_value(status, "VmSize:"), Some(125_952));
		assert_eq!(status_kib_value(status, "VmRSS:"), Some(46_080));
		assert_eq!(status_kib_value("VmRSS: 45 MB\n", "VmRSS:"), None);
	}
}
