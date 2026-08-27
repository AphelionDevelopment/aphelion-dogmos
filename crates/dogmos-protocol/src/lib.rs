#![forbid(unsafe_code)]

use std::fmt;

mod transport;

pub use transport::{read_frame_into, write_frame, TransportError};

pub const DOGMOS_FRAME_MAGIC: u32 = 0x534d_4744;
pub const DOGMOS_ABI_VERSION: u16 = 1;
pub const DOGMOS_PROTOCOL_VERSION: u16 = 4;
pub const PROTOCOL_HEADER_LEN: u16 = 48;
pub const HANDSHAKE_PAYLOAD_LEN: usize = 160;
pub const MAX_CONTROL_PAYLOAD: u32 = 1024 * 1024;
pub const MAX_CALLBACK_EVENTS: u32 = 1024 * 1024;
pub const MAX_GAS_SLOTS: usize = 32;
pub const MIXTURE_SNAPSHOT_LEN: usize = 24 + MAX_GAS_SLOTS * 8;
pub const MIXTURE_STATE_MUTATION_LEN: usize = 32 + MAX_GAS_SLOTS * 8;
pub const LIFECYCLE_MUTATION_LEN: usize = 12;
pub const ADJACENCY_MUTATION_LEN: usize = 24;
pub const SIMULATION_STAGE_REQUEST_LEN: usize = 12;
pub const SIMULATION_STAGE_RESPONSE_LEN: usize = 8;
pub const CALLBACK_BATCH_REQUEST_LEN: usize = 4;
pub const CALLBACK_BATCH_HEADER_LEN: usize = 24;
pub const CALLBACK_EVENT_LEN: usize = 64;
pub const FLAG_RESPONSE: u16 = 1 << 0;
pub const FLAG_ERROR: u16 = 1 << 1;
const KNOWN_FLAGS: u16 = FLAG_RESPONSE | FLAG_ERROR;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u16)]
pub enum OperationKind {
	Handshake = 1,
	Echo = 2,
	Shutdown = 3,
	ScalarGet = 10,
	ScalarSet = 11,
	Transfer = 12,
	GasVector = 13,
	AdjacencyUpdate = 14,
	Batch = 15,
	CallbackBatch = 16,
	AllocateDiagnostic = 17,
	MixtureSnapshot = 18,
	MixtureLifecycleBatch = 19,
	AdjacencyBatch = 20,
	SimulationStage = 21,
	DiagnosticCallbackEnqueue = 22,
	MixtureStateBatch = 23,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum ServiceErrorCode {
	Busy = 1,
	AuthenticationFailed = 2,
	InvalidRequest = 3,
	DeadlineExceeded = 4,
	Internal = 5,
	CallbackBackpressure = 6,
	UnknownHandle = 7,
	StaleHandle = 8,
	RevisionMismatch = 9,
	RevisionExhausted = 10,
	DuplicateMixtureState = 11,
	InvalidMixtureState = 12,
	StateCapacityExceeded = 13,
	AllocationFailed = 14,
	InvalidGraph = 15,
}

impl ServiceErrorCode {
	pub const fn encode(self) -> [u8; 4] {
		(self as u32).to_le_bytes()
	}

	pub fn decode(input: &[u8]) -> Result<Self, ProtocolError> {
		if input.len() != 4 {
			return Err(ProtocolError::InvalidServiceErrorLength {
				actual: input.len() as u32,
			});
		}
		match read_u32(input, 0) {
			1 => Ok(Self::Busy),
			2 => Ok(Self::AuthenticationFailed),
			3 => Ok(Self::InvalidRequest),
			4 => Ok(Self::DeadlineExceeded),
			5 => Ok(Self::Internal),
			6 => Ok(Self::CallbackBackpressure),
			7 => Ok(Self::UnknownHandle),
			8 => Ok(Self::StaleHandle),
			9 => Ok(Self::RevisionMismatch),
			10 => Ok(Self::RevisionExhausted),
			11 => Ok(Self::DuplicateMixtureState),
			12 => Ok(Self::InvalidMixtureState),
			13 => Ok(Self::StateCapacityExceeded),
			14 => Ok(Self::AllocationFailed),
			15 => Ok(Self::InvalidGraph),
			actual => Err(ProtocolError::UnknownServiceErrorCode(actual)),
		}
	}
}

impl TryFrom<u16> for OperationKind {
	type Error = ProtocolError;

	fn try_from(value: u16) -> Result<Self, Self::Error> {
		match value {
			1 => Ok(Self::Handshake),
			2 => Ok(Self::Echo),
			3 => Ok(Self::Shutdown),
			10 => Ok(Self::ScalarGet),
			11 => Ok(Self::ScalarSet),
			12 => Ok(Self::Transfer),
			13 => Ok(Self::GasVector),
			14 => Ok(Self::AdjacencyUpdate),
			15 => Ok(Self::Batch),
			16 => Ok(Self::CallbackBatch),
			17 => Ok(Self::AllocateDiagnostic),
			18 => Ok(Self::MixtureSnapshot),
			19 => Ok(Self::MixtureLifecycleBatch),
			20 => Ok(Self::AdjacencyBatch),
			21 => Ok(Self::SimulationStage),
			22 => Ok(Self::DiagnosticCallbackEnqueue),
			23 => Ok(Self::MixtureStateBatch),
			actual => Err(ProtocolError::UnknownOperationKind(actual)),
		}
	}
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(C)]
pub struct ProtocolHeader {
	pub magic: u32,
	pub protocol_version: u16,
	pub header_len: u16,
	pub operation_kind: u16,
	pub flags: u16,
	pub payload_len: u32,
	pub request_id: u64,
	pub world_generation: u32,
	pub reserved: u32,
	pub world_nonce: u64,
	pub deadline_ns: u64,
}

impl ProtocolHeader {
	pub const fn request(
		operation_kind: OperationKind,
		request_id: u64,
		world_generation: u32,
		world_nonce: u64,
		payload_len: u32,
		deadline_ns: u64,
	) -> Self {
		Self {
			magic: DOGMOS_FRAME_MAGIC,
			protocol_version: DOGMOS_PROTOCOL_VERSION,
			header_len: PROTOCOL_HEADER_LEN,
			operation_kind: operation_kind as u16,
			flags: 0,
			payload_len,
			request_id,
			world_generation,
			reserved: 0,
			world_nonce,
			deadline_ns,
		}
	}

	pub const fn response(self) -> Self {
		Self {
			flags: self.flags | FLAG_RESPONSE,
			..self
		}
	}

	pub fn operation_kind(self) -> Result<OperationKind, ProtocolError> {
		OperationKind::try_from(self.operation_kind)
	}

	pub fn encode(self) -> [u8; PROTOCOL_HEADER_LEN as usize] {
		let mut output = [0_u8; PROTOCOL_HEADER_LEN as usize];
		output[0..4].copy_from_slice(&self.magic.to_le_bytes());
		output[4..6].copy_from_slice(&self.protocol_version.to_le_bytes());
		output[6..8].copy_from_slice(&self.header_len.to_le_bytes());
		output[8..10].copy_from_slice(&self.operation_kind.to_le_bytes());
		output[10..12].copy_from_slice(&self.flags.to_le_bytes());
		output[12..16].copy_from_slice(&self.payload_len.to_le_bytes());
		output[16..24].copy_from_slice(&self.request_id.to_le_bytes());
		output[24..28].copy_from_slice(&self.world_generation.to_le_bytes());
		output[28..32].copy_from_slice(&self.reserved.to_le_bytes());
		output[32..40].copy_from_slice(&self.world_nonce.to_le_bytes());
		output[40..48].copy_from_slice(&self.deadline_ns.to_le_bytes());
		output
	}

	pub fn decode(input: &[u8]) -> Result<Self, ProtocolError> {
		if input.len() < PROTOCOL_HEADER_LEN as usize {
			return Err(ProtocolError::TruncatedHeader {
				actual: input.len() as u32,
			});
		}
		let header = Self {
			magic: read_u32(input, 0),
			protocol_version: read_u16(input, 4),
			header_len: read_u16(input, 6),
			operation_kind: read_u16(input, 8),
			flags: read_u16(input, 10),
			payload_len: read_u32(input, 12),
			request_id: read_u64(input, 16),
			world_generation: read_u32(input, 24),
			reserved: read_u32(input, 28),
			world_nonce: read_u64(input, 32),
			deadline_ns: read_u64(input, 40),
		};
		header.validate()?;
		Ok(header)
	}

	pub fn validate_response_to(self, request: &Self) -> Result<(), ProtocolError> {
		if self.flags & FLAG_RESPONSE == 0 {
			return Err(ProtocolError::ExpectedResponse);
		}
		if self.world_generation != request.world_generation {
			return Err(ProtocolError::WorldGenerationMismatch {
				expected: request.world_generation,
				actual: self.world_generation,
			});
		}
		if self.world_nonce != request.world_nonce {
			return Err(ProtocolError::WorldNonceMismatch);
		}
		if self.request_id != request.request_id {
			return Err(ProtocolError::RequestIdMismatch {
				expected: request.request_id,
				actual: self.request_id,
			});
		}
		if self.operation_kind != request.operation_kind {
			return Err(ProtocolError::OperationKindMismatch {
				expected: request.operation_kind,
				actual: self.operation_kind,
			});
		}
		Ok(())
	}

	fn validate(self) -> Result<(), ProtocolError> {
		if self.magic != DOGMOS_FRAME_MAGIC {
			return Err(ProtocolError::InvalidMagic(self.magic));
		}
		if self.protocol_version != DOGMOS_PROTOCOL_VERSION {
			return Err(ProtocolError::UnsupportedProtocolVersion(
				self.protocol_version,
			));
		}
		if self.header_len != PROTOCOL_HEADER_LEN {
			return Err(ProtocolError::InvalidHeaderLength(self.header_len));
		}
		self.operation_kind()?;
		if self.flags & !KNOWN_FLAGS != 0 {
			return Err(ProtocolError::UnknownFlags(self.flags & !KNOWN_FLAGS));
		}
		if self.flags & FLAG_ERROR != 0 && self.flags & FLAG_RESPONSE == 0 {
			return Err(ProtocolError::ErrorFlagWithoutResponse);
		}
		if self.reserved != 0 {
			return Err(ProtocolError::ReservedField(self.reserved));
		}
		if self.payload_len > MAX_CONTROL_PAYLOAD {
			return Err(ProtocolError::PayloadTooLarge {
				actual: self.payload_len,
				maximum: MAX_CONTROL_PAYLOAD,
			});
		}
		Ok(())
	}
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DecodedFrame<'a> {
	pub header: ProtocolHeader,
	pub payload: &'a [u8],
}

pub fn decode_frame(input: &[u8]) -> Result<DecodedFrame<'_>, ProtocolError> {
	let header = ProtocolHeader::decode(input)?;
	let expected_frame_len = PROTOCOL_HEADER_LEN as usize + header.payload_len as usize;
	if input.len() < expected_frame_len {
		return Err(ProtocolError::TruncatedPayload {
			expected_frame_len: expected_frame_len as u32,
			actual_frame_len: input.len() as u32,
		});
	}
	if input.len() > expected_frame_len {
		return Err(ProtocolError::TrailingBytes {
			expected_frame_len: expected_frame_len as u32,
			actual_frame_len: input.len() as u32,
		});
	}
	Ok(DecodedFrame {
		header,
		payload: &input[PROTOCOL_HEADER_LEN as usize..],
	})
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(C)]
pub struct BuildIdentity {
	pub abi_version: u16,
	pub protocol_version: u16,
	pub source_revision: [u8; 20],
	pub feature_fingerprint: [u8; 32],
	pub executable_digest: [u8; 32],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(C)]
pub struct CapacityLimits {
	pub max_control_payload: u32,
	pub max_batch_operations: u32,
	pub max_callback_events: u32,
	pub reserved: u32,
	pub max_world_bytes: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(C)]
pub struct HandshakePayload {
	pub auth_token: [u8; 32],
	pub identity: BuildIdentity,
	pub capacities: CapacityLimits,
	pub process_id: u32,
	pub world_generation: u32,
	pub world_nonce: u64,
}

impl HandshakePayload {
	pub fn encode(self) -> [u8; HANDSHAKE_PAYLOAD_LEN] {
		let mut output = [0_u8; HANDSHAKE_PAYLOAD_LEN];
		output[0..32].copy_from_slice(&self.auth_token);
		output[32..34].copy_from_slice(&self.identity.abi_version.to_le_bytes());
		output[34..36].copy_from_slice(&self.identity.protocol_version.to_le_bytes());
		output[36..56].copy_from_slice(&self.identity.source_revision);
		output[56..88].copy_from_slice(&self.identity.feature_fingerprint);
		output[88..120].copy_from_slice(&self.identity.executable_digest);
		output[120..124].copy_from_slice(&self.capacities.max_control_payload.to_le_bytes());
		output[124..128].copy_from_slice(&self.capacities.max_batch_operations.to_le_bytes());
		output[128..132].copy_from_slice(&self.capacities.max_callback_events.to_le_bytes());
		output[132..136].copy_from_slice(&self.capacities.reserved.to_le_bytes());
		output[136..144].copy_from_slice(&self.capacities.max_world_bytes.to_le_bytes());
		output[144..148].copy_from_slice(&self.process_id.to_le_bytes());
		output[148..152].copy_from_slice(&self.world_generation.to_le_bytes());
		output[152..160].copy_from_slice(&self.world_nonce.to_le_bytes());
		output
	}

	pub fn decode(input: &[u8]) -> Result<Self, ProtocolError> {
		if input.len() != HANDSHAKE_PAYLOAD_LEN {
			return Err(ProtocolError::InvalidHandshakeLength {
				expected: HANDSHAKE_PAYLOAD_LEN as u32,
				actual: input.len() as u32,
			});
		}
		let mut auth_token = [0_u8; 32];
		auth_token.copy_from_slice(&input[0..32]);
		let mut source_revision = [0_u8; 20];
		source_revision.copy_from_slice(&input[36..56]);
		let mut feature_fingerprint = [0_u8; 32];
		feature_fingerprint.copy_from_slice(&input[56..88]);
		let mut executable_digest = [0_u8; 32];
		executable_digest.copy_from_slice(&input[88..120]);
		let payload = Self {
			auth_token,
			identity: BuildIdentity {
				abi_version: read_u16(input, 32),
				protocol_version: read_u16(input, 34),
				source_revision,
				feature_fingerprint,
				executable_digest,
			},
			capacities: CapacityLimits {
				max_control_payload: read_u32(input, 120),
				max_batch_operations: read_u32(input, 124),
				max_callback_events: read_u32(input, 128),
				reserved: read_u32(input, 132),
				max_world_bytes: read_u64(input, 136),
			},
			process_id: read_u32(input, 144),
			world_generation: read_u32(input, 148),
			world_nonce: read_u64(input, 152),
		};
		payload.validate()?;
		Ok(payload)
	}

	pub fn validate_peer(self, expected: &Self) -> Result<(), ProtocolError> {
		if !constant_time_equal(&self.auth_token, &expected.auth_token) {
			return Err(ProtocolError::AuthenticationFailed);
		}
		if self.identity != expected.identity {
			return Err(ProtocolError::BuildIdentityMismatch);
		}
		if self.world_generation != expected.world_generation {
			return Err(ProtocolError::WorldGenerationMismatch {
				expected: expected.world_generation,
				actual: self.world_generation,
			});
		}
		if self.world_nonce != expected.world_nonce {
			return Err(ProtocolError::WorldNonceMismatch);
		}
		Ok(())
	}

	fn validate(self) -> Result<(), ProtocolError> {
		if self.auth_token == [0; 32] {
			return Err(ProtocolError::EmptyAuthenticationToken);
		}
		if self.identity.abi_version != DOGMOS_ABI_VERSION {
			return Err(ProtocolError::UnsupportedAbiVersion(
				self.identity.abi_version,
			));
		}
		if self.identity.protocol_version != DOGMOS_PROTOCOL_VERSION {
			return Err(ProtocolError::UnsupportedProtocolVersion(
				self.identity.protocol_version,
			));
		}
		if self.identity.source_revision == [0; 20] {
			return Err(ProtocolError::EmptySourceRevision);
		}
		if self.identity.feature_fingerprint == [0; 32] {
			return Err(ProtocolError::EmptyFeatureFingerprint);
		}
		if self.identity.executable_digest == [0; 32] {
			return Err(ProtocolError::EmptyExecutableDigest);
		}
		if self.capacities.reserved != 0 {
			return Err(ProtocolError::ReservedHandshakeField(
				self.capacities.reserved,
			));
		}
		if self.process_id == 0 {
			return Err(ProtocolError::InvalidProcessId);
		}
		if self.capacities.max_control_payload == 0
			|| self.capacities.max_control_payload > MAX_CONTROL_PAYLOAD
		{
			return Err(ProtocolError::InvalidControlCapacity {
				actual: self.capacities.max_control_payload,
				maximum: MAX_CONTROL_PAYLOAD,
			});
		}
		if self.capacities.max_callback_events == 0
			|| self.capacities.max_callback_events > MAX_CALLBACK_EVENTS
		{
			return Err(ProtocolError::InvalidCallbackCapacity {
				actual: self.capacities.max_callback_events,
				maximum: MAX_CALLBACK_EVENTS,
			});
		}
		Ok(())
	}
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(C)]
pub struct WireHandle {
	pub slot: u32,
	pub generation: u32,
}

impl WireHandle {
	pub fn encode(self) -> [u8; 8] {
		let mut output = [0_u8; 8];
		output[0..4].copy_from_slice(&self.slot.to_le_bytes());
		output[4..8].copy_from_slice(&self.generation.to_le_bytes());
		output
	}

	pub fn decode(input: &[u8]) -> Result<Self, ProtocolError> {
		if input.len() < 8 {
			return Err(ProtocolError::TruncatedHandle {
				actual: input.len() as u32,
			});
		}
		Ok(Self {
			slot: read_u32(input, 0),
			generation: read_u32(input, 4),
		})
	}
}

#[derive(Clone, Copy, Debug, PartialEq)]
#[repr(transparent)]
pub struct ScalarValue(pub f64);

impl ScalarValue {
	pub fn encode(self) -> Result<[u8; 8], ProtocolError> {
		if !self.0.is_finite() {
			return Err(ProtocolError::NonFiniteScalar);
		}
		Ok(self.0.to_bits().to_le_bytes())
	}

	pub fn decode(input: &[u8]) -> Result<Self, ProtocolError> {
		if input.len() < 8 {
			return Err(ProtocolError::TruncatedScalar {
				actual: input.len() as u32,
			});
		}
		let value = f64::from_bits(read_u64(input, 0));
		if !value.is_finite() {
			return Err(ProtocolError::NonFiniteScalar);
		}
		Ok(Self(value))
	}
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u16)]
pub enum CallbackEventKind {
	Diagnostic = 1,
	ReactionFinished = 2,
	PressureDifference = 3,
	DecompressionFloorRip = 4,
	FirelockConsideration = 5,
	TurfDestructionRequest = 6,
}

impl TryFrom<u16> for CallbackEventKind {
	type Error = ProtocolError;

	fn try_from(value: u16) -> Result<Self, Self::Error> {
		match value {
			1 => Ok(Self::Diagnostic),
			2 => Ok(Self::ReactionFinished),
			3 => Ok(Self::PressureDifference),
			4 => Ok(Self::DecompressionFloorRip),
			5 => Ok(Self::FirelockConsideration),
			6 => Ok(Self::TurfDestructionRequest),
			actual => Err(ProtocolError::UnknownCallbackEventKind(actual)),
		}
	}
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum ReactionKind {
	Plasma = 1,
	Hydrogen = 2,
	Tritium = 3,
	Freon = 4,
}

impl TryFrom<u32> for ReactionKind {
	type Error = ProtocolError;

	fn try_from(value: u32) -> Result<Self, Self::Error> {
		match value {
			1 => Ok(Self::Plasma),
			2 => Ok(Self::Hydrogen),
			3 => Ok(Self::Tritium),
			4 => Ok(Self::Freon),
			actual => Err(ProtocolError::UnknownCallbackAux {
				kind: CallbackEventKind::ReactionFinished as u16,
				actual,
			}),
		}
	}
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum TurfDestructionReason {
	SuperconductiveHeat = 1,
}

impl TryFrom<u32> for TurfDestructionReason {
	type Error = ProtocolError;

	fn try_from(value: u32) -> Result<Self, Self::Error> {
		match value {
			1 => Ok(Self::SuperconductiveHeat),
			actual => Err(ProtocolError::UnknownCallbackAux {
				kind: CallbackEventKind::TurfDestructionRequest as u16,
				actual,
			}),
		}
	}
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CallbackEvent {
	pub sequence: u64,
	pub kind: CallbackEventKind,
	pub flags: u16,
	pub subject: WireHandle,
	pub target: WireHandle,
	pub values: [ScalarValue; 4],
	pub aux: u32,
}

impl CallbackEvent {
	pub fn encode(self) -> Result<[u8; CALLBACK_EVENT_LEN], ProtocolError> {
		if self.flags != 0 {
			return Err(ProtocolError::UnknownCallbackFlags(self.flags));
		}
		validate_callback_aux(self.kind, self.aux)?;
		let mut output = [0_u8; CALLBACK_EVENT_LEN];
		output[0..8].copy_from_slice(&self.sequence.to_le_bytes());
		output[8..10].copy_from_slice(&(self.kind as u16).to_le_bytes());
		output[10..12].copy_from_slice(&self.flags.to_le_bytes());
		output[12..20].copy_from_slice(&self.subject.encode());
		output[20..28].copy_from_slice(&self.target.encode());
		for (index, value) in self.values.into_iter().enumerate() {
			let offset = 28 + index * 8;
			output[offset..offset + 8].copy_from_slice(&value.encode()?);
		}
		output[60..64].copy_from_slice(&self.aux.to_le_bytes());
		Ok(output)
	}

	pub fn decode(input: &[u8]) -> Result<Self, ProtocolError> {
		require_exact_len(input, CALLBACK_EVENT_LEN)?;
		let flags = read_u16(input, 10);
		if flags != 0 {
			return Err(ProtocolError::UnknownCallbackFlags(flags));
		}
		let kind = CallbackEventKind::try_from(read_u16(input, 8))?;
		let aux = read_u32(input, 60);
		validate_callback_aux(kind, aux)?;
		let event = Self {
			sequence: read_u64(input, 0),
			kind,
			flags,
			subject: WireHandle::decode(&input[12..20])?,
			target: WireHandle::decode(&input[20..28])?,
			values: [
				ScalarValue::decode(&input[28..36])?,
				ScalarValue::decode(&input[36..44])?,
				ScalarValue::decode(&input[44..52])?,
				ScalarValue::decode(&input[52..60])?,
			],
			aux,
		};
		Ok(event)
	}
}

fn validate_callback_aux(kind: CallbackEventKind, aux: u32) -> Result<(), ProtocolError> {
	match kind {
		CallbackEventKind::ReactionFinished => {
			ReactionKind::try_from(aux)?;
		}
		CallbackEventKind::TurfDestructionRequest => {
			TurfDestructionReason::try_from(aux)?;
		}
		CallbackEventKind::Diagnostic
		| CallbackEventKind::PressureDifference
		| CallbackEventKind::DecompressionFloorRip
		| CallbackEventKind::FirelockConsideration => {
			if aux != 0 {
				return Err(ProtocolError::UnknownCallbackAux {
					kind: kind as u16,
					actual: aux,
				});
			}
		}
	}
	Ok(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CallbackBatchRequest {
	pub max_events: u32,
}

impl CallbackBatchRequest {
	pub fn encode(self) -> [u8; CALLBACK_BATCH_REQUEST_LEN] {
		self.max_events.to_le_bytes()
	}

	pub fn decode(input: &[u8]) -> Result<Self, ProtocolError> {
		require_exact_len(input, CALLBACK_BATCH_REQUEST_LEN)?;
		Ok(Self {
			max_events: read_u32(input, 0),
		})
	}
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CallbackBatchHeader {
	pub returned: u32,
	pub remaining: u32,
	pub capacity: u32,
	pub high_water: u32,
	pub rejected: u64,
}

impl CallbackBatchHeader {
	pub fn encode(self) -> [u8; CALLBACK_BATCH_HEADER_LEN] {
		let mut output = [0_u8; CALLBACK_BATCH_HEADER_LEN];
		output[0..4].copy_from_slice(&self.returned.to_le_bytes());
		output[4..8].copy_from_slice(&self.remaining.to_le_bytes());
		output[8..12].copy_from_slice(&self.capacity.to_le_bytes());
		output[12..16].copy_from_slice(&self.high_water.to_le_bytes());
		output[16..24].copy_from_slice(&self.rejected.to_le_bytes());
		output
	}

	pub fn decode(input: &[u8]) -> Result<Self, ProtocolError> {
		require_exact_len(input, CALLBACK_BATCH_HEADER_LEN)?;
		Ok(Self {
			returned: read_u32(input, 0),
			remaining: read_u32(input, 4),
			capacity: read_u32(input, 8),
			high_water: read_u32(input, 12),
			rejected: read_u64(input, 16),
		})
	}
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MixtureSnapshotRequest {
	pub handle: WireHandle,
}

impl MixtureSnapshotRequest {
	pub fn encode(self) -> [u8; 8] {
		self.handle.encode()
	}

	pub fn decode(input: &[u8]) -> Result<Self, ProtocolError> {
		require_exact_len(input, 8)?;
		Ok(Self {
			handle: WireHandle::decode(input)?,
		})
	}
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MixtureSnapshot {
	pub revision: u32,
	pub gas_count: u32,
	pub temperature: ScalarValue,
	pub volume: ScalarValue,
	pub gases: [ScalarValue; MAX_GAS_SLOTS],
}

impl MixtureSnapshot {
	pub fn encode(self) -> Result<[u8; MIXTURE_SNAPSHOT_LEN], ProtocolError> {
		validate_gas_count(self.gas_count)?;
		let mut output = [0_u8; MIXTURE_SNAPSHOT_LEN];
		output[0..4].copy_from_slice(&self.revision.to_le_bytes());
		output[4..8].copy_from_slice(&self.gas_count.to_le_bytes());
		output[8..16].copy_from_slice(&self.temperature.encode()?);
		output[16..24].copy_from_slice(&self.volume.encode()?);
		for (index, value) in self.gases.into_iter().enumerate() {
			let offset = 24 + index * 8;
			output[offset..offset + 8].copy_from_slice(&value.encode()?);
		}
		Ok(output)
	}

	pub fn decode(input: &[u8]) -> Result<Self, ProtocolError> {
		require_exact_len(input, MIXTURE_SNAPSHOT_LEN)?;
		let gas_count = read_u32(input, 4);
		validate_gas_count(gas_count)?;
		let mut gases = [ScalarValue(0.0); MAX_GAS_SLOTS];
		for (index, value) in gases.iter_mut().enumerate() {
			let offset = 24 + index * 8;
			*value = ScalarValue::decode(&input[offset..offset + 8])?;
		}
		Ok(Self {
			revision: read_u32(input, 0),
			gas_count,
			temperature: ScalarValue::decode(&input[8..16])?,
			volume: ScalarValue::decode(&input[16..24])?,
			gases,
		})
	}
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MixtureStateMutation {
	pub handle: WireHandle,
	pub expected_revision: u32,
	pub temperature: ScalarValue,
	pub volume: ScalarValue,
	pub gases: [ScalarValue; MAX_GAS_SLOTS],
}

pub fn encode_mixture_state_batch(
	entries: &[MixtureStateMutation],
	output: &mut Vec<u8>,
) -> Result<(), ProtocolError> {
	let count = checked_encode_count(entries.len())?;
	output.clear();
	output.reserve(4 + entries.len() * MIXTURE_STATE_MUTATION_LEN);
	output.extend_from_slice(&count.to_le_bytes());
	for entry in entries {
		output.extend_from_slice(&entry.handle.encode());
		output.extend_from_slice(&entry.expected_revision.to_le_bytes());
		output.extend_from_slice(&0_u32.to_le_bytes());
		output.extend_from_slice(&entry.temperature.encode()?);
		output.extend_from_slice(&entry.volume.encode()?);
		for value in entry.gases {
			output.extend_from_slice(&value.encode()?);
		}
	}
	Ok(())
}

pub fn decode_mixture_state_batch(
	input: &[u8],
	maximum: u32,
) -> Result<Vec<MixtureStateMutation>, ProtocolError> {
	let count = validate_mixture_state_batch(input, maximum)?;
	let mut entries = Vec::with_capacity(count as usize);
	for index in 0..count as usize {
		let offset = 4 + index * MIXTURE_STATE_MUTATION_LEN;
		let mut gases = [ScalarValue(0.0); MAX_GAS_SLOTS];
		for (gas_index, value) in gases.iter_mut().enumerate() {
			let gas_offset = offset + 32 + gas_index * 8;
			*value = ScalarValue::decode(&input[gas_offset..gas_offset + 8])?;
		}
		entries.push(MixtureStateMutation {
			handle: WireHandle::decode(&input[offset..offset + 8])?,
			expected_revision: read_u32(input, offset + 8),
			temperature: ScalarValue::decode(&input[offset + 16..offset + 24])?,
			volume: ScalarValue::decode(&input[offset + 24..offset + 32])?,
			gases,
		});
	}
	Ok(entries)
}

pub fn validate_mixture_state_batch(input: &[u8], maximum: u32) -> Result<u32, ProtocolError> {
	let count = validate_counted_payload(input, MIXTURE_STATE_MUTATION_LEN, maximum)?;
	for index in 0..count as usize {
		let offset = 4 + index * MIXTURE_STATE_MUTATION_LEN;
		WireHandle::decode(&input[offset..offset + 8])?;
		let reserved = read_u32(input, offset + 12);
		if reserved != 0 {
			return Err(ProtocolError::ReservedMixtureStateField(reserved));
		}
		ScalarValue::decode(&input[offset + 16..offset + 24])?;
		ScalarValue::decode(&input[offset + 24..offset + 32])?;
		for gas_index in 0..MAX_GAS_SLOTS {
			let gas_offset = offset + 32 + gas_index * 8;
			ScalarValue::decode(&input[gas_offset..gas_offset + 8])?;
		}
	}
	Ok(count)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum LifecycleAction {
	Register = 1,
	Unregister = 2,
}

impl TryFrom<u32> for LifecycleAction {
	type Error = ProtocolError;

	fn try_from(value: u32) -> Result<Self, Self::Error> {
		match value {
			1 => Ok(Self::Register),
			2 => Ok(Self::Unregister),
			actual => Err(ProtocolError::UnknownLifecycleAction(actual)),
		}
	}
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LifecycleMutation {
	pub action: LifecycleAction,
	pub handle: WireHandle,
}

pub fn encode_lifecycle_batch(
	entries: &[LifecycleMutation],
	output: &mut Vec<u8>,
) -> Result<(), ProtocolError> {
	let count = checked_encode_count(entries.len())?;
	output.clear();
	output.reserve(4 + entries.len() * LIFECYCLE_MUTATION_LEN);
	output.extend_from_slice(&count.to_le_bytes());
	for entry in entries {
		output.extend_from_slice(&(entry.action as u32).to_le_bytes());
		output.extend_from_slice(&entry.handle.encode());
	}
	Ok(())
}

pub fn decode_lifecycle_batch(
	input: &[u8],
	maximum: u32,
) -> Result<Vec<LifecycleMutation>, ProtocolError> {
	let count = validate_lifecycle_batch(input, maximum)?;
	let mut entries = Vec::with_capacity(count as usize);
	for index in 0..count as usize {
		let offset = 4 + index * LIFECYCLE_MUTATION_LEN;
		entries.push(LifecycleMutation {
			action: LifecycleAction::try_from(read_u32(input, offset))?,
			handle: WireHandle::decode(&input[offset + 4..offset + 12])?,
		});
	}
	Ok(entries)
}

pub fn validate_lifecycle_batch(input: &[u8], maximum: u32) -> Result<u32, ProtocolError> {
	let count = validate_counted_payload(input, LIFECYCLE_MUTATION_LEN, maximum)?;
	for index in 0..count as usize {
		let offset = 4 + index * LIFECYCLE_MUTATION_LEN;
		LifecycleAction::try_from(read_u32(input, offset))?;
		WireHandle::decode(&input[offset + 4..offset + 12])?;
	}
	Ok(count)
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AdjacencyMutation {
	pub left: WireHandle,
	pub right: WireHandle,
	pub conductivity: ScalarValue,
}

pub fn encode_adjacency_batch(
	entries: &[AdjacencyMutation],
	output: &mut Vec<u8>,
) -> Result<(), ProtocolError> {
	let count = checked_encode_count(entries.len())?;
	output.clear();
	output.reserve(4 + entries.len() * ADJACENCY_MUTATION_LEN);
	output.extend_from_slice(&count.to_le_bytes());
	for entry in entries {
		output.extend_from_slice(&entry.left.encode());
		output.extend_from_slice(&entry.right.encode());
		output.extend_from_slice(&entry.conductivity.encode()?);
	}
	Ok(())
}

pub fn decode_adjacency_batch(
	input: &[u8],
	maximum: u32,
) -> Result<Vec<AdjacencyMutation>, ProtocolError> {
	let count = validate_adjacency_batch(input, maximum)?;
	let mut entries = Vec::with_capacity(count as usize);
	for index in 0..count as usize {
		let offset = 4 + index * ADJACENCY_MUTATION_LEN;
		entries.push(AdjacencyMutation {
			left: WireHandle::decode(&input[offset..offset + 8])?,
			right: WireHandle::decode(&input[offset + 8..offset + 16])?,
			conductivity: ScalarValue::decode(&input[offset + 16..offset + 24])?,
		});
	}
	Ok(entries)
}

pub fn validate_adjacency_batch(input: &[u8], maximum: u32) -> Result<u32, ProtocolError> {
	let count = validate_counted_payload(input, ADJACENCY_MUTATION_LEN, maximum)?;
	for index in 0..count as usize {
		let offset = 4 + index * ADJACENCY_MUTATION_LEN;
		WireHandle::decode(&input[offset..offset + 8])?;
		WireHandle::decode(&input[offset + 8..offset + 16])?;
		ScalarValue::decode(&input[offset + 16..offset + 24])?;
	}
	Ok(count)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum SimulationStage {
	ProcessExcitedGroups = 1,
	ProcessTurfEqualize = 2,
	ProcessTurfHeat = 3,
	ProcessTurfs = 4,
}

impl TryFrom<u32> for SimulationStage {
	type Error = ProtocolError;

	fn try_from(value: u32) -> Result<Self, Self::Error> {
		match value {
			1 => Ok(Self::ProcessExcitedGroups),
			2 => Ok(Self::ProcessTurfEqualize),
			3 => Ok(Self::ProcessTurfHeat),
			4 => Ok(Self::ProcessTurfs),
			actual => Err(ProtocolError::UnknownSimulationStage(actual)),
		}
	}
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SimulationStageRequest {
	pub stage: SimulationStage,
	pub seconds_per_tick: ScalarValue,
}

impl SimulationStageRequest {
	pub fn encode(self) -> Result<[u8; SIMULATION_STAGE_REQUEST_LEN], ProtocolError> {
		let mut output = [0_u8; SIMULATION_STAGE_REQUEST_LEN];
		output[0..4].copy_from_slice(&(self.stage as u32).to_le_bytes());
		output[4..12].copy_from_slice(&self.seconds_per_tick.encode()?);
		Ok(output)
	}

	pub fn decode(input: &[u8]) -> Result<Self, ProtocolError> {
		require_exact_len(input, SIMULATION_STAGE_REQUEST_LEN)?;
		Ok(Self {
			stage: SimulationStage::try_from(read_u32(input, 0))?,
			seconds_per_tick: ScalarValue::decode(&input[4..12])?,
		})
	}
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SimulationStageResponse {
	pub work_items: u32,
	pub callback_events: u32,
}

impl SimulationStageResponse {
	pub fn encode(self) -> [u8; SIMULATION_STAGE_RESPONSE_LEN] {
		let mut output = [0_u8; SIMULATION_STAGE_RESPONSE_LEN];
		output[0..4].copy_from_slice(&self.work_items.to_le_bytes());
		output[4..8].copy_from_slice(&self.callback_events.to_le_bytes());
		output
	}

	pub fn decode(input: &[u8]) -> Result<Self, ProtocolError> {
		require_exact_len(input, SIMULATION_STAGE_RESPONSE_LEN)?;
		Ok(Self {
			work_items: read_u32(input, 0),
			callback_events: read_u32(input, 4),
		})
	}
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProtocolError {
	TruncatedHeader {
		actual: u32,
	},
	TruncatedHandle {
		actual: u32,
	},
	TruncatedScalar {
		actual: u32,
	},
	InvalidMagic(u32),
	UnsupportedAbiVersion(u16),
	UnsupportedProtocolVersion(u16),
	InvalidHeaderLength(u16),
	UnknownOperationKind(u16),
	UnknownFlags(u16),
	ReservedField(u32),
	PayloadTooLarge {
		actual: u32,
		maximum: u32,
	},
	TruncatedPayload {
		expected_frame_len: u32,
		actual_frame_len: u32,
	},
	TrailingBytes {
		expected_frame_len: u32,
		actual_frame_len: u32,
	},
	InvalidHandshakeLength {
		expected: u32,
		actual: u32,
	},
	ReservedHandshakeField(u32),
	InvalidProcessId,
	EmptyAuthenticationToken,
	EmptySourceRevision,
	EmptyFeatureFingerprint,
	EmptyExecutableDigest,
	InvalidControlCapacity {
		actual: u32,
		maximum: u32,
	},
	InvalidCallbackCapacity {
		actual: u32,
		maximum: u32,
	},
	AuthenticationFailed,
	BuildIdentityMismatch,
	InvalidServiceErrorLength {
		actual: u32,
	},
	UnknownServiceErrorCode(u32),
	ErrorFlagWithoutResponse,
	ExpectedResponse,
	WorldGenerationMismatch {
		expected: u32,
		actual: u32,
	},
	WorldNonceMismatch,
	RequestIdMismatch {
		expected: u64,
		actual: u64,
	},
	OperationKindMismatch {
		expected: u16,
		actual: u16,
	},
	NonFiniteScalar,
	InvalidPayloadLength {
		expected: u32,
		actual: u32,
	},
	GasCountExceeded {
		actual: u32,
		maximum: u32,
	},
	OperationCountExceeded {
		actual: u32,
		maximum: u32,
	},
	UnknownLifecycleAction(u32),
	ReservedMixtureStateField(u32),
	UnknownSimulationStage(u32),
	UnknownCallbackEventKind(u16),
	UnknownCallbackFlags(u16),
	UnknownCallbackAux {
		kind: u16,
		actual: u32,
	},
}

impl fmt::Display for ProtocolError {
	fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
		write!(formatter, "{self:?}")
	}
}

impl std::error::Error for ProtocolError {}

fn read_u16(input: &[u8], offset: usize) -> u16 {
	u16::from_le_bytes([input[offset], input[offset + 1]])
}

fn read_u32(input: &[u8], offset: usize) -> u32 {
	u32::from_le_bytes([
		input[offset],
		input[offset + 1],
		input[offset + 2],
		input[offset + 3],
	])
}

fn read_u64(input: &[u8], offset: usize) -> u64 {
	u64::from_le_bytes([
		input[offset],
		input[offset + 1],
		input[offset + 2],
		input[offset + 3],
		input[offset + 4],
		input[offset + 5],
		input[offset + 6],
		input[offset + 7],
	])
}

fn require_exact_len(input: &[u8], expected: usize) -> Result<(), ProtocolError> {
	if input.len() != expected {
		return Err(ProtocolError::InvalidPayloadLength {
			expected: expected as u32,
			actual: input.len().min(u32::MAX as usize) as u32,
		});
	}
	Ok(())
}

fn validate_gas_count(gas_count: u32) -> Result<(), ProtocolError> {
	if gas_count > MAX_GAS_SLOTS as u32 {
		return Err(ProtocolError::GasCountExceeded {
			actual: gas_count,
			maximum: MAX_GAS_SLOTS as u32,
		});
	}
	Ok(())
}

fn checked_encode_count(count: usize) -> Result<u32, ProtocolError> {
	u32::try_from(count).map_err(|_| ProtocolError::OperationCountExceeded {
		actual: u32::MAX,
		maximum: u32::MAX,
	})
}

fn validate_counted_payload(
	input: &[u8],
	record_len: usize,
	maximum: u32,
) -> Result<u32, ProtocolError> {
	if input.len() < 4 {
		return Err(ProtocolError::InvalidPayloadLength {
			expected: 4,
			actual: input.len() as u32,
		});
	}
	let count = read_u32(input, 0);
	if count > maximum {
		return Err(ProtocolError::OperationCountExceeded {
			actual: count,
			maximum,
		});
	}
	let expected = 4_usize
		.checked_add((count as usize).checked_mul(record_len).ok_or(
			ProtocolError::InvalidPayloadLength {
				expected: u32::MAX,
				actual: input.len().min(u32::MAX as usize) as u32,
			},
		)?)
		.ok_or(ProtocolError::InvalidPayloadLength {
			expected: u32::MAX,
			actual: input.len().min(u32::MAX as usize) as u32,
		})?;
	require_exact_len(input, expected)?;
	Ok(count)
}

fn constant_time_equal(left: &[u8; 32], right: &[u8; 32]) -> bool {
	let mut difference = 0_u8;
	for index in 0..left.len() {
		difference |= left[index] ^ right[index];
	}
	difference == 0
}
