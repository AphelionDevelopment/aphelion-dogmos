use crate::{ProtocolError, ProtocolHeader, PROTOCOL_HEADER_LEN};
use std::{fmt, io, io::Read, io::Write};

#[derive(Debug)]
pub enum TransportError {
	Io(io::Error),
	Protocol(ProtocolError),
	PayloadLengthMismatch { header: u32, actual: usize },
	BufferTooSmall { required: usize, available: usize },
}

impl fmt::Display for TransportError {
	fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
		write!(formatter, "{self:?}")
	}
}

impl std::error::Error for TransportError {
	fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
		match self {
			Self::Io(error) => Some(error),
			Self::Protocol(error) => Some(error),
			_ => None,
		}
	}
}

impl From<io::Error> for TransportError {
	fn from(error: io::Error) -> Self {
		Self::Io(error)
	}
}

impl From<ProtocolError> for TransportError {
	fn from(error: ProtocolError) -> Self {
		Self::Protocol(error)
	}
}

pub fn write_frame(
	writer: &mut impl Write,
	header: ProtocolHeader,
	payload: &[u8],
) -> Result<(), TransportError> {
	if payload.len() != header.payload_len as usize {
		return Err(TransportError::PayloadLengthMismatch {
			header: header.payload_len,
			actual: payload.len(),
		});
	}
	let header_bytes = header.encode();
	// Self-check that the header we are about to put on the wire round-trips, then reuse those
	// same bytes rather than encoding a second time for the write.
	ProtocolHeader::decode(&header_bytes)?;
	/*
		Control frames are overwhelmingly small - an eight byte scalar, an empty acknowledgement,
		a fixed-size snapshot - and the stream underneath is unbuffered, so splitting every frame
		into a header write plus a payload write doubles the syscall count on exactly the frames
		where the round trip is pure latency. Coalesce into one stack buffer when the payload is
		small, and fall back to two writes for bulk frames, where the second write is amortized
		over the payload anyway.
	*/
	const COALESCE_PAYLOAD_LIMIT: usize = 512;
	if payload.len() <= COALESCE_PAYLOAD_LIMIT {
		let mut frame = [0_u8; PROTOCOL_HEADER_LEN as usize + COALESCE_PAYLOAD_LIMIT];
		let frame_len = header_bytes.len() + payload.len();
		frame[..header_bytes.len()].copy_from_slice(&header_bytes);
		frame[header_bytes.len()..frame_len].copy_from_slice(payload);
		writer.write_all(&frame[..frame_len])?;
	} else {
		writer.write_all(&header_bytes)?;
		writer.write_all(payload)?;
	}
	writer.flush()?;
	Ok(())
}

pub fn read_frame_into(
	reader: &mut impl Read,
	payload_buffer: &mut [u8],
) -> Result<(ProtocolHeader, usize), TransportError> {
	let mut header_bytes = [0_u8; PROTOCOL_HEADER_LEN as usize];
	reader.read_exact(&mut header_bytes)?;
	let header = ProtocolHeader::decode(&header_bytes)?;
	let payload_len = header.payload_len as usize;
	if payload_len > payload_buffer.len() {
		return Err(TransportError::BufferTooSmall {
			required: payload_len,
			available: payload_buffer.len(),
		});
	}
	reader.read_exact(&mut payload_buffer[..payload_len])?;
	Ok((header, payload_len))
}
