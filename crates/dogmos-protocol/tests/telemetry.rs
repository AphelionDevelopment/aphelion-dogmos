use dogmos_protocol::{
	OperationKind, ProtocolError, ServiceTelemetry, CALLBACK_EVENT_KIND_COUNT,
	DOGMOS_PROTOCOL_VERSION, SERVICE_PROCESS_ALL_AVAILABLE, SERVICE_PROCESS_CPU_AVAILABLE,
	SERVICE_PROCESS_RSS_AVAILABLE, SERVICE_TELEMETRY_LEN,
};

#[test]
fn service_telemetry_operation_and_layout_are_stable() {
	assert_eq!(OperationKind::ServiceTelemetry as u16, 36);
	assert_eq!(CALLBACK_EVENT_KIND_COUNT, 8);
	assert_eq!(DOGMOS_PROTOCOL_VERSION, 13);
	assert_eq!(SERVICE_TELEMETRY_LEN, 368);
	assert_eq!(SERVICE_PROCESS_RSS_AVAILABLE, 1);
	assert_eq!(SERVICE_PROCESS_CPU_AVAILABLE, 2);
	assert_eq!(SERVICE_PROCESS_ALL_AVAILABLE, 3);

	let telemetry = ServiceTelemetry {
		callback_depth: 1,
		callback_capacity: 2,
		callback_high_water: 3,
		continuation_depth: 4,
		continuation_capacity: 5,
		continuation_high_water: 6,
		oldest_callback_age_ticks: 7,
		callback_enqueued: 8,
		callback_drained: 9,
		callback_rejected: 10,
		continuation_timeouts: 11,
		request_timeouts: 12,
		protocol_errors: 13,
		callback_enqueued_by_kind: [14, 15, 16, 17, 18, 19, 20, 21],
		callback_drained_by_kind: [22, 23, 24, 25, 26, 27, 28, 29],
		callback_rejected_by_kind: [30, 31, 32, 33, 34, 35, 36, 37],
		service_process_available_flags: SERVICE_PROCESS_ALL_AVAILABLE,
		service_rss_bytes: 0x0123_4567_89ab_cdef,
		service_cpu_total_milliseconds: 0xfedc_ba98_7654_3210,
		general_callback_depth: 38,
		reaction_callback_depth: 39,
		reaction_transaction_depth: 40,
		reaction_transaction_high_water: 41,
		frontier_count: 42,
		stage_kind: 5,
		frontier_upload_bytes: 43,
		stage_epoch: 44,
		stage_cursor: 45,
		stage_remaining: 46,
		topology_revision: 47,
		reusable_workset_bytes: 48,
		packed_topology_bytes: 49,
	};
	let encoded = telemetry.encode();

	assert_eq!(&encoded[0..4], &1_u32.to_le_bytes());
	assert_eq!(&encoded[20..24], &6_u32.to_le_bytes());
	assert_eq!(&encoded[24..32], &7_u64.to_le_bytes());
	assert_eq!(&encoded[72..80], &13_u64.to_le_bytes());
	assert_eq!(&encoded[80..88], &14_u64.to_le_bytes());
	assert_eq!(&encoded[144..152], &22_u64.to_le_bytes());
	assert_eq!(&encoded[208..216], &30_u64.to_le_bytes());
	assert_eq!(&encoded[264..272], &37_u64.to_le_bytes());
	assert_eq!(
		&encoded[272..276],
		&SERVICE_PROCESS_ALL_AVAILABLE.to_le_bytes()
	);
	assert_eq!(&encoded[276..280], &[0_u8; 4]);
	assert_eq!(&encoded[280..288], &0x0123_4567_89ab_cdef_u64.to_le_bytes());
	assert_eq!(&encoded[288..296], &0xfedc_ba98_7654_3210_u64.to_le_bytes());
	assert_eq!(&encoded[296..300], &38_u32.to_le_bytes());
	assert_eq!(&encoded[360..368], &49_u64.to_le_bytes());
	assert_eq!(ServiceTelemetry::decode(&encoded).unwrap(), telemetry);
}

#[test]
fn service_telemetry_decoder_requires_the_exact_fixed_width() {
	assert!(ServiceTelemetry::decode(&[0_u8; SERVICE_TELEMETRY_LEN - 1]).is_err());
	assert!(ServiceTelemetry::decode(&[0_u8; SERVICE_TELEMETRY_LEN + 1]).is_err());
}

#[test]
fn service_telemetry_rejects_unknown_process_flags() {
	let mut encoded = [0_u8; SERVICE_TELEMETRY_LEN];
	encoded[272..276].copy_from_slice(&4_u32.to_le_bytes());

	assert_eq!(
		ServiceTelemetry::decode(&encoded),
		Err(ProtocolError::UnknownServiceProcessFlags(4))
	);
}

#[test]
fn service_telemetry_rejects_nonzero_reserved_process_field() {
	let mut encoded = [0_u8; SERVICE_TELEMETRY_LEN];
	encoded[276..280].copy_from_slice(&1_u32.to_le_bytes());

	assert_eq!(
		ServiceTelemetry::decode(&encoded),
		Err(ProtocolError::ReservedServiceTelemetryField(1))
	);
}

#[test]
fn service_telemetry_rejects_nonzero_unavailable_process_metrics() {
	for offset in [280, 288] {
		let mut encoded = [0_u8; SERVICE_TELEMETRY_LEN];
		encoded[offset..offset + 8].copy_from_slice(&1_u64.to_le_bytes());

		assert_eq!(
			ServiceTelemetry::decode(&encoded),
			Err(ProtocolError::NonZeroUnavailableServiceProcessMetric)
		);
	}
}
