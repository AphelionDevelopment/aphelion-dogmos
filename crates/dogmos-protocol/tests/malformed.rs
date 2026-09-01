use dogmos_protocol::{
	decode_frame, OperationKind, ProtocolError, ProtocolHeader, ScalarValue,
	DOGMOS_PROTOCOL_VERSION, FLAG_ERROR, FLAG_RESPONSE, MAX_CONTROL_PAYLOAD,
};

fn request() -> ProtocolHeader {
	ProtocolHeader::request(OperationKind::ScalarGet, 41, 7, 0x1234_5678, 4, 9_000_000)
}

#[test]
fn decoder_rejects_truncated_and_oversized_frames() {
	assert_eq!(
		ProtocolHeader::decode(&request().encode()[..47]),
		Err(ProtocolError::TruncatedHeader { actual: 47 })
	);

	let mut oversized = request().encode();
	oversized[12..16].copy_from_slice(&(MAX_CONTROL_PAYLOAD + 1).to_le_bytes());
	assert_eq!(
		ProtocolHeader::decode(&oversized),
		Err(ProtocolError::PayloadTooLarge {
			actual: MAX_CONTROL_PAYLOAD + 1,
			maximum: MAX_CONTROL_PAYLOAD,
		})
	);
}

#[test]
fn decoder_rejects_unknown_kind_and_reserved_bits() {
	let mut unknown_kind = request().encode();
	unknown_kind[8..10].copy_from_slice(&0xffff_u16.to_le_bytes());
	assert_eq!(
		ProtocolHeader::decode(&unknown_kind),
		Err(ProtocolError::UnknownOperationKind(0xffff))
	);

	let mut reserved_flags = request().encode();
	reserved_flags[10..12].copy_from_slice(&0x8000_u16.to_le_bytes());
	assert_eq!(
		ProtocolHeader::decode(&reserved_flags),
		Err(ProtocolError::UnknownFlags(0x8000))
	);
}

#[test]
fn decoder_rejects_invalid_identity_and_header_fields() {
	let mut invalid_magic = request().encode();
	invalid_magic[0] ^= 0xff;
	assert!(matches!(
		ProtocolHeader::decode(&invalid_magic),
		Err(ProtocolError::InvalidMagic(_))
	));

	let mut invalid_version = request().encode();
	let invalid_protocol_version = DOGMOS_PROTOCOL_VERSION + 1;
	invalid_version[4..6].copy_from_slice(&invalid_protocol_version.to_le_bytes());
	assert_eq!(
		ProtocolHeader::decode(&invalid_version),
		Err(ProtocolError::UnsupportedProtocolVersion(
			invalid_protocol_version
		))
	);

	let mut invalid_header_len = request().encode();
	invalid_header_len[6..8].copy_from_slice(&47_u16.to_le_bytes());
	assert_eq!(
		ProtocolHeader::decode(&invalid_header_len),
		Err(ProtocolError::InvalidHeaderLength(47))
	);
}

#[test]
fn full_frame_decoder_requires_exact_payload_length() {
	let header = request().encode();
	assert_eq!(
		decode_frame(&header),
		Err(ProtocolError::TruncatedPayload {
			expected_frame_len: 52,
			actual_frame_len: 48,
		})
	);

	let mut trailing = ProtocolHeader::request(OperationKind::Echo, 1, 1, 1, 0, 1)
		.encode()
		.to_vec();
	trailing.push(0xff);
	assert_eq!(
		decode_frame(&trailing),
		Err(ProtocolError::TrailingBytes {
			expected_frame_len: 48,
			actual_frame_len: 49,
		})
	);
}

#[test]
fn request_decoder_rejects_response_only_error_flag() {
	let mut invalid = request().encode();
	invalid[10..12].copy_from_slice(&FLAG_ERROR.to_le_bytes());
	assert_eq!(
		ProtocolHeader::decode(&invalid),
		Err(ProtocolError::ErrorFlagWithoutResponse)
	);
}

#[test]
fn response_validation_rejects_stale_world_and_request_mismatch() {
	let expected = request();
	let mut stale = expected.response();
	stale.world_generation += 1;
	assert_eq!(
		stale.validate_response_to(&expected),
		Err(ProtocolError::WorldGenerationMismatch {
			expected: 7,
			actual: 8,
		})
	);

	let mut mismatched = expected.response();
	mismatched.request_id += 1;
	assert_eq!(
		mismatched.validate_response_to(&expected),
		Err(ProtocolError::RequestIdMismatch {
			expected: 41,
			actual: 42,
		})
	);
	assert_ne!(mismatched.flags & FLAG_RESPONSE, 0);
}

#[test]
fn response_validation_rejects_nonce_and_operation_mismatch() {
	let expected = request();
	let mut wrong_nonce = expected.response();
	wrong_nonce.world_nonce += 1;
	assert_eq!(
		wrong_nonce.validate_response_to(&expected),
		Err(ProtocolError::WorldNonceMismatch)
	);

	let mut wrong_operation = expected.response();
	wrong_operation.operation_kind = OperationKind::ScalarSet as u16;
	assert_eq!(
		wrong_operation.validate_response_to(&expected),
		Err(ProtocolError::OperationKindMismatch {
			expected: OperationKind::ScalarGet as u16,
			actual: OperationKind::ScalarSet as u16,
		})
	);
}

#[test]
fn scalar_decoder_rejects_non_finite_values() {
	assert_eq!(
		ScalarValue::decode(&f64::NAN.to_bits().to_le_bytes()),
		Err(ProtocolError::NonFiniteScalar)
	);
	assert_eq!(
		ScalarValue::decode(&f64::INFINITY.to_bits().to_le_bytes()),
		Err(ProtocolError::NonFiniteScalar)
	);
}

#[test]
fn service_error_codes_are_fixed_width_and_reject_unknown_values() {
	use dogmos_protocol::ServiceErrorCode;

	let errors = [
		ServiceErrorCode::Busy,
		ServiceErrorCode::AuthenticationFailed,
		ServiceErrorCode::InvalidRequest,
		ServiceErrorCode::DeadlineExceeded,
		ServiceErrorCode::Internal,
		ServiceErrorCode::CallbackBackpressure,
		ServiceErrorCode::UnknownHandle,
		ServiceErrorCode::StaleHandle,
		ServiceErrorCode::RevisionMismatch,
		ServiceErrorCode::RevisionExhausted,
		ServiceErrorCode::DuplicateMixtureState,
		ServiceErrorCode::InvalidMixtureState,
		ServiceErrorCode::StateCapacityExceeded,
		ServiceErrorCode::AllocationFailed,
		ServiceErrorCode::InvalidGraph,
		ServiceErrorCode::UnknownContinuation,
		ServiceErrorCode::ContinuationExpired,
		ServiceErrorCode::ContinuationCapacityExceeded,
		ServiceErrorCode::ContinuationWorldMismatch,
		ServiceErrorCode::ContinuationTokenMismatch,
		ServiceErrorCode::FrontierConflict,
		ServiceErrorCode::FrontierIncomplete,
		ServiceErrorCode::StageConflict,
		ServiceErrorCode::MixtureStateUploadConflict,
		ServiceErrorCode::MixtureStateUploadIncomplete,
	];
	for (index, error) in errors.into_iter().enumerate() {
		let expected = (index as u32 + 1).to_le_bytes();
		assert_eq!(error.encode(), expected);
		assert_eq!(ServiceErrorCode::decode(&expected), Ok(error));
	}
	assert_eq!(
		ServiceErrorCode::decode(&99_u32.to_le_bytes()),
		Err(ProtocolError::UnknownServiceErrorCode(99))
	);
	assert_eq!(
		ServiceErrorCode::decode(&[1, 0, 0]),
		Err(ProtocolError::InvalidServiceErrorLength { actual: 3 })
	);
}

#[test]
fn handshake_rejects_zero_required_capacities_and_nonzero_reserved_capacity() {
	use dogmos_protocol::{BuildIdentity, CapacityLimits, HandshakePayload, DOGMOS_ABI_VERSION};

	let valid = HandshakePayload {
		auth_token: [1; 32],
		identity: BuildIdentity {
			abi_version: DOGMOS_ABI_VERSION,
			protocol_version: DOGMOS_PROTOCOL_VERSION,
			source_revision: [2; 20],
			feature_fingerprint: [3; 32],
			executable_digest: [4; 32],
		},
		capacities: CapacityLimits {
			max_control_payload: 65_536,
			max_batch_operations: 512,
			max_callback_events: 1024,
			max_pending_continuations: 64,
			max_frontier_handles: 100_000,
			max_stage_work_items: 4096,
			max_reaction_transactions: 64,
			reserved: 0,
			max_world_bytes: 1 << 30,
		},
		process_id: 1,
		world_generation: 1,
		world_nonce: 1,
	};

	let mut zero_frontier = valid.encode();
	zero_frontier[136..140].fill(0);
	assert_eq!(
		HandshakePayload::decode(&zero_frontier),
		Err(ProtocolError::InvalidCapacityLimit("max_frontier_handles"))
	);

	let mut reserved = valid.encode();
	reserved[148..152].copy_from_slice(&1_u32.to_le_bytes());
	assert_eq!(
		HandshakePayload::decode(&reserved),
		Err(ProtocolError::ReservedCapacityField(1))
	);
}
