use dogmos_protocol::{
	BuildIdentity, CapacityLimits, HandshakePayload, ProtocolError, DOGMOS_ABI_VERSION,
	DOGMOS_PROTOCOL_VERSION, HANDSHAKE_PAYLOAD_LEN, MAX_CALLBACK_EVENTS, MAX_CONTROL_PAYLOAD,
	MAX_PENDING_CONTINUATIONS,
};

fn payload() -> HandshakePayload {
	HandshakePayload {
		auth_token: [7; 32],
		identity: BuildIdentity {
			abi_version: DOGMOS_ABI_VERSION,
			protocol_version: DOGMOS_PROTOCOL_VERSION,
			source_revision: [1; 20],
			feature_fingerprint: [2; 32],
			executable_digest: [3; 32],
		},
		capacities: CapacityLimits {
			max_control_payload: MAX_CONTROL_PAYLOAD,
			max_batch_operations: 4096,
			max_callback_events: 1024,
			max_pending_continuations: 1024,
			max_world_bytes: 8 * 1024 * 1024 * 1024,
		},
		process_id: 1234,
		world_generation: 8,
		world_nonce: 0x1234_5678_90ab_cdef,
	}
}

#[test]
fn handshake_decoder_requires_exact_length() {
	let bytes = payload().encode();
	assert_eq!(
		HandshakePayload::decode(&bytes[..HANDSHAKE_PAYLOAD_LEN - 1]),
		Err(ProtocolError::InvalidHandshakeLength {
			expected: HANDSHAKE_PAYLOAD_LEN as u32,
			actual: (HANDSHAKE_PAYLOAD_LEN - 1) as u32,
		})
	);

	let mut trailing = bytes.to_vec();
	trailing.push(0);
	assert_eq!(
		HandshakePayload::decode(&trailing),
		Err(ProtocolError::InvalidHandshakeLength {
			expected: HANDSHAKE_PAYLOAD_LEN as u32,
			actual: (HANDSHAKE_PAYLOAD_LEN + 1) as u32,
		})
	);
}

#[test]
fn handshake_rejects_zero_pid_and_invalid_capacity() {
	let mut zero_pid = payload().encode();
	zero_pid[144..148].copy_from_slice(&0_u32.to_le_bytes());
	assert_eq!(
		HandshakePayload::decode(&zero_pid),
		Err(ProtocolError::InvalidProcessId)
	);

	let mut oversized = payload().encode();
	oversized[120..124].copy_from_slice(&(MAX_CONTROL_PAYLOAD + 1).to_le_bytes());
	assert_eq!(
		HandshakePayload::decode(&oversized),
		Err(ProtocolError::InvalidControlCapacity {
			actual: MAX_CONTROL_PAYLOAD + 1,
			maximum: MAX_CONTROL_PAYLOAD,
		})
	);

	let mut zero_capacity = payload().encode();
	zero_capacity[120..124].copy_from_slice(&0_u32.to_le_bytes());
	assert_eq!(
		HandshakePayload::decode(&zero_capacity),
		Err(ProtocolError::InvalidControlCapacity {
			actual: 0,
			maximum: MAX_CONTROL_PAYLOAD,
		})
	);

	let mut zero_callbacks = payload().encode();
	zero_callbacks[128..132].copy_from_slice(&0_u32.to_le_bytes());
	assert_eq!(
		HandshakePayload::decode(&zero_callbacks),
		Err(ProtocolError::InvalidCallbackCapacity {
			actual: 0,
			maximum: MAX_CALLBACK_EVENTS,
		})
	);

	let mut oversized_callbacks = payload().encode();
	oversized_callbacks[128..132].copy_from_slice(&(MAX_CALLBACK_EVENTS + 1).to_le_bytes());
	assert_eq!(
		HandshakePayload::decode(&oversized_callbacks),
		Err(ProtocolError::InvalidCallbackCapacity {
			actual: MAX_CALLBACK_EVENTS + 1,
			maximum: MAX_CALLBACK_EVENTS,
		})
	);

	let mut zero_continuations = payload().encode();
	zero_continuations[132..136].copy_from_slice(&0_u32.to_le_bytes());
	assert_eq!(
		HandshakePayload::decode(&zero_continuations),
		Err(ProtocolError::InvalidContinuationCapacity {
			actual: 0,
			maximum: MAX_PENDING_CONTINUATIONS,
		})
	);

	let mut oversized_continuations = payload().encode();
	oversized_continuations[132..136]
		.copy_from_slice(&(MAX_PENDING_CONTINUATIONS + 1).to_le_bytes());
	assert_eq!(
		HandshakePayload::decode(&oversized_continuations),
		Err(ProtocolError::InvalidContinuationCapacity {
			actual: MAX_PENDING_CONTINUATIONS + 1,
			maximum: MAX_PENDING_CONTINUATIONS,
		})
	);
}

#[test]
fn handshake_rejects_zero_token_and_incompatible_versions() {
	let mut zero_token = payload().encode();
	zero_token[0..32].fill(0);
	assert_eq!(
		HandshakePayload::decode(&zero_token),
		Err(ProtocolError::EmptyAuthenticationToken)
	);

	let mut wrong_abi = payload().encode();
	wrong_abi[32..34].copy_from_slice(&(DOGMOS_ABI_VERSION + 1).to_le_bytes());
	assert_eq!(
		HandshakePayload::decode(&wrong_abi),
		Err(ProtocolError::UnsupportedAbiVersion(DOGMOS_ABI_VERSION + 1))
	);

	let mut wrong_protocol = payload().encode();
	wrong_protocol[34..36].copy_from_slice(&(DOGMOS_PROTOCOL_VERSION + 1).to_le_bytes());
	assert_eq!(
		HandshakePayload::decode(&wrong_protocol),
		Err(ProtocolError::UnsupportedProtocolVersion(
			DOGMOS_PROTOCOL_VERSION + 1
		))
	);
}

#[test]
fn handshake_rejects_empty_build_identity_fields() {
	let mut empty_source_revision = payload().encode();
	empty_source_revision[36..56].fill(0);
	assert_eq!(
		HandshakePayload::decode(&empty_source_revision),
		Err(ProtocolError::EmptySourceRevision)
	);

	let mut empty_feature_fingerprint = payload().encode();
	empty_feature_fingerprint[56..88].fill(0);
	assert_eq!(
		HandshakePayload::decode(&empty_feature_fingerprint),
		Err(ProtocolError::EmptyFeatureFingerprint)
	);

	let mut empty_executable_digest = payload().encode();
	empty_executable_digest[88..120].fill(0);
	assert_eq!(
		HandshakePayload::decode(&empty_executable_digest),
		Err(ProtocolError::EmptyExecutableDigest)
	);
}

#[test]
fn peer_validation_authenticates_token_world_and_build_identity() {
	let expected = payload();
	let mut actual = expected;
	actual.process_id += 1;
	assert_eq!(actual.validate_peer(&expected), Ok(()));

	actual.auth_token[0] ^= 1;
	assert_eq!(
		actual.validate_peer(&expected),
		Err(ProtocolError::AuthenticationFailed)
	);

	actual = expected;
	actual.identity.executable_digest[0] ^= 1;
	assert_eq!(
		actual.validate_peer(&expected),
		Err(ProtocolError::BuildIdentityMismatch)
	);

	actual = expected;
	actual.capacities.max_pending_continuations -= 1;
	assert_eq!(
		actual.validate_peer(&expected),
		Err(ProtocolError::CapacityMismatch)
	);

	actual = expected;
	actual.world_generation += 1;
	assert_eq!(
		actual.validate_peer(&expected),
		Err(ProtocolError::WorldGenerationMismatch {
			expected: expected.world_generation,
			actual: actual.world_generation,
		})
	);

	actual = expected;
	actual.world_nonce += 1;
	assert_eq!(
		actual.validate_peer(&expected),
		Err(ProtocolError::WorldNonceMismatch)
	);
}
