use dogmos_protocol::{
	OperationKind, ServiceTelemetry, CALLBACK_EVENT_KIND_COUNT, SERVICE_TELEMETRY_LEN,
};

#[test]
fn service_telemetry_operation_and_layout_are_stable() {
	assert_eq!(OperationKind::ServiceTelemetry as u16, 36);
	assert_eq!(CALLBACK_EVENT_KIND_COUNT, 7);
	assert_eq!(SERVICE_TELEMETRY_LEN, 248);

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
		callback_enqueued_by_kind: [14, 15, 16, 17, 18, 19, 20],
		callback_drained_by_kind: [21, 22, 23, 24, 25, 26, 27],
		callback_rejected_by_kind: [28, 29, 30, 31, 32, 33, 34],
	};
	let encoded = telemetry.encode();

	assert_eq!(&encoded[0..4], &1_u32.to_le_bytes());
	assert_eq!(&encoded[20..24], &6_u32.to_le_bytes());
	assert_eq!(&encoded[24..32], &7_u64.to_le_bytes());
	assert_eq!(&encoded[72..80], &13_u64.to_le_bytes());
	assert_eq!(&encoded[80..88], &14_u64.to_le_bytes());
	assert_eq!(&encoded[136..144], &21_u64.to_le_bytes());
	assert_eq!(&encoded[192..200], &28_u64.to_le_bytes());
	assert_eq!(&encoded[240..248], &34_u64.to_le_bytes());
	assert_eq!(ServiceTelemetry::decode(&encoded).unwrap(), telemetry);
}

#[test]
fn service_telemetry_decoder_requires_the_exact_fixed_width() {
	assert!(ServiceTelemetry::decode(&[0_u8; SERVICE_TELEMETRY_LEN - 1]).is_err());
	assert!(ServiceTelemetry::decode(&[0_u8; SERVICE_TELEMETRY_LEN + 1]).is_err());
}
