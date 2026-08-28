#![forbid(unsafe_code)]

use std::{collections::BTreeSet, fmt};

mod continuation_wire;
mod metadata_wire;
mod telemetry_wire;
mod transport;

pub use continuation_wire::*;
pub use metadata_wire::*;
pub use telemetry_wire::*;
pub use transport::{read_frame_into, write_frame, TransportError};

pub const DOGMOS_FRAME_MAGIC: u32 = 0x534d_4744;
pub const DOGMOS_ABI_VERSION: u16 = 1;
pub const DOGMOS_PROTOCOL_VERSION: u16 = 8;
pub const PROTOCOL_HEADER_LEN: u16 = 48;
pub const HANDSHAKE_PAYLOAD_LEN: usize = 160;
pub const MAX_CONTROL_PAYLOAD: u32 = 1024 * 1024;
pub const MAX_CALLBACK_EVENTS: u32 = 1024 * 1024;
pub const MAX_GAS_SLOTS: usize = 32;
pub const MIXTURE_SNAPSHOT_LEN: usize = 40 + MAX_GAS_SLOTS * 8;
pub const MIXTURE_STATE_MUTATION_LEN: usize = 32 + MAX_GAS_SLOTS * 8;
pub const LIFECYCLE_MUTATION_LEN: usize = 12;
pub const ADJACENCY_MUTATION_LEN: usize = 24;
pub const TURF_LIFECYCLE_MUTATION_LEN: usize = 24;
pub const TURF_ADJACENCY_MUTATION_LEN: usize = 24;
pub const TURF_HEAT_MUTATION_LEN: usize = 40;
pub const TURF_HEAT_ADJACENCY_MUTATION_LEN: usize = 24;
pub const TURF_HEAT_SNAPSHOT_LEN: usize = 32;
pub const MIXTURE_COMMAND_REQUEST_LEN: usize = 56;
pub const MIXTURE_COMMAND_RESPONSE_LEN: usize = 24;
pub const MIXTURE_ADJUST_MULTIPLE_HEADER_LEN: usize = 12;
pub const MIXTURE_ADJUSTMENT_LEN: usize = 16;
pub const SIMULATION_STAGE_REQUEST_LEN: usize = 12;
pub const SIMULATION_STAGE_RESPONSE_LEN: usize = 8;
pub const CALLBACK_BATCH_REQUEST_LEN: usize = 4;
pub const CALLBACK_BATCH_HEADER_LEN: usize = 24;
pub const CALLBACK_EVENT_LEN: usize = 88;
pub const FLAG_RESPONSE: u16 = 1 << 0;
pub const FLAG_ERROR: u16 = 1 << 1;
const KNOWN_FLAGS: u16 = FLAG_RESPONSE | FLAG_ERROR;
pub const REACTION_REACTING: u32 = 1 << 0;
pub const REACTION_STOP: u32 = 1 << 1;
pub const REACTION_VOLATILE: u32 = 1 << 2;
pub const REACTION_FLAGS: u32 = REACTION_REACTING | REACTION_STOP | REACTION_VOLATILE;

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
	TurfLifecycleBatch = 24,
	TurfAdjacencyBatch = 25,
	TurfHeatBatch = 26,
	TurfHeatAdjacencyBatch = 27,
	MixtureCommand = 28,
	GasMetadataInstall = 29,
	ReactionMetadataInstall = 30,
	MixtureAdjustMultiple = 31,
	ContinuationCommand = 32,
	ContinuationAdjustMultiple = 33,
	ContinuationResume = 34,
	ContinuationCancel = 35,
	ServiceTelemetry = 36,
	TurfHeatSnapshot = 37,
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
	UnknownContinuation = 16,
	ContinuationExpired = 17,
	ContinuationCapacityExceeded = 18,
	ContinuationWorldMismatch = 19,
	ContinuationTokenMismatch = 20,
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
			16 => Ok(Self::UnknownContinuation),
			17 => Ok(Self::ContinuationExpired),
			18 => Ok(Self::ContinuationCapacityExceeded),
			19 => Ok(Self::ContinuationWorldMismatch),
			20 => Ok(Self::ContinuationTokenMismatch),
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
			24 => Ok(Self::TurfLifecycleBatch),
			25 => Ok(Self::TurfAdjacencyBatch),
			26 => Ok(Self::TurfHeatBatch),
			27 => Ok(Self::TurfHeatAdjacencyBatch),
			28 => Ok(Self::MixtureCommand),
			29 => Ok(Self::GasMetadataInstall),
			30 => Ok(Self::ReactionMetadataInstall),
			31 => Ok(Self::MixtureAdjustMultiple),
			32 => Ok(Self::ContinuationCommand),
			33 => Ok(Self::ContinuationAdjustMultiple),
			34 => Ok(Self::ContinuationResume),
			35 => Ok(Self::ContinuationCancel),
			36 => Ok(Self::ServiceTelemetry),
			37 => Ok(Self::TurfHeatSnapshot),
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
	pub max_pending_continuations: u32,
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
		output[132..136].copy_from_slice(&self.capacities.max_pending_continuations.to_le_bytes());
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
				max_pending_continuations: read_u32(input, 132),
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
		if self.capacities != expected.capacities {
			return Err(ProtocolError::CapacityMismatch);
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
		if self.capacities.max_pending_continuations == 0
			|| self.capacities.max_pending_continuations > MAX_PENDING_CONTINUATIONS
		{
			return Err(ProtocolError::InvalidContinuationCapacity {
				actual: self.capacities.max_pending_continuations,
				maximum: MAX_PENDING_CONTINUATIONS,
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
	RunDmReaction = 7,
	ReactionProfiled = 8,
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
			7 => Ok(Self::RunDmReaction),
			8 => Ok(Self::ReactionProfiled),
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
	pub continuation: Option<ContinuationToken>,
}

impl CallbackEvent {
	pub fn encode(self) -> Result<[u8; CALLBACK_EVENT_LEN], ProtocolError> {
		if self.flags != 0 {
			return Err(ProtocolError::UnknownCallbackFlags(self.flags));
		}
		validate_callback_aux(self.kind, self.aux)?;
		match (self.kind, self.continuation) {
			(CallbackEventKind::RunDmReaction, Some(_)) => {}
			(CallbackEventKind::RunDmReaction, None) => {
				return Err(ProtocolError::MissingContinuationToken);
			}
			(_, Some(_)) => return Err(ProtocolError::UnexpectedContinuationToken),
			(_, None) => {}
		}
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
		if let Some(continuation) = self.continuation {
			output[64..88].copy_from_slice(&continuation.encode()?);
		}
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
		let continuation_present = input[64..88].iter().any(|byte| *byte != 0);
		let continuation = match (kind, continuation_present) {
			(CallbackEventKind::RunDmReaction, true) => {
				Some(ContinuationToken::decode(&input[64..88])?)
			}
			(CallbackEventKind::RunDmReaction, false) => {
				return Err(ProtocolError::MissingContinuationToken);
			}
			(_, true) => return Err(ProtocolError::UnexpectedContinuationToken),
			(_, false) => None,
		};
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
			continuation,
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
		CallbackEventKind::RunDmReaction | CallbackEventKind::ReactionProfiled => {}
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
	pub minimum_heat_capacity: ScalarValue,
	pub immutable: bool,
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
		output[24..32].copy_from_slice(&self.minimum_heat_capacity.encode()?);
		output[32..36].copy_from_slice(&u32::from(self.immutable).to_le_bytes());
		for (index, value) in self.gases.into_iter().enumerate() {
			let offset = 40 + index * 8;
			output[offset..offset + 8].copy_from_slice(&value.encode()?);
		}
		Ok(output)
	}

	pub fn decode(input: &[u8]) -> Result<Self, ProtocolError> {
		require_exact_len(input, MIXTURE_SNAPSHOT_LEN)?;
		let gas_count = read_u32(input, 4);
		validate_gas_count(gas_count)?;
		let flags = read_u32(input, 32);
		if flags & !1 != 0 {
			return Err(ProtocolError::UnknownMixtureSnapshotFlags(flags));
		}
		let reserved = read_u32(input, 36);
		if reserved != 0 {
			return Err(ProtocolError::ReservedMixtureSnapshotField(reserved));
		}
		let mut gases = [ScalarValue(0.0); MAX_GAS_SLOTS];
		for (index, value) in gases.iter_mut().enumerate() {
			let offset = 40 + index * 8;
			*value = ScalarValue::decode(&input[offset..offset + 8])?;
		}
		Ok(Self {
			revision: read_u32(input, 0),
			gas_count,
			temperature: ScalarValue::decode(&input[8..16])?,
			volume: ScalarValue::decode(&input[16..24])?,
			minimum_heat_capacity: ScalarValue::decode(&input[24..32])?,
			immutable: flags & 1 != 0,
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

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MixtureAdjustment {
	pub gas_id: u16,
	pub delta: ScalarValue,
}

pub fn encode_adjust_multiple_request(
	handle: WireHandle,
	adjustments: &[MixtureAdjustment],
	output: &mut Vec<u8>,
) -> Result<(), ProtocolError> {
	let count =
		u32::try_from(adjustments.len()).map_err(|_| ProtocolError::OperationCountExceeded {
			actual: u32::MAX,
			maximum: MAX_GAS_SLOTS as u32,
		})?;
	if count > MAX_GAS_SLOTS as u32 {
		return Err(ProtocolError::OperationCountExceeded {
			actual: count,
			maximum: MAX_GAS_SLOTS as u32,
		});
	}
	output.clear();
	output.resize(
		MIXTURE_ADJUST_MULTIPLE_HEADER_LEN + adjustments.len() * MIXTURE_ADJUSTMENT_LEN,
		0,
	);
	output[0..8].copy_from_slice(&handle.encode());
	output[8..12].copy_from_slice(&count.to_le_bytes());
	for (index, adjustment) in adjustments.iter().enumerate() {
		let offset = MIXTURE_ADJUST_MULTIPLE_HEADER_LEN + index * MIXTURE_ADJUSTMENT_LEN;
		output[offset..offset + 2].copy_from_slice(&adjustment.gas_id.to_le_bytes());
		output[offset + 8..offset + 16].copy_from_slice(&adjustment.delta.encode()?);
	}
	Ok(())
}

pub fn decode_adjust_multiple_request(
	input: &[u8],
) -> Result<(WireHandle, Vec<MixtureAdjustment>), ProtocolError> {
	if input.len() < MIXTURE_ADJUST_MULTIPLE_HEADER_LEN {
		return Err(ProtocolError::InvalidPayloadLength {
			expected: MIXTURE_ADJUST_MULTIPLE_HEADER_LEN as u32,
			actual: input.len() as u32,
		});
	}
	let handle = WireHandle::decode(&input[0..8])?;
	let count = read_u32(input, 8);
	if count > MAX_GAS_SLOTS as u32 {
		return Err(ProtocolError::OperationCountExceeded {
			actual: count,
			maximum: MAX_GAS_SLOTS as u32,
		});
	}
	let expected = MIXTURE_ADJUST_MULTIPLE_HEADER_LEN
		.checked_add((count as usize).checked_mul(MIXTURE_ADJUSTMENT_LEN).ok_or(
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
	let mut adjustments = Vec::with_capacity(count as usize);
	for index in 0..count as usize {
		let offset = MIXTURE_ADJUST_MULTIPLE_HEADER_LEN + index * MIXTURE_ADJUSTMENT_LEN;
		if read_u16(input, offset + 2) != 0 || read_u32(input, offset + 4) != 0 {
			return Err(ProtocolError::ReservedMixtureAdjustmentField);
		}
		adjustments.push(MixtureAdjustment {
			gas_id: read_u16(input, offset),
			delta: ScalarValue::decode(&input[offset + 8..offset + 16])?,
		});
	}
	Ok((handle, adjustments))
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TurfLifecycleMutation {
	pub action: LifecycleAction,
	pub turf: WireHandle,
	pub mixture: Option<WireHandle>,
}

pub fn encode_turf_lifecycle_batch(
	entries: &[TurfLifecycleMutation],
	output: &mut Vec<u8>,
) -> Result<(), ProtocolError> {
	let count = checked_encode_count(entries.len())?;
	output.clear();
	output.reserve(4 + entries.len() * TURF_LIFECYCLE_MUTATION_LEN);
	output.extend_from_slice(&count.to_le_bytes());
	for entry in entries {
		if entry.action == LifecycleAction::Unregister && entry.mixture.is_some() {
			return Err(ProtocolError::UnexpectedUnregisterMixture);
		}
		output.extend_from_slice(&(entry.action as u32).to_le_bytes());
		output.extend_from_slice(&entry.turf.encode());
		output.extend_from_slice(&u32::from(entry.mixture.is_some()).to_le_bytes());
		output.extend_from_slice(
			&entry
				.mixture
				.unwrap_or(WireHandle {
					slot: 0,
					generation: 0,
				})
				.encode(),
		);
	}
	Ok(())
}

pub fn decode_turf_lifecycle_batch(
	input: &[u8],
	maximum: u32,
) -> Result<Vec<TurfLifecycleMutation>, ProtocolError> {
	let count = validate_counted_payload(input, TURF_LIFECYCLE_MUTATION_LEN, maximum)?;
	let mut entries = Vec::with_capacity(count as usize);
	for index in 0..count as usize {
		let offset = 4 + index * TURF_LIFECYCLE_MUTATION_LEN;
		let action = LifecycleAction::try_from(read_u32(input, offset))?;
		let turf = WireHandle::decode(&input[offset + 4..offset + 12])?;
		let present = decode_boolean(read_u32(input, offset + 12))?;
		let mixture = WireHandle::decode(&input[offset + 16..offset + 24])?;
		if !present
			&& mixture
				!= (WireHandle {
					slot: 0,
					generation: 0,
				}) {
			return Err(ProtocolError::NonZeroAbsentHandle);
		}
		if action == LifecycleAction::Unregister && present {
			return Err(ProtocolError::UnexpectedUnregisterMixture);
		}
		entries.push(TurfLifecycleMutation {
			action,
			turf,
			mixture: present.then_some(mixture),
		});
	}
	Ok(entries)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TurfAdjacencyMutation {
	pub left: WireHandle,
	pub right: WireHandle,
	pub connected: bool,
	pub firelock: bool,
}

pub fn encode_turf_adjacency_batch(
	entries: &[TurfAdjacencyMutation],
	output: &mut Vec<u8>,
) -> Result<(), ProtocolError> {
	let count = checked_encode_count(entries.len())?;
	validate_unique_turf_adjacency(entries)?;
	output.clear();
	output.reserve(4 + entries.len() * TURF_ADJACENCY_MUTATION_LEN);
	output.extend_from_slice(&count.to_le_bytes());
	for entry in entries {
		if entry.firelock && !entry.connected {
			return Err(ProtocolError::FirelockOnDisconnectedEdge);
		}
		output.extend_from_slice(&entry.left.encode());
		output.extend_from_slice(&entry.right.encode());
		output.extend_from_slice(&u32::from(entry.connected).to_le_bytes());
		output.extend_from_slice(&u32::from(entry.firelock).to_le_bytes());
	}
	Ok(())
}

pub fn decode_turf_adjacency_batch(
	input: &[u8],
	maximum: u32,
) -> Result<Vec<TurfAdjacencyMutation>, ProtocolError> {
	let count = validate_counted_payload(input, TURF_ADJACENCY_MUTATION_LEN, maximum)?;
	let mut entries = Vec::with_capacity(count as usize);
	for index in 0..count as usize {
		let offset = 4 + index * TURF_ADJACENCY_MUTATION_LEN;
		let connected = decode_boolean(read_u32(input, offset + 16))?;
		let firelock = decode_boolean(read_u32(input, offset + 20))?;
		if firelock && !connected {
			return Err(ProtocolError::FirelockOnDisconnectedEdge);
		}
		entries.push(TurfAdjacencyMutation {
			left: WireHandle::decode(&input[offset..offset + 8])?,
			right: WireHandle::decode(&input[offset + 8..offset + 16])?,
			connected,
			firelock,
		});
	}
	validate_unique_turf_adjacency(&entries)?;
	Ok(entries)
}

fn validate_unique_turf_adjacency(entries: &[TurfAdjacencyMutation]) -> Result<(), ProtocolError> {
	let mut edges = BTreeSet::new();
	for entry in entries {
		let edge = (
			entry.left.slot.min(entry.right.slot),
			entry.left.slot.max(entry.right.slot),
		);
		if !edges.insert(edge) {
			return Err(ProtocolError::DuplicateTurfAdjacency {
				left: edge.0,
				right: edge.1,
			});
		}
	}
	Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TurfHeatState {
	pub temperature: ScalarValue,
	pub thermal_conductivity: ScalarValue,
	pub heat_capacity: ScalarValue,
	pub adjacent_to_space: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TurfHeatMutation {
	pub turf: WireHandle,
	pub state: Option<TurfHeatState>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TurfHeatSnapshotRequest {
	pub turf: WireHandle,
}

impl TurfHeatSnapshotRequest {
	pub fn encode(self) -> [u8; 8] {
		self.turf.encode()
	}

	pub fn decode(input: &[u8]) -> Result<Self, ProtocolError> {
		require_exact_len(input, 8)?;
		Ok(Self {
			turf: WireHandle::decode(input)?,
		})
	}
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TurfHeatSnapshot {
	pub state: Option<TurfHeatState>,
}

impl TurfHeatSnapshot {
	pub fn encode(self) -> Result<[u8; TURF_HEAT_SNAPSHOT_LEN], ProtocolError> {
		let mut output = [0_u8; TURF_HEAT_SNAPSHOT_LEN];
		let (flags, temperature, conductivity, capacity) = self.state.map_or(
			(0, ScalarValue(0.0), ScalarValue(0.0), ScalarValue(0.0)),
			|state| {
				(
					1 | (u32::from(state.adjacent_to_space) << 1),
					state.temperature,
					state.thermal_conductivity,
					state.heat_capacity,
				)
			},
		);
		output[0..4].copy_from_slice(&flags.to_le_bytes());
		output[8..16].copy_from_slice(&temperature.encode()?);
		output[16..24].copy_from_slice(&conductivity.encode()?);
		output[24..32].copy_from_slice(&capacity.encode()?);
		Ok(output)
	}

	pub fn decode(input: &[u8]) -> Result<Self, ProtocolError> {
		require_exact_len(input, TURF_HEAT_SNAPSHOT_LEN)?;
		let flags = read_u32(input, 0);
		if flags & !3 != 0 {
			return Err(ProtocolError::UnknownTurfHeatFlags(flags));
		}
		let reserved = read_u32(input, 4);
		if reserved != 0 {
			return Err(ProtocolError::ReservedTurfHeatField(reserved));
		}
		let present = flags & 1 != 0;
		let adjacent_to_space = flags & 2 != 0;
		if adjacent_to_space && !present {
			return Err(ProtocolError::UnknownTurfHeatFlags(flags));
		}
		let temperature = ScalarValue::decode(&input[8..16])?;
		let thermal_conductivity = ScalarValue::decode(&input[16..24])?;
		let heat_capacity = ScalarValue::decode(&input[24..32])?;
		if !present
			&& (temperature.0 != 0.0 || thermal_conductivity.0 != 0.0 || heat_capacity.0 != 0.0)
		{
			return Err(ProtocolError::NonZeroAbsentTurfHeatState);
		}
		Ok(Self {
			state: present.then_some(TurfHeatState {
				temperature,
				thermal_conductivity,
				heat_capacity,
				adjacent_to_space,
			}),
		})
	}
}

pub fn encode_turf_heat_batch(
	entries: &[TurfHeatMutation],
	output: &mut Vec<u8>,
) -> Result<(), ProtocolError> {
	let count = checked_encode_count(entries.len())?;
	output.clear();
	output.reserve(4 + entries.len() * TURF_HEAT_MUTATION_LEN);
	output.extend_from_slice(&count.to_le_bytes());
	for entry in entries {
		output.extend_from_slice(&entry.turf.encode());
		let (flags, temperature, conductivity, capacity) = entry.state.map_or(
			(0, ScalarValue(0.0), ScalarValue(0.0), ScalarValue(0.0)),
			|state| {
				(
					1 | (u32::from(state.adjacent_to_space) << 1),
					state.temperature,
					state.thermal_conductivity,
					state.heat_capacity,
				)
			},
		);
		output.extend_from_slice(&flags.to_le_bytes());
		output.extend_from_slice(&0_u32.to_le_bytes());
		output.extend_from_slice(&temperature.encode()?);
		output.extend_from_slice(&conductivity.encode()?);
		output.extend_from_slice(&capacity.encode()?);
	}
	Ok(())
}

pub fn decode_turf_heat_batch(
	input: &[u8],
	maximum: u32,
) -> Result<Vec<TurfHeatMutation>, ProtocolError> {
	let count = validate_counted_payload(input, TURF_HEAT_MUTATION_LEN, maximum)?;
	let mut entries = Vec::with_capacity(count as usize);
	for index in 0..count as usize {
		let offset = 4 + index * TURF_HEAT_MUTATION_LEN;
		let flags = read_u32(input, offset + 8);
		if flags & !3 != 0 {
			return Err(ProtocolError::UnknownTurfHeatFlags(flags));
		}
		let reserved = read_u32(input, offset + 12);
		if reserved != 0 {
			return Err(ProtocolError::ReservedTurfHeatField(reserved));
		}
		let present = flags & 1 != 0;
		let adjacent_to_space = flags & 2 != 0;
		if adjacent_to_space && !present {
			return Err(ProtocolError::UnknownTurfHeatFlags(flags));
		}
		let temperature = ScalarValue::decode(&input[offset + 16..offset + 24])?;
		let thermal_conductivity = ScalarValue::decode(&input[offset + 24..offset + 32])?;
		let heat_capacity = ScalarValue::decode(&input[offset + 32..offset + 40])?;
		if !present
			&& (temperature.0 != 0.0 || thermal_conductivity.0 != 0.0 || heat_capacity.0 != 0.0)
		{
			return Err(ProtocolError::NonZeroAbsentTurfHeatState);
		}
		entries.push(TurfHeatMutation {
			turf: WireHandle::decode(&input[offset..offset + 8])?,
			state: present.then_some(TurfHeatState {
				temperature,
				thermal_conductivity,
				heat_capacity,
				adjacent_to_space,
			}),
		});
	}
	Ok(entries)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TurfHeatAdjacencyMutation {
	pub left: WireHandle,
	pub right: WireHandle,
	pub connected: bool,
}

pub fn encode_turf_heat_adjacency_batch(
	entries: &[TurfHeatAdjacencyMutation],
	output: &mut Vec<u8>,
) -> Result<(), ProtocolError> {
	let count = checked_encode_count(entries.len())?;
	output.clear();
	output.reserve(4 + entries.len() * TURF_HEAT_ADJACENCY_MUTATION_LEN);
	output.extend_from_slice(&count.to_le_bytes());
	for entry in entries {
		output.extend_from_slice(&entry.left.encode());
		output.extend_from_slice(&entry.right.encode());
		output.extend_from_slice(&u32::from(entry.connected).to_le_bytes());
		output.extend_from_slice(&0_u32.to_le_bytes());
	}
	Ok(())
}

pub fn decode_turf_heat_adjacency_batch(
	input: &[u8],
	maximum: u32,
) -> Result<Vec<TurfHeatAdjacencyMutation>, ProtocolError> {
	let count = validate_counted_payload(input, TURF_HEAT_ADJACENCY_MUTATION_LEN, maximum)?;
	let mut entries = Vec::with_capacity(count as usize);
	for index in 0..count as usize {
		let offset = 4 + index * TURF_HEAT_ADJACENCY_MUTATION_LEN;
		let connected = decode_boolean(read_u32(input, offset + 16))?;
		let reserved = read_u32(input, offset + 20);
		if reserved != 0 {
			return Err(ProtocolError::ReservedTurfHeatField(reserved));
		}
		entries.push(TurfHeatAdjacencyMutation {
			left: WireHandle::decode(&input[offset..offset + 8])?,
			right: WireHandle::decode(&input[offset + 8..offset + 16])?,
			connected,
		});
	}
	Ok(entries)
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum MixtureCommandRequest {
	SetMoles {
		handle: WireHandle,
		gas_id: u16,
		amount: ScalarValue,
	},
	AdjustMoles {
		handle: WireHandle,
		gas_id: u16,
		delta: ScalarValue,
	},
	AdjustMolesTemperature {
		handle: WireHandle,
		gas_id: u16,
		amount: ScalarValue,
		temperature: ScalarValue,
	},
	GetMoles {
		handle: WireHandle,
		gas_id: u16,
	},
	Temperature {
		handle: WireHandle,
	},
	Volume {
		handle: WireHandle,
	},
	HeatCapacity {
		handle: WireHandle,
	},
	PartialHeatCapacity {
		handle: WireHandle,
		gas_id: u16,
	},
	TotalMoles {
		handle: WireHandle,
	},
	Pressure {
		handle: WireHandle,
	},
	ThermalEnergy {
		handle: WireHandle,
	},
	GetMolesByFlags {
		handle: WireHandle,
		flags: u32,
	},
	Burnability {
		handle: WireHandle,
		temperature: Option<ScalarValue>,
	},
	SetTemperature {
		handle: WireHandle,
		temperature: ScalarValue,
	},
	SetVolume {
		handle: WireHandle,
		volume: ScalarValue,
	},
	SetMinimumHeatCapacity {
		handle: WireHandle,
		amount: ScalarValue,
	},
	Clear {
		handle: WireHandle,
	},
	Add {
		handle: WireHandle,
		amount: ScalarValue,
	},
	Multiply {
		handle: WireHandle,
		factor: ScalarValue,
	},
	CopyFrom {
		receiver: WireHandle,
		giver: WireHandle,
	},
	AdjustHeat {
		handle: WireHandle,
		heat: ScalarValue,
	},
	Compare {
		left: WireHandle,
		right: WireHandle,
	},
	EqualizeWith {
		receiver: WireHandle,
		total: WireHandle,
	},
	TemperatureShare {
		first: WireHandle,
		second: WireHandle,
		conduction_coefficient: ScalarValue,
	},
	TemperatureShareNonGas {
		handle: WireHandle,
		conduction_coefficient: ScalarValue,
		sharer_temperature: ScalarValue,
		sharer_heat_capacity: ScalarValue,
	},
	MarkImmutable {
		handle: WireHandle,
	},
	IsImmutable {
		handle: WireHandle,
	},
	Merge {
		receiver: WireHandle,
		giver: WireHandle,
	},
	RemoveRatioInto {
		source: WireHandle,
		destination: WireHandle,
		ratio: ScalarValue,
	},
	RemoveAmountInto {
		source: WireHandle,
		destination: WireHandle,
		amount: ScalarValue,
	},
	TransferGases {
		source: WireHandle,
		destination: WireHandle,
		ratio: ScalarValue,
		gas_mask: u32,
	},
	TransferAmount {
		source: WireHandle,
		destination: WireHandle,
		amount: ScalarValue,
	},
	TransferRatio {
		source: WireHandle,
		destination: WireHandle,
		ratio: ScalarValue,
	},
	TransferByFlags {
		source: WireHandle,
		destination: WireHandle,
		flags: u32,
		amount: ScalarValue,
	},
	ShareRatio {
		first: WireHandle,
		second: WireHandle,
		ratio: ScalarValue,
		one_way: bool,
	},
	React {
		handle: WireHandle,
		target: WireHandle,
		reaction_profile_threshold_ms: Option<ScalarValue>,
	},
}

impl MixtureCommandRequest {
	pub fn encode(self) -> Result<[u8; MIXTURE_COMMAND_REQUEST_LEN], ProtocolError> {
		let zero = WireHandle {
			slot: 0,
			generation: 0,
		};
		let z = ScalarValue(0.0);
		let (kind, flags, primary, secondary, scalars, gas_id, aux): (
			u16,
			u16,
			WireHandle,
			WireHandle,
			[ScalarValue; 3],
			u16,
			u32,
		) = match self {
			Self::SetMoles {
				handle,
				gas_id,
				amount,
			} => (1, 0, handle, zero, [amount, z, z], gas_id, 0),
			Self::AdjustMoles {
				handle,
				gas_id,
				delta,
			} => (2, 0, handle, zero, [delta, z, z], gas_id, 0),
			Self::AdjustMolesTemperature {
				handle,
				gas_id,
				amount,
				temperature,
			} => (3, 0, handle, zero, [amount, temperature, z], gas_id, 0),
			Self::GetMoles { handle, gas_id } => (4, 0, handle, zero, [z, z, z], gas_id, 0),
			Self::Temperature { handle } => (5, 0, handle, zero, [z, z, z], 0, 0),
			Self::Volume { handle } => (6, 0, handle, zero, [z, z, z], 0, 0),
			Self::HeatCapacity { handle } => (7, 0, handle, zero, [z, z, z], 0, 0),
			Self::PartialHeatCapacity { handle, gas_id } => {
				(8, 0, handle, zero, [z, z, z], gas_id, 0)
			}
			Self::TotalMoles { handle } => (9, 0, handle, zero, [z, z, z], 0, 0),
			Self::Pressure { handle } => (10, 0, handle, zero, [z, z, z], 0, 0),
			Self::ThermalEnergy { handle } => (11, 0, handle, zero, [z, z, z], 0, 0),
			Self::GetMolesByFlags { handle, flags } => (12, 0, handle, zero, [z, z, z], 0, flags),
			Self::Burnability {
				handle,
				temperature,
			} => (
				13,
				u16::from(temperature.is_some()),
				handle,
				zero,
				[temperature.unwrap_or(z), z, z],
				0,
				0,
			),
			Self::SetTemperature {
				handle,
				temperature,
			} => (14, 0, handle, zero, [temperature, z, z], 0, 0),
			Self::SetVolume { handle, volume } => (15, 0, handle, zero, [volume, z, z], 0, 0),
			Self::SetMinimumHeatCapacity { handle, amount } => {
				(16, 0, handle, zero, [amount, z, z], 0, 0)
			}
			Self::Clear { handle } => (17, 0, handle, zero, [z, z, z], 0, 0),
			Self::Add { handle, amount } => (18, 0, handle, zero, [amount, z, z], 0, 0),
			Self::Multiply { handle, factor } => (19, 0, handle, zero, [factor, z, z], 0, 0),
			Self::CopyFrom { receiver, giver } => (20, 0, receiver, giver, [z, z, z], 0, 0),
			Self::AdjustHeat { handle, heat } => (21, 0, handle, zero, [heat, z, z], 0, 0),
			Self::Compare { left, right } => (22, 0, left, right, [z, z, z], 0, 0),
			Self::EqualizeWith { receiver, total } => (23, 0, receiver, total, [z, z, z], 0, 0),
			Self::TemperatureShare {
				first,
				second,
				conduction_coefficient,
			} => (24, 0, first, second, [conduction_coefficient, z, z], 0, 0),
			Self::TemperatureShareNonGas {
				handle,
				conduction_coefficient,
				sharer_temperature,
				sharer_heat_capacity,
			} => (
				25,
				0,
				handle,
				zero,
				[
					conduction_coefficient,
					sharer_temperature,
					sharer_heat_capacity,
				],
				0,
				0,
			),
			Self::MarkImmutable { handle } => (26, 0, handle, zero, [z, z, z], 0, 0),
			Self::IsImmutable { handle } => (27, 0, handle, zero, [z, z, z], 0, 0),
			Self::Merge { receiver, giver } => (28, 0, receiver, giver, [z, z, z], 0, 0),
			Self::RemoveRatioInto {
				source,
				destination,
				ratio,
			} => (29, 0, source, destination, [ratio, z, z], 0, 0),
			Self::RemoveAmountInto {
				source,
				destination,
				amount,
			} => (30, 0, source, destination, [amount, z, z], 0, 0),
			Self::TransferGases {
				source,
				destination,
				ratio,
				gas_mask,
			} => (31, 0, source, destination, [ratio, z, z], 0, gas_mask),
			Self::TransferAmount {
				source,
				destination,
				amount,
			} => (32, 0, source, destination, [amount, z, z], 0, 0),
			Self::TransferRatio {
				source,
				destination,
				ratio,
			} => (33, 0, source, destination, [ratio, z, z], 0, 0),
			Self::TransferByFlags {
				source,
				destination,
				flags,
				amount,
			} => (34, 0, source, destination, [amount, z, z], 0, flags),
			Self::ShareRatio {
				first,
				second,
				ratio,
				one_way,
			} => (35, u16::from(one_way), first, second, [ratio, z, z], 0, 0),
			Self::React {
				handle,
				target,
				reaction_profile_threshold_ms,
			} => {
				let threshold = reaction_profile_threshold_ms.unwrap_or(z);
				if threshold.0 < 0.0 {
					return Err(ProtocolError::InvalidReactionProfileThreshold);
				}
				(
					36,
					u16::from(reaction_profile_threshold_ms.is_some()),
					handle,
					target,
					[threshold, z, z],
					0,
					0,
				)
			}
		};
		let mut output = [0_u8; MIXTURE_COMMAND_REQUEST_LEN];
		output[0..2].copy_from_slice(&kind.to_le_bytes());
		output[2..4].copy_from_slice(&flags.to_le_bytes());
		output[4..12].copy_from_slice(&primary.encode());
		output[12..20].copy_from_slice(&secondary.encode());
		for (index, scalar) in scalars.into_iter().enumerate() {
			let offset = 20 + index * 8;
			output[offset..offset + 8].copy_from_slice(&scalar.encode()?);
		}
		output[44..46].copy_from_slice(&gas_id.to_le_bytes());
		output[48..52].copy_from_slice(&aux.to_le_bytes());
		Ok(output)
	}

	pub fn decode(input: &[u8]) -> Result<Self, ProtocolError> {
		require_exact_len(input, MIXTURE_COMMAND_REQUEST_LEN)?;
		let kind = read_u16(input, 0);
		let flags = read_u16(input, 2);
		if !(1..=36).contains(&kind) {
			return Err(ProtocolError::UnknownMixtureCommand(kind));
		}
		if read_u16(input, 46) != 0 || read_u32(input, 52) != 0 {
			return Err(ProtocolError::ReservedMixtureCommandField);
		}
		let allowed_flags = if kind == 13 || kind == 35 || kind == 36 {
			1
		} else {
			0
		};
		if flags & !allowed_flags != 0 {
			return Err(ProtocolError::UnknownMixtureCommandFlags { kind, flags });
		}
		let primary = WireHandle::decode(&input[4..12])?;
		let secondary = WireHandle::decode(&input[12..20])?;
		let s1 = ScalarValue::decode(&input[20..28])?;
		let s2 = ScalarValue::decode(&input[28..36])?;
		let s3 = ScalarValue::decode(&input[36..44])?;
		let gas_id = read_u16(input, 44);
		let aux = read_u32(input, 48);
		let uses_secondary = matches!(kind, 20 | 22..=24 | 28..=36);
		let uses_s1 = matches!(kind, 1..=3 | 14..=16 | 18..=19 | 21 | 24..=25 | 29..=35)
			|| matches!(kind, 13 | 36) && flags & 1 != 0;
		let uses_s2 = matches!(kind, 3 | 25);
		let uses_s3 = kind == 25;
		let uses_gas_id = matches!(kind, 1..=4 | 8);
		let uses_aux = matches!(kind, 12 | 31 | 34);
		if (!uses_secondary && input[12..20].iter().any(|byte| *byte != 0))
			|| (!uses_s1 && input[20..28].iter().any(|byte| *byte != 0))
			|| (!uses_s2 && input[28..36].iter().any(|byte| *byte != 0))
			|| (!uses_s3 && input[36..44].iter().any(|byte| *byte != 0))
			|| (!uses_gas_id && gas_id != 0)
			|| (!uses_aux && aux != 0)
		{
			return Err(ProtocolError::ReservedMixtureCommandField);
		}
		match kind {
			1 => Ok(Self::SetMoles {
				handle: primary,
				gas_id,
				amount: s1,
			}),
			2 => Ok(Self::AdjustMoles {
				handle: primary,
				gas_id,
				delta: s1,
			}),
			3 => Ok(Self::AdjustMolesTemperature {
				handle: primary,
				gas_id,
				amount: s1,
				temperature: s2,
			}),
			4 => Ok(Self::GetMoles {
				handle: primary,
				gas_id,
			}),
			5 => Ok(Self::Temperature { handle: primary }),
			6 => Ok(Self::Volume { handle: primary }),
			7 => Ok(Self::HeatCapacity { handle: primary }),
			8 => Ok(Self::PartialHeatCapacity {
				handle: primary,
				gas_id,
			}),
			9 => Ok(Self::TotalMoles { handle: primary }),
			10 => Ok(Self::Pressure { handle: primary }),
			11 => Ok(Self::ThermalEnergy { handle: primary }),
			12 => Ok(Self::GetMolesByFlags {
				handle: primary,
				flags: aux,
			}),
			13 => Ok(Self::Burnability {
				handle: primary,
				temperature: (flags & 1 != 0).then_some(s1),
			}),
			14 => Ok(Self::SetTemperature {
				handle: primary,
				temperature: s1,
			}),
			15 => Ok(Self::SetVolume {
				handle: primary,
				volume: s1,
			}),
			16 => Ok(Self::SetMinimumHeatCapacity {
				handle: primary,
				amount: s1,
			}),
			17 => Ok(Self::Clear { handle: primary }),
			18 => Ok(Self::Add {
				handle: primary,
				amount: s1,
			}),
			19 => Ok(Self::Multiply {
				handle: primary,
				factor: s1,
			}),
			20 => Ok(Self::CopyFrom {
				receiver: primary,
				giver: secondary,
			}),
			21 => Ok(Self::AdjustHeat {
				handle: primary,
				heat: s1,
			}),
			22 => Ok(Self::Compare {
				left: primary,
				right: secondary,
			}),
			23 => Ok(Self::EqualizeWith {
				receiver: primary,
				total: secondary,
			}),
			24 => Ok(Self::TemperatureShare {
				first: primary,
				second: secondary,
				conduction_coefficient: s1,
			}),
			25 => Ok(Self::TemperatureShareNonGas {
				handle: primary,
				conduction_coefficient: s1,
				sharer_temperature: s2,
				sharer_heat_capacity: s3,
			}),
			26 => Ok(Self::MarkImmutable { handle: primary }),
			27 => Ok(Self::IsImmutable { handle: primary }),
			28 => Ok(Self::Merge {
				receiver: primary,
				giver: secondary,
			}),
			29 => Ok(Self::RemoveRatioInto {
				source: primary,
				destination: secondary,
				ratio: s1,
			}),
			30 => Ok(Self::RemoveAmountInto {
				source: primary,
				destination: secondary,
				amount: s1,
			}),
			31 => Ok(Self::TransferGases {
				source: primary,
				destination: secondary,
				ratio: s1,
				gas_mask: aux,
			}),
			32 => Ok(Self::TransferAmount {
				source: primary,
				destination: secondary,
				amount: s1,
			}),
			33 => Ok(Self::TransferRatio {
				source: primary,
				destination: secondary,
				ratio: s1,
			}),
			34 => Ok(Self::TransferByFlags {
				source: primary,
				destination: secondary,
				flags: aux,
				amount: s1,
			}),
			35 => Ok(Self::ShareRatio {
				first: primary,
				second: secondary,
				ratio: s1,
				one_way: flags & 1 != 0,
			}),
			36 => {
				if flags & 1 != 0 && s1.0 < 0.0 {
					return Err(ProtocolError::InvalidReactionProfileThreshold);
				}
				Ok(Self::React {
					handle: primary,
					target: secondary,
					reaction_profile_threshold_ms: (flags & 1 != 0).then_some(s1),
				})
			}
			actual => Err(ProtocolError::UnknownMixtureCommand(actual)),
		}
	}
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum MixtureCommandResponse {
	Applied {
		updated: u32,
	},
	Scalar(ScalarValue),
	Scalars([ScalarValue; 2]),
	Boolean(bool),
	ReactionProgress {
		flags: u32,
		work_items: u32,
		pending: bool,
	},
}

impl MixtureCommandResponse {
	pub fn encode(self) -> Result<[u8; MIXTURE_COMMAND_RESPONSE_LEN], ProtocolError> {
		let (kind, value, payload) = match self {
			Self::Applied { updated } => (1_u32, updated, [0_u8; 16]),
			Self::Scalar(scalar) => {
				let mut payload = [0_u8; 16];
				payload[..8].copy_from_slice(&scalar.encode()?);
				(2, 0, payload)
			}
			Self::Scalars(scalars) => {
				let mut payload = [0_u8; 16];
				payload[..8].copy_from_slice(&scalars[0].encode()?);
				payload[8..].copy_from_slice(&scalars[1].encode()?);
				(3, 0, payload)
			}
			Self::Boolean(value) => (4, u32::from(value), [0_u8; 16]),
			Self::ReactionProgress {
				flags,
				work_items,
				pending,
			} => {
				if flags & !REACTION_FLAGS != 0 {
					return Err(ProtocolError::InvalidReactionFlags(flags));
				}
				let mut payload = [0_u8; 16];
				payload[..4].copy_from_slice(&work_items.to_le_bytes());
				payload[4..8].copy_from_slice(&u32::from(pending).to_le_bytes());
				(5, flags, payload)
			}
		};
		let mut output = [0_u8; MIXTURE_COMMAND_RESPONSE_LEN];
		output[0..4].copy_from_slice(&kind.to_le_bytes());
		output[4..8].copy_from_slice(&value.to_le_bytes());
		output[8..24].copy_from_slice(&payload);
		Ok(output)
	}

	pub fn decode(input: &[u8]) -> Result<Self, ProtocolError> {
		require_exact_len(input, MIXTURE_COMMAND_RESPONSE_LEN)?;
		let kind = read_u32(input, 0);
		let value = read_u32(input, 4);
		let scalars = [
			ScalarValue::decode(&input[8..16])?,
			ScalarValue::decode(&input[16..24])?,
		];
		match kind {
			1 if input[8..24].iter().all(|byte| *byte == 0) => Ok(Self::Applied { updated: value }),
			2 if value == 0 && input[16..24].iter().all(|byte| *byte == 0) => {
				Ok(Self::Scalar(scalars[0]))
			}
			3 if value == 0 => Ok(Self::Scalars(scalars)),
			4 if input[8..24].iter().all(|byte| *byte == 0) => {
				Ok(Self::Boolean(decode_boolean(value)?))
			}
			5 => {
				if value & !REACTION_FLAGS != 0 || input[16..24].iter().any(|byte| *byte != 0) {
					return Err(ProtocolError::InvalidReactionFlags(value));
				}
				Ok(Self::ReactionProgress {
					flags: value,
					work_items: read_u32(input, 8),
					pending: decode_boolean(read_u32(input, 12))?,
				})
			}
			actual => Err(ProtocolError::UnknownMixtureCommandResponse(actual)),
		}
	}
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
	ProcessReactions = 5,
}

impl TryFrom<u32> for SimulationStage {
	type Error = ProtocolError;

	fn try_from(value: u32) -> Result<Self, Self::Error> {
		match value {
			1 => Ok(Self::ProcessExcitedGroups),
			2 => Ok(Self::ProcessTurfEqualize),
			3 => Ok(Self::ProcessTurfHeat),
			4 => Ok(Self::ProcessTurfs),
			5 => Ok(Self::ProcessReactions),
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
	InvalidContinuationCapacity {
		actual: u32,
		maximum: u32,
	},
	AuthenticationFailed,
	BuildIdentityMismatch,
	CapacityMismatch,
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
	MissingContinuationToken,
	UnexpectedContinuationToken,
	InvalidBoolean(u32),
	NonZeroAbsentHandle,
	UnexpectedUnregisterMixture,
	FirelockOnDisconnectedEdge,
	DuplicateTurfAdjacency {
		left: u32,
		right: u32,
	},
	UnknownTurfHeatFlags(u32),
	ReservedTurfHeatField(u32),
	NonZeroAbsentTurfHeatState,
	UnknownMixtureCommand(u16),
	UnknownMixtureCommandFlags {
		kind: u16,
		flags: u16,
	},
	ReservedMixtureCommandField,
	ReservedMixtureAdjustmentField,
	ReservedContinuationField(u32),
	InvalidContinuationId,
	InvalidContinuationDeadline,
	InvalidReactionFlags(u32),
	InvalidReactionProfileThreshold,
	UnknownMixtureCommandResponse(u32),
	UnknownMixtureSnapshotFlags(u32),
	ReservedMixtureSnapshotField(u32),
	MetadataStringTooLong {
		actual: u32,
		maximum: u32,
	},
	InvalidMetadataUtf8,
	UnknownGasFireRole(u16),
	UnknownFireProducts(u32),
	UnknownMetadataFlags(u32),
	NonZeroMetadataPadding,
	UnknownReactionExecution(u16),
	UnknownServiceProcessFlags(u32),
	ReservedServiceTelemetryField(u32),
	NonZeroUnavailableServiceProcessMetric,
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

fn decode_boolean(value: u32) -> Result<bool, ProtocolError> {
	match value {
		0 => Ok(false),
		1 => Ok(true),
		actual => Err(ProtocolError::InvalidBoolean(actual)),
	}
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
