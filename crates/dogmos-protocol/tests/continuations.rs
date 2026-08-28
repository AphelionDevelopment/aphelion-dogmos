use dogmos_protocol::{
	decode_continuation_adjust_multiple_request, encode_continuation_adjust_multiple_request,
	ContinuationCommandRequest, ContinuationResumeRequest, ContinuationToken, MixtureAdjustment,
	MixtureCommandRequest, OperationKind, ProtocolError, ScalarValue, ServiceErrorCode, WireHandle,
	CONTINUATION_COMMAND_REQUEST_LEN, CONTINUATION_RESUME_REQUEST_LEN, CONTINUATION_TOKEN_LEN,
};

fn token() -> ContinuationToken {
	ContinuationToken {
		world_generation: 7,
		id: 0x0102_0304_0506_0708,
		deadline_ticks: 0x1112_1314_1516_1718,
	}
}

#[test]
fn continuation_service_errors_have_stable_codes() {
	for (code, value) in [
		(ServiceErrorCode::UnknownContinuation, 16_u32),
		(ServiceErrorCode::ContinuationExpired, 17),
		(ServiceErrorCode::ContinuationCapacityExceeded, 18),
		(ServiceErrorCode::ContinuationWorldMismatch, 19),
		(ServiceErrorCode::ContinuationTokenMismatch, 20),
	] {
		assert_eq!(code.encode(), value.to_le_bytes());
		assert_eq!(ServiceErrorCode::decode(&value.to_le_bytes()), Ok(code));
	}
}

#[test]
fn continuation_operation_ids_and_token_layout_are_stable() {
	assert_eq!(OperationKind::ContinuationCommand as u16, 32);
	assert_eq!(OperationKind::ContinuationAdjustMultiple as u16, 33);
	assert_eq!(OperationKind::ContinuationResume as u16, 34);
	assert_eq!(OperationKind::ContinuationCancel as u16, 35);
	assert_eq!(CONTINUATION_TOKEN_LEN, 24);

	let token = token();
	let bytes = token.encode().unwrap();
	assert_eq!(&bytes[0..4], &token.world_generation.to_le_bytes());
	assert_eq!(&bytes[4..8], &[0; 4]);
	assert_eq!(&bytes[8..16], &token.id.to_le_bytes());
	assert_eq!(&bytes[16..24], &token.deadline_ticks.to_le_bytes());
	assert_eq!(ContinuationToken::decode(&bytes), Ok(token));
}

#[test]
fn continuation_token_rejects_reserved_and_zero_identity_fields() {
	let mut bytes = token().encode().unwrap();
	bytes[4] = 1;
	assert_eq!(
		ContinuationToken::decode(&bytes),
		Err(ProtocolError::ReservedContinuationField(1))
	);

	let mut zero_id = token().encode().unwrap();
	zero_id[8..16].fill(0);
	assert_eq!(
		ContinuationToken::decode(&zero_id),
		Err(ProtocolError::InvalidContinuationId)
	);

	let mut zero_deadline = token().encode().unwrap();
	zero_deadline[16..24].fill(0);
	assert_eq!(
		ContinuationToken::decode(&zero_deadline),
		Err(ProtocolError::InvalidContinuationDeadline)
	);
}

#[test]
fn continuation_resume_carries_validated_dm_reaction_flags() {
	let request = ContinuationResumeRequest {
		token: token(),
		reaction_result: 5,
	};
	let bytes = request.encode().unwrap();
	assert_eq!(bytes.len(), CONTINUATION_RESUME_REQUEST_LEN);
	assert_eq!(&bytes[..CONTINUATION_TOKEN_LEN], &token().encode().unwrap());
	assert_eq!(&bytes[24..28], &5_u32.to_le_bytes());
	assert_eq!(&bytes[28..32], &[0; 4]);
	assert_eq!(ContinuationResumeRequest::decode(&bytes), Ok(request));

	let mut invalid = bytes;
	invalid[24..28].copy_from_slice(&8_u32.to_le_bytes());
	assert_eq!(
		ContinuationResumeRequest::decode(&invalid),
		Err(ProtocolError::InvalidReactionFlags(8))
	);
}

#[test]
fn nested_fixed_command_carries_the_exact_continuation_token() {
	let request = ContinuationCommandRequest {
		token: token(),
		command: MixtureCommandRequest::SetMoles {
			handle: WireHandle {
				slot: 3,
				generation: 4,
			},
			gas_id: 5,
			amount: ScalarValue(6.5),
		},
	};
	let bytes = request.encode().unwrap();
	assert_eq!(bytes.len(), CONTINUATION_COMMAND_REQUEST_LEN);
	assert_eq!(&bytes[..CONTINUATION_TOKEN_LEN], &token().encode().unwrap());
	assert_eq!(ContinuationCommandRequest::decode(&bytes), Ok(request));
}

#[test]
fn nested_adjust_multiple_is_bounded_and_preserves_order() {
	let handle = WireHandle {
		slot: 3,
		generation: 4,
	};
	let adjustments = [
		MixtureAdjustment {
			gas_id: 1,
			delta: ScalarValue(2.0),
		},
		MixtureAdjustment {
			gas_id: 1,
			delta: ScalarValue(-0.5),
		},
	];
	let mut bytes = Vec::new();
	encode_continuation_adjust_multiple_request(token(), handle, &adjustments, &mut bytes).unwrap();
	assert_eq!(
		decode_continuation_adjust_multiple_request(&bytes),
		Ok((token(), handle, adjustments.to_vec()))
	);

	bytes.push(0);
	assert!(matches!(
		decode_continuation_adjust_multiple_request(&bytes),
		Err(ProtocolError::InvalidPayloadLength { .. })
	));
}
