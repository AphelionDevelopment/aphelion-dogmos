use dogmos_identity::{BuildMetadata, BuildMetadataError, MetadataField};

const SOURCE_REVISION: &str = "0123456789abcdef0123456789abcdef01234567";
const FEATURE_FINGERPRINT: &str =
	"89abcdef0123456789abcdef0123456789abcdef0123456789abcdef01234567";

#[test]
fn parses_exact_hex_build_metadata() {
	let metadata = BuildMetadata::parse(SOURCE_REVISION, FEATURE_FINGERPRINT).unwrap();
	assert_eq!(
		metadata.source_revision,
		[
			0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef, 0x01, 0x23, 0x45, 0x67, 0x89, 0xab,
			0xcd, 0xef, 0x01, 0x23, 0x45, 0x67,
		]
	);
	assert_eq!(
		metadata.feature_fingerprint[0..8],
		[0x89, 0xab, 0xcd, 0xef, 0x01, 0x23, 0x45, 0x67,]
	);
}

#[test]
fn rejects_wrong_length_and_non_hex_metadata() {
	assert_eq!(
		BuildMetadata::parse("0123", FEATURE_FINGERPRINT),
		Err(BuildMetadataError::InvalidLength {
			field: MetadataField::SourceRevision,
			expected: 40,
			actual: 4,
		})
	);
	assert_eq!(
		BuildMetadata::parse(
			"g123456789abcdef0123456789abcdef01234567",
			FEATURE_FINGERPRINT,
		),
		Err(BuildMetadataError::InvalidHex {
			field: MetadataField::SourceRevision,
			index: 0,
		})
	);
}

#[test]
fn rejects_empty_identity_components() {
	assert_eq!(
		BuildMetadata::parse(&"0".repeat(40), FEATURE_FINGERPRINT),
		Err(BuildMetadataError::Empty(MetadataField::SourceRevision))
	);
	assert_eq!(
		BuildMetadata::parse(SOURCE_REVISION, &"0".repeat(64)),
		Err(BuildMetadataError::Empty(MetadataField::FeatureFingerprint))
	);
}

#[test]
fn reports_missing_required_metadata() {
	assert_eq!(
		BuildMetadata::parse_required(None, Some(FEATURE_FINGERPRINT)),
		Err(BuildMetadataError::Missing(MetadataField::SourceRevision))
	);
	assert_eq!(
		BuildMetadata::parse_required(Some(SOURCE_REVISION), None),
		Err(BuildMetadataError::Missing(
			MetadataField::FeatureFingerprint
		))
	);
}
