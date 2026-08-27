#![cfg(windows)]

use dogmos_identity::{sha256_file, sha256_reader};
use std::{fs, io::Cursor};

const ABC_SHA256: [u8; 32] = [
	0xba, 0x78, 0x16, 0xbf, 0x8f, 0x01, 0xcf, 0xea, 0x41, 0x41, 0x40, 0xde, 0x5d, 0xae, 0x22, 0x23,
	0xb0, 0x03, 0x61, 0xa3, 0x96, 0x17, 0x7a, 0x9c, 0xb4, 0x10, 0xff, 0x61, 0xf2, 0x00, 0x15, 0xad,
];

#[test]
fn hashes_a_known_stream_without_buffering_the_whole_input() {
	assert_eq!(sha256_reader(Cursor::new(b"abc")).unwrap(), ABC_SHA256);
}

#[test]
fn file_hash_matches_stream_hash() {
	let path = std::env::temp_dir().join(format!("dogmos-sha256-{}.tmp", std::process::id()));
	fs::write(&path, b"abc").unwrap();
	let result = sha256_file(&path);
	let _ = fs::remove_file(&path);
	assert_eq!(result.unwrap(), ABC_SHA256);
}
