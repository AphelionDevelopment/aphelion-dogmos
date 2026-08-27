use dogmos_protocol::{
	BuildIdentity, CapacityLimits, HandshakePayload, OperationKind, ProtocolHeader, WireHandle,
	DOGMOS_ABI_VERSION, DOGMOS_PROTOCOL_VERSION,
};

#[test]
fn golden_header_bytes_are_architecture_independent() {
	let header = ProtocolHeader::request(
		OperationKind::Transfer,
		0x0102_0304_0506_0708,
		0x1122_3344,
		0x1020_3040_5060_7080,
		24,
		0x8877_6655_4433_2211,
	);
	let expected = [
		0x44, 0x47, 0x4d, 0x53, 0x04, 0x00, 0x30, 0x00, 0x0c, 0x00, 0x00, 0x00, 0x18, 0x00, 0x00,
		0x00, 0x08, 0x07, 0x06, 0x05, 0x04, 0x03, 0x02, 0x01, 0x44, 0x33, 0x22, 0x11, 0x00, 0x00,
		0x00, 0x00, 0x80, 0x70, 0x60, 0x50, 0x40, 0x30, 0x20, 0x10, 0x11, 0x22, 0x33, 0x44, 0x55,
		0x66, 0x77, 0x88,
	];
	assert_eq!(header.encode(), expected);
	assert_eq!(ProtocolHeader::decode(&expected).unwrap(), header);
}

#[test]
fn golden_handle_bytes_are_architecture_independent() {
	let handle = WireHandle {
		slot: 0x1122_3344,
		generation: 0xaabb_ccdd,
	};
	let expected = [0x44, 0x33, 0x22, 0x11, 0xdd, 0xcc, 0xbb, 0xaa];
	assert_eq!(handle.encode(), expected);
	assert_eq!(WireHandle::decode(&expected).unwrap(), handle);
}

#[test]
fn golden_handshake_bytes_are_architecture_independent() {
	let handshake = HandshakePayload {
		auth_token: [0xa5; 32],
		identity: BuildIdentity {
			abi_version: DOGMOS_ABI_VERSION,
			protocol_version: DOGMOS_PROTOCOL_VERSION,
			source_revision: [0x11; 20],
			feature_fingerprint: [0x22; 32],
			executable_digest: [0x33; 32],
		},
		capacities: CapacityLimits {
			max_control_payload: 0x0008_0304,
			max_batch_operations: 0x1112_1314,
			max_callback_events: 0x0002_2324,
			reserved: 0,
			max_world_bytes: 0x3132_3334_3536_3738,
		},
		process_id: 0x4142_4344,
		world_generation: 0x5152_5354,
		world_nonce: 0x6162_6364_6566_6768,
	};
	let bytes = handshake.encode();
	assert_eq!(&bytes[0..32], &[0xa5; 32]);
	assert_eq!(&bytes[32..34], &DOGMOS_ABI_VERSION.to_le_bytes());
	assert_eq!(&bytes[34..36], &DOGMOS_PROTOCOL_VERSION.to_le_bytes());
	assert_eq!(&bytes[36..56], &[0x11; 20]);
	assert_eq!(&bytes[56..88], &[0x22; 32]);
	assert_eq!(&bytes[88..120], &[0x33; 32]);
	assert_eq!(&bytes[120..124], &[0x04, 0x03, 0x08, 0x00]);
	assert_eq!(&bytes[124..128], &[0x14, 0x13, 0x12, 0x11]);
	assert_eq!(&bytes[128..132], &[0x24, 0x23, 0x02, 0x00]);
	assert_eq!(&bytes[132..136], &[0; 4]);
	assert_eq!(
		&bytes[136..144],
		&[0x38, 0x37, 0x36, 0x35, 0x34, 0x33, 0x32, 0x31]
	);
	assert_eq!(&bytes[144..148], &[0x44, 0x43, 0x42, 0x41]);
	assert_eq!(&bytes[148..152], &[0x54, 0x53, 0x52, 0x51]);
	assert_eq!(
		&bytes[152..160],
		&[0x68, 0x67, 0x66, 0x65, 0x64, 0x63, 0x62, 0x61]
	);
	assert_eq!(HandshakePayload::decode(&bytes).unwrap(), handshake);
}
