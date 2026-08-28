use super::*;

pub const CONTINUATION_TOKEN_LEN: usize = 24;
pub const CONTINUATION_COMMAND_REQUEST_LEN: usize =
	CONTINUATION_TOKEN_LEN + MIXTURE_COMMAND_REQUEST_LEN;
pub const CONTINUATION_TICK_MILLIS: u64 = 100;
pub const DEFAULT_CONTINUATION_TIMEOUT_TICKS: u64 = 50;
pub const MAX_PENDING_CONTINUATIONS: u32 = 65_536;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(C)]
pub struct ContinuationToken {
	pub world_generation: u32,
	pub id: u64,
	pub deadline_ticks: u64,
}

impl ContinuationToken {
	pub fn encode(self) -> Result<[u8; CONTINUATION_TOKEN_LEN], ProtocolError> {
		if self.id == 0 {
			return Err(ProtocolError::InvalidContinuationId);
		}
		if self.deadline_ticks == 0 {
			return Err(ProtocolError::InvalidContinuationDeadline);
		}
		let mut output = [0_u8; CONTINUATION_TOKEN_LEN];
		output[0..4].copy_from_slice(&self.world_generation.to_le_bytes());
		output[8..16].copy_from_slice(&self.id.to_le_bytes());
		output[16..24].copy_from_slice(&self.deadline_ticks.to_le_bytes());
		Ok(output)
	}

	pub fn decode(input: &[u8]) -> Result<Self, ProtocolError> {
		require_exact_len(input, CONTINUATION_TOKEN_LEN)?;
		let reserved = read_u32(input, 4);
		if reserved != 0 {
			return Err(ProtocolError::ReservedContinuationField(reserved));
		}
		let token = Self {
			world_generation: read_u32(input, 0),
			id: read_u64(input, 8),
			deadline_ticks: read_u64(input, 16),
		};
		token.encode()?;
		Ok(token)
	}
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ContinuationCommandRequest {
	pub token: ContinuationToken,
	pub command: MixtureCommandRequest,
}

impl ContinuationCommandRequest {
	pub fn encode(self) -> Result<[u8; CONTINUATION_COMMAND_REQUEST_LEN], ProtocolError> {
		let mut output = [0_u8; CONTINUATION_COMMAND_REQUEST_LEN];
		output[..CONTINUATION_TOKEN_LEN].copy_from_slice(&self.token.encode()?);
		output[CONTINUATION_TOKEN_LEN..].copy_from_slice(&self.command.encode()?);
		Ok(output)
	}

	pub fn decode(input: &[u8]) -> Result<Self, ProtocolError> {
		require_exact_len(input, CONTINUATION_COMMAND_REQUEST_LEN)?;
		Ok(Self {
			token: ContinuationToken::decode(&input[..CONTINUATION_TOKEN_LEN])?,
			command: MixtureCommandRequest::decode(&input[CONTINUATION_TOKEN_LEN..])?,
		})
	}
}

pub fn encode_continuation_adjust_multiple_request(
	token: ContinuationToken,
	handle: WireHandle,
	adjustments: &[MixtureAdjustment],
	output: &mut Vec<u8>,
) -> Result<(), ProtocolError> {
	encode_adjust_multiple_request(handle, adjustments, output)?;
	let nested_len = output.len();
	output.resize(CONTINUATION_TOKEN_LEN + nested_len, 0);
	output.copy_within(0..nested_len, CONTINUATION_TOKEN_LEN);
	output[..CONTINUATION_TOKEN_LEN].copy_from_slice(&token.encode()?);
	Ok(())
}

pub fn decode_continuation_adjust_multiple_request(
	input: &[u8],
) -> Result<(ContinuationToken, WireHandle, Vec<MixtureAdjustment>), ProtocolError> {
	if input.len() < CONTINUATION_TOKEN_LEN {
		return Err(ProtocolError::InvalidPayloadLength {
			expected: CONTINUATION_TOKEN_LEN as u32,
			actual: input.len() as u32,
		});
	}
	let token = ContinuationToken::decode(&input[..CONTINUATION_TOKEN_LEN])?;
	let (handle, adjustments) = decode_adjust_multiple_request(&input[CONTINUATION_TOKEN_LEN..])?;
	Ok((token, handle, adjustments))
}
