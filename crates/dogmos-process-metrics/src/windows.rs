use super::{
	hundred_nanoseconds_to_milliseconds, normalize_process_metrics, CurrentProcessMetrics,
};
use std::mem;
use windows_sys::Win32::{
	Foundation::FILETIME,
	System::{
		ProcessStatus::{
			K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX,
		},
		SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX},
		Threading::{GetCurrentProcess, GetProcessTimes},
	},
};

pub(super) fn sample_current_process() -> CurrentProcessMetrics {
	let process = unsafe { GetCurrentProcess() };
	let (private_bytes, working_set_bytes) = process_memory(process);
	let virtual_bytes = process_virtual_memory();
	let cpu_total_milliseconds = process_cpu_time(process);
	normalize_process_metrics(
		private_bytes,
		virtual_bytes,
		working_set_bytes,
		cpu_total_milliseconds,
	)
}

fn process_memory(process: windows_sys::Win32::Foundation::HANDLE) -> (Option<u64>, Option<u64>) {
	let mut counters = PROCESS_MEMORY_COUNTERS_EX {
		cb: mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32,
		..Default::default()
	};
	// SAFETY: `process` is the current-process pseudo-handle and the output pointer and size describe
	// a live `PROCESS_MEMORY_COUNTERS_EX` value for the duration of the call.
	let succeeded = unsafe {
		K32GetProcessMemoryInfo(
			process,
			(&mut counters as *mut PROCESS_MEMORY_COUNTERS_EX).cast::<PROCESS_MEMORY_COUNTERS>(),
			mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32,
		)
	} != 0;
	if !succeeded {
		return (None, None);
	}
	(
		Some(counters.PrivateUsage as u64),
		Some(counters.WorkingSetSize as u64),
	)
}

fn process_virtual_memory() -> Option<u64> {
	let mut status = MEMORYSTATUSEX {
		dwLength: mem::size_of::<MEMORYSTATUSEX>() as u32,
		..Default::default()
	};
	// SAFETY: the output pointer refers to a live `MEMORYSTATUSEX` whose `dwLength` is initialized.
	if unsafe { GlobalMemoryStatusEx(&mut status) } == 0 {
		return None;
	}
	Some(
		status
			.ullTotalVirtual
			.saturating_sub(status.ullAvailVirtual),
	)
}

fn process_cpu_time(process: windows_sys::Win32::Foundation::HANDLE) -> Option<u64> {
	let mut creation = FILETIME::default();
	let mut exit = FILETIME::default();
	let mut kernel = FILETIME::default();
	let mut user = FILETIME::default();
	// SAFETY: `process` is the current-process pseudo-handle and every output pointer targets a live
	// `FILETIME` value for the duration of the call.
	if unsafe { GetProcessTimes(process, &mut creation, &mut exit, &mut kernel, &mut user) } == 0 {
		return None;
	}
	let kernel_ticks = filetime_ticks(kernel);
	let user_ticks = filetime_ticks(user);
	Some(hundred_nanoseconds_to_milliseconds(
		kernel_ticks.saturating_add(user_ticks),
	))
}

const fn filetime_ticks(value: FILETIME) -> u64 {
	(value.dwLowDateTime as u64) | ((value.dwHighDateTime as u64) << 32)
}
