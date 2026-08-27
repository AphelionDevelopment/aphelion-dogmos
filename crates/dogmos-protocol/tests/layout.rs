use dogmos_protocol::{
	BuildIdentity, CapacityLimits, HandshakePayload, ProtocolHeader, WireHandle,
	HANDSHAKE_PAYLOAD_LEN, PROTOCOL_HEADER_LEN,
};
use std::mem::{offset_of, size_of};

#[test]
fn protocol_header_layout_is_identical_on_every_supported_bitness() {
	assert_eq!(size_of::<ProtocolHeader>(), 48);
	assert_eq!(PROTOCOL_HEADER_LEN, 48);
	assert_eq!(offset_of!(ProtocolHeader, magic), 0);
	assert_eq!(offset_of!(ProtocolHeader, protocol_version), 4);
	assert_eq!(offset_of!(ProtocolHeader, header_len), 6);
	assert_eq!(offset_of!(ProtocolHeader, operation_kind), 8);
	assert_eq!(offset_of!(ProtocolHeader, flags), 10);
	assert_eq!(offset_of!(ProtocolHeader, payload_len), 12);
	assert_eq!(offset_of!(ProtocolHeader, request_id), 16);
	assert_eq!(offset_of!(ProtocolHeader, world_generation), 24);
	assert_eq!(offset_of!(ProtocolHeader, reserved), 28);
	assert_eq!(offset_of!(ProtocolHeader, world_nonce), 32);
	assert_eq!(offset_of!(ProtocolHeader, deadline_ns), 40);
}

#[test]
fn public_handle_layout_uses_only_fixed_width_fields() {
	assert_eq!(size_of::<WireHandle>(), 8);
	assert_eq!(offset_of!(WireHandle, slot), 0);
	assert_eq!(offset_of!(WireHandle, generation), 4);
}

#[test]
fn handshake_layout_uses_only_fixed_width_fields() {
	assert_eq!(size_of::<BuildIdentity>(), 88);
	assert_eq!(size_of::<CapacityLimits>(), 24);
	assert_eq!(size_of::<HandshakePayload>(), 160);
	assert_eq!(HANDSHAKE_PAYLOAD_LEN, 160);
}
