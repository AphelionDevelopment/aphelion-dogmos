use dogmos_byond::decode_production_mixture_snapshot_batch;
use dogmos_protocol::{
	encode_mixture_snapshot_batch_response, MixtureSnapshot, MixtureSnapshotRecord, ScalarValue,
	WireHandle, MAX_GAS_SLOTS,
};

#[test]
fn malformed_last_snapshot_cannot_return_a_valid_prefix() {
	let record = MixtureSnapshotRecord {
		handle: WireHandle {
			slot: 3,
			generation: 7,
		},
		snapshot: MixtureSnapshot {
			revision: 4,
			gas_count: 1,
			temperature: ScalarValue(300.0),
			volume: ScalarValue(2500.0),
			minimum_heat_capacity: ScalarValue(0.0),
			total_moles: ScalarValue(1.0),
			pressure: ScalarValue(1.0),
			heat_capacity: ScalarValue(20.0),
			immutable: false,
			gases: [ScalarValue(0.0); MAX_GAS_SLOTS],
		},
	};
	let mut bytes = Vec::new();
	encode_mixture_snapshot_batch_response(&[record, record], &mut bytes).unwrap();
	let valid = decode_production_mixture_snapshot_batch(&bytes).unwrap();
	assert_eq!(valid.len(), 2 * (MAX_GAS_SLOTS + 12));
	let last = bytes.len() - 4;
	bytes[last..].copy_from_slice(&f32::NAN.to_le_bytes());
	assert!(decode_production_mixture_snapshot_batch(&bytes).is_err());
	bytes.pop();
	assert!(decode_production_mixture_snapshot_batch(&bytes).is_err());
}
