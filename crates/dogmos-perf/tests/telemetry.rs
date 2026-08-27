use dogmos_perf::{
	allocation_floor_bytes, classify_binding, snapshot_to_json, snapshot_to_json_with_diagnostics,
	AllocationFloorLayout, AllocatorProcessDiagnostics, OperationClass, RuntimeMetric, Telemetry,
};
use std::time::Duration;

#[test]
fn exact_bindings_receive_independent_fixed_slots() {
	let telemetry = Telemetry::new();
	let read = telemetry.begin(
		"/datum/gas_mixture/proc/return_pressure",
		1,
		OperationClass::ScalarRead,
	);
	read.finish(1);
	let write = telemetry.begin(
		"/datum/gas_mixture/proc/__set_moles",
		3,
		OperationClass::ScalarWrite,
	);
	write.finish(1);

	let snapshot = telemetry.snapshot(16);
	assert_eq!(snapshot.operations.len(), 2);
	let read = snapshot
		.operations
		.iter()
		.find(|operation| operation.binding.ends_with("return_pressure"))
		.unwrap();
	assert_eq!(read.calls, 1);
	assert_eq!(read.request_values, 1);
	assert_eq!(read.response_values, 1);
	assert_eq!(read.class, OperationClass::ScalarRead);
	let write = snapshot
		.operations
		.iter()
		.find(|operation| operation.binding.ends_with("__set_moles"))
		.unwrap();
	assert_eq!(write.class, OperationClass::ScalarWrite);
	assert_ne!(read.slot, write.slot);
}

#[test]
fn binding_classes_expose_read_write_and_batch_barriers() {
	assert_eq!(
		classify_binding("/datum/gas_mixture/proc/return_pressure"),
		OperationClass::ScalarRead
	);
	assert_eq!(
		classify_binding("/datum/gas_mixture/proc/__set_moles"),
		OperationClass::ScalarWrite
	);
	assert_eq!(
		classify_binding("/datum/gas_mixture/proc/transfer_to"),
		OperationClass::MixtureTransaction
	);
	assert_eq!(
		classify_binding("/datum/controller/subsystem/air/proc/process_turfs_auxtools"),
		OperationClass::SimulationStage
	);
}

#[test]
fn snapshot_json_keeps_process_roles_separate() {
	let telemetry = Telemetry::new();
	telemetry
		.begin("/proc/read", 1, OperationClass::ScalarRead)
		.finish(1);
	let json = snapshot_to_json(&telemetry.snapshot(8), 123, 0, 456, 0);
	assert!(json.contains("\"dreamdaemon_private_bytes\":123"));
	assert!(json.contains("\"server_private_bytes\":456"));
	assert!(json.contains("\"server_memory_is_separate\":true"));
	assert!(!json.contains("combined_memory"));
}

#[test]
fn detailed_capture_records_latency_and_bounded_order() {
	let telemetry = Telemetry::new();
	telemetry.set_detailed(true);
	for index in 0..80 {
		let call = telemetry.begin(
			if index % 2 == 0 {
				"/proc/read"
			} else {
				"/proc/write"
			},
			1,
			if index % 2 == 0 {
				OperationClass::ScalarRead
			} else {
				OperationClass::ScalarWrite
			},
		);
		call.finish_with_duration(1, Duration::from_nanos(1_u64 << (index % 12)));
	}
	let snapshot = telemetry.snapshot(32);
	assert_eq!(snapshot.sequence.len(), 32);
	assert_eq!(snapshot.sequence_dropped, 48);
	assert!(
		snapshot
			.operations
			.iter()
			.flat_map(|operation| operation.latency_buckets)
			.sum::<u64>()
			>= 80
	);
	assert_eq!(snapshot.class_transitions[1][0], 39);
	assert_eq!(snapshot.class_transitions[0][1], 40);
}

#[test]
fn runtime_metrics_support_counts_gauges_and_high_water_marks() {
	let telemetry = Telemetry::new();
	telemetry.increment_metric(RuntimeMetric::FdmNodesScanned, 10);
	telemetry.increment_metric(RuntimeMetric::FdmNodesScanned, 5);
	telemetry.set_metric(RuntimeMetric::GasGraphNodes, 40);
	telemetry.set_metric(RuntimeMetric::GasGraphNodes, 12);
	telemetry.update_high_water(RuntimeMetric::MixtureSlotHighWater, 20);
	telemetry.update_high_water(RuntimeMetric::MixtureSlotHighWater, 8);
	let snapshot = telemetry.snapshot(0);
	assert_eq!(snapshot.metric(RuntimeMetric::FdmNodesScanned), 15);
	assert_eq!(snapshot.metric(RuntimeMetric::GasGraphNodes), 12);
	assert_eq!(snapshot.metric(RuntimeMetric::MixtureSlotHighWater), 20);
}

#[test]
fn audited_allocation_floor_is_reproducible() {
	let layout = AllocationFloorLayout::audited_i686();
	assert_eq!(layout.mixture_bytes, 60);
	assert_eq!(layout.mixture_lock_bytes, 64);
	assert_eq!(layout.turf_mixture_bytes, 32);
	assert_eq!(layout.thermal_info_bytes, 28);
	assert_eq!(
		allocation_floor_bytes(layout, 240_000, 650_250, 1_300_500).unwrap(),
		132_405_000,
	);
}

#[test]
fn diagnostic_json_identifies_layout_and_allocator_process_semantics() {
	let telemetry = Telemetry::new();
	let diagnostics = AllocatorProcessDiagnostics {
		layout: AllocationFloorLayout::audited_i686(),
		allocation_floor_bytes: 132_405_000,
		elapsed_milliseconds: 100,
		user_milliseconds: 20,
		system_milliseconds: 5,
		current_rss_bytes: 111,
		peak_rss_bytes: 222,
		current_commit_bytes: 333,
		peak_commit_bytes: 444,
		page_faults: 7,
	};
	let json = snapshot_to_json_with_diagnostics(&telemetry.snapshot(0), 0, 0, 0, 0, diagnostics);
	assert!(json.contains("\"allocation_floor_bytes\":132405000"));
	assert!(json.contains("\"mixture_bytes\":60"));
	assert!(json.contains("\"allocator_process_current_commit_bytes\":333"));
	assert!(json.contains("\"allocator_process_scope\":\"current_process_not_server\""));
	assert!(!json.contains("},\"allocation_layout\""));
	assert!(!json.contains("combined_memory"));
}
