use dogmos_protocol::{
	read_frame_into, write_frame, OperationKind, ProtocolHeader, TransportError,
};
use std::io::Cursor;

#[test]
fn framed_transport_round_trips_without_owning_the_payload() {
	let payload = b"dogmos-echo";
	let header = ProtocolHeader::request(
		OperationKind::Echo,
		91,
		4,
		0x1234_5678_90ab_cdef,
		payload.len() as u32,
		55_000,
	);
	let mut bytes = Vec::new();
	write_frame(&mut bytes, header, payload).unwrap();

	let mut storage = [0_u8; 32];
	let (decoded_header, payload_len) =
		read_frame_into(&mut Cursor::new(bytes), &mut storage).unwrap();
	assert_eq!(decoded_header, header);
	assert_eq!(&storage[..payload_len], payload);
}

#[test]
fn framed_transport_rejects_payload_mismatch_before_writing() {
	let header = ProtocolHeader::request(OperationKind::Echo, 1, 1, 1, 4, 1);
	let mut bytes = Vec::new();
	assert!(matches!(
		write_frame(&mut bytes, header, b"bad"),
		Err(TransportError::PayloadLengthMismatch {
			header: 4,
			actual: 3,
		})
	));
	assert!(bytes.is_empty());
}

#[test]
fn framed_transport_rejects_payload_larger_than_caller_buffer() {
	let header = ProtocolHeader::request(OperationKind::Echo, 1, 1, 1, 5, 1);
	let mut bytes = header.encode().to_vec();
	bytes.extend_from_slice(b"12345");
	let mut storage = [0_u8; 4];
	assert!(matches!(
		read_frame_into(&mut Cursor::new(bytes), &mut storage),
		Err(TransportError::BufferTooSmall {
			required: 5,
			available: 4,
		})
	));
}
