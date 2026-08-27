use std::{
	fmt,
	fs::File,
	io::{self, Read},
	path::Path,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MetadataField {
	SourceRevision,
	FeatureFingerprint,
}

impl fmt::Display for MetadataField {
	fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
		match self {
			Self::SourceRevision => formatter.write_str("source revision"),
			Self::FeatureFingerprint => formatter.write_str("feature fingerprint"),
		}
	}
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BuildMetadataError {
	Missing(MetadataField),
	InvalidLength {
		field: MetadataField,
		expected: usize,
		actual: usize,
	},
	InvalidHex {
		field: MetadataField,
		index: usize,
	},
	Empty(MetadataField),
}

impl fmt::Display for BuildMetadataError {
	fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
		match self {
			Self::Missing(field) => write!(formatter, "{field} is required at compile time"),
			Self::InvalidLength {
				field,
				expected,
				actual,
			} => write!(
				formatter,
				"{field} must contain exactly {expected} hexadecimal characters, got {actual}",
			),
			Self::InvalidHex { field, index } => {
				write!(
					formatter,
					"{field} contains invalid hexadecimal at index {index}"
				)
			}
			Self::Empty(field) => write!(formatter, "{field} must not be all zero"),
		}
	}
}

impl std::error::Error for BuildMetadataError {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BuildMetadata {
	pub source_revision: [u8; 20],
	pub feature_fingerprint: [u8; 32],
}

impl BuildMetadata {
	pub fn from_compile_environment() -> Result<Self, BuildMetadataError> {
		Self::parse_required(
			option_env!("DOGMOS_SOURCE_REVISION"),
			option_env!("DOGMOS_FEATURE_FINGERPRINT"),
		)
	}

	pub fn parse_required(
		source_revision: Option<&str>,
		feature_fingerprint: Option<&str>,
	) -> Result<Self, BuildMetadataError> {
		Self::parse(
			source_revision.ok_or(BuildMetadataError::Missing(MetadataField::SourceRevision))?,
			feature_fingerprint.ok_or(BuildMetadataError::Missing(
				MetadataField::FeatureFingerprint,
			))?,
		)
	}

	pub fn parse(
		source_revision: &str,
		feature_fingerprint: &str,
	) -> Result<Self, BuildMetadataError> {
		Ok(Self {
			source_revision: decode_hex(source_revision, MetadataField::SourceRevision)?,
			feature_fingerprint: decode_hex(
				feature_fingerprint,
				MetadataField::FeatureFingerprint,
			)?,
		})
	}
}

fn decode_hex<const N: usize>(
	input: &str,
	field: MetadataField,
) -> Result<[u8; N], BuildMetadataError> {
	let expected = N * 2;
	if input.len() != expected {
		return Err(BuildMetadataError::InvalidLength {
			field,
			expected,
			actual: input.len(),
		});
	}
	let bytes = input.as_bytes();
	let mut output = [0_u8; N];
	for (index, output_byte) in output.iter_mut().enumerate() {
		let high_index = index * 2;
		let low_index = high_index + 1;
		let high = hex_digit(bytes[high_index]).ok_or(BuildMetadataError::InvalidHex {
			field,
			index: high_index,
		})?;
		let low = hex_digit(bytes[low_index]).ok_or(BuildMetadataError::InvalidHex {
			field,
			index: low_index,
		})?;
		*output_byte = (high << 4) | low;
	}
	if output == [0; N] {
		return Err(BuildMetadataError::Empty(field));
	}
	Ok(output)
}

fn hex_digit(byte: u8) -> Option<u8> {
	match byte {
		b'0'..=b'9' => Some(byte - b'0'),
		b'a'..=b'f' => Some(byte - b'a' + 10),
		b'A'..=b'F' => Some(byte - b'A' + 10),
		_ => None,
	}
}

pub fn sha256_file(path: &Path) -> io::Result<[u8; 32]> {
	sha256_reader(File::open(path)?)
}

#[cfg(windows)]
pub fn sha256_reader(mut reader: impl Read) -> io::Result<[u8; 32]> {
	use std::ptr;
	use windows_sys::Win32::Security::Cryptography::{
		BCryptCloseAlgorithmProvider, BCryptCreateHash, BCryptDestroyHash, BCryptFinishHash,
		BCryptGetProperty, BCryptHashData, BCryptOpenAlgorithmProvider, BCRYPT_ALG_HANDLE,
		BCRYPT_HASH_HANDLE, BCRYPT_OBJECT_LENGTH, BCRYPT_SHA256_ALGORITHM,
	};

	struct Algorithm(BCRYPT_ALG_HANDLE);
	impl Drop for Algorithm {
		fn drop(&mut self) {
			// SAFETY: the handle was returned by BCryptOpenAlgorithmProvider and is owned here.
			unsafe {
				BCryptCloseAlgorithmProvider(self.0, 0);
			}
		}
	}

	struct Hash(BCRYPT_HASH_HANDLE);
	impl Drop for Hash {
		fn drop(&mut self) {
			// SAFETY: the handle was returned by BCryptCreateHash and is owned here.
			unsafe {
				BCryptDestroyHash(self.0);
			}
		}
	}

	let mut algorithm = ptr::null_mut();
	// SAFETY: output points to initialized writable storage; both PCWSTR inputs are valid constants.
	nt_success(unsafe {
		BCryptOpenAlgorithmProvider(&mut algorithm, BCRYPT_SHA256_ALGORITHM, ptr::null(), 0)
	})?;
	let algorithm = Algorithm(algorithm);

	let mut object_len = 0_u32;
	let mut property_len = 0_u32;
	// SAFETY: the algorithm handle is live and the output buffer is exactly one u32.
	nt_success(unsafe {
		BCryptGetProperty(
			algorithm.0,
			BCRYPT_OBJECT_LENGTH,
			ptr::from_mut(&mut object_len).cast(),
			std::mem::size_of::<u32>() as u32,
			&mut property_len,
			0,
		)
	})?;
	if property_len != std::mem::size_of::<u32>() as u32 || object_len == 0 {
		return Err(io::Error::other(
			"BCrypt returned an invalid SHA-256 object length",
		));
	}

	let mut object = vec![0_u8; object_len as usize];
	let mut hash = ptr::null_mut();
	// SAFETY: all handles and buffers remain live for the call and lengths match their allocations.
	nt_success(unsafe {
		BCryptCreateHash(
			algorithm.0,
			&mut hash,
			object.as_mut_ptr(),
			object_len,
			ptr::null(),
			0,
			0,
		)
	})?;
	let hash = Hash(hash);

	let mut buffer = [0_u8; 16 * 1024];
	loop {
		let read = reader.read(&mut buffer)?;
		if read == 0 {
			break;
		}
		// SAFETY: the hash handle is live and the input slice is valid for exactly `read` bytes.
		nt_success(unsafe { BCryptHashData(hash.0, buffer.as_ptr(), read as u32, 0) })?;
	}

	let mut digest = [0_u8; 32];
	// SAFETY: the hash handle is live and digest is a writable 32-byte SHA-256 output buffer.
	nt_success(unsafe { BCryptFinishHash(hash.0, digest.as_mut_ptr(), digest.len() as u32, 0) })?;
	Ok(digest)
}

#[cfg(not(windows))]
pub fn sha256_reader(_reader: impl Read) -> io::Result<[u8; 32]> {
	Err(io::Error::new(
		io::ErrorKind::Unsupported,
		"Dogmos executable hashing currently requires Windows CNG",
	))
}

#[cfg(windows)]
fn nt_success(status: windows_sys::Win32::Foundation::NTSTATUS) -> io::Result<()> {
	if status >= 0 {
		Ok(())
	} else {
		Err(io::Error::other(format!(
			"Windows CNG failed with NTSTATUS {status:#x}"
		)))
	}
}
