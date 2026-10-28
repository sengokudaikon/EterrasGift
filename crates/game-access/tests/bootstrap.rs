use eg_game_access::bootstrap::{BootstrapConfig, BootstrapReply, CONFIG_SIZE, HelperArtifact, HelperLayout};

const SOURCE: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
fn set16(bytes: &mut [u8], at: usize, v: u16) {
    bytes[at..at + 2].copy_from_slice(&v.to_le_bytes())
}
fn set32(bytes: &mut [u8], at: usize, v: u32) {
    bytes[at..at + 4].copy_from_slice(&v.to_le_bytes())
}
fn image() -> Vec<u8> {
    let mut bytes = vec![0; 0xc00];
    bytes[..2].copy_from_slice(b"MZ");
    set32(&mut bytes, 0x3c, 0x80);
    bytes[0x80..0x84].copy_from_slice(b"PE\0\0");
    set16(&mut bytes, 0x84, 0x8664);
    set16(&mut bytes, 0x86, 1);
    set16(&mut bytes, 0x94, 240);
    set16(&mut bytes, 0x96, 0x2000);
    set16(&mut bytes, 0x98, 0x20b);
    set32(&mut bytes, 0xd0, 0x2000);
    set32(&mut bytes, 0x104, 16);
    set32(&mut bytes, 0x108, 0x1100);
    set32(&mut bytes, 0x10c, 0x180);
    // One file-backed executable section. The file extends exactly to its raw end.
    set32(&mut bytes, 0x188 + 8, 0x800);
    set32(&mut bytes, 0x188 + 12, 0x1000);
    set32(&mut bytes, 0x188 + 16, 0x800);
    set32(&mut bytes, 0x188 + 20, 0x400);
    set32(&mut bytes, 0x188 + 36, 0x6000_0020);
    let dir = 0x500;
    set32(&mut bytes, dir + 20, 2);
    set32(&mut bytes, dir + 24, 2);
    set32(&mut bytes, dir + 28, 0x1140);
    set32(&mut bytes, dir + 32, 0x1150);
    set32(&mut bytes, dir + 36, 0x1160);
    set32(&mut bytes, 0x540, 0x1000);
    set32(&mut bytes, 0x544, 0x1020);
    set32(&mut bytes, 0x550, 0x1170);
    set32(&mut bytes, 0x554, 0x1190);
    set16(&mut bytes, 0x560, 0);
    set16(&mut bytes, 0x562, 1);
    bytes[0x570..0x584].copy_from_slice(b"eg_helper_bootstrap\0");
    bytes[0x590..0x5a3].copy_from_slice(b"eg_helper_build_id\0");
    bytes[0x400..0x410].fill(0xc3);
    bytes[0x420..0x423].copy_from_slice(&[0x48, 0x8d, 0x05]);
    set32(&mut bytes, 0x423, 0x1400 - (0x1020 + 7));
    bytes[0x427] = 0xc3;
    bytes[0x800..0x840].copy_from_slice(SOURCE.as_bytes());
    bytes
}

#[test]
fn configuration_matches_independent_native_layout_and_strict_reply_contract() {
    let config = BootstrapConfig::new(0x11223344, SOURCE).unwrap();
    let mut expected = [0; 96];
    expected[..16].copy_from_slice(&[0x53, 0x42, 0x47, 0x45, 96, 0, 0, 0, 0x44, 0x33, 0x22, 0x11, 255, 255, 255, 255]);
    expected[16..80].copy_from_slice(SOURCE.as_bytes());
    assert_eq!(config.bytes(), &expected);
    expected[12..16].fill(0);
    assert_eq!(config.decode_reply(0, &expected).unwrap(), BootstrapReply::Started);
    for at in (0..12).chain(16..96) {
        let mut bad = expected;
        bad[at] ^= 1;
        assert!(config.decode_reply(0, &bad).is_err(), "byte {at}")
    }
    assert!(config.decode_reply(1, &expected).is_err());
    expected[12] = 1;
    assert!(config.decode_reply(0, &expected).is_err());
    for len in 0..CONFIG_SIZE {
        assert!(config.decode_reply(0, &expected[..len]).is_err())
    }
    for (pid, id) in [(0, SOURCE), (1, "abc"), (1, "G123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef")] {
        assert!(BootstrapConfig::new(pid, id).is_err())
    }
}

#[test]
fn pure_pe_parser_resolves_exact_nonforwarded_exports_and_compiled_source_without_loading() {
    let bytes = image();
    let layout = HelperLayout::parse(&bytes, SOURCE).unwrap();
    assert_eq!(layout.bootstrap_rva, 0x1000);
    assert_eq!(layout.image_size, 0x2000);
    for len in 0..bytes.len() {
        assert!(HelperLayout::parse(&bytes[..len], SOURCE).is_err(), "prefix {len}")
    }
    assert!(HelperLayout::parse(&bytes, &"a".repeat(64)).is_err());
}

#[test]
fn malformed_exports_rva_storage_and_getter_adapters_never_become_bootstrap_entries() {
    for fault in 0..12 {
        let mut bytes = image();
        match fault {
            0 => set16(&mut bytes, 0x84, 0x14c),
            1 => set16(&mut bytes, 0x96, 0),
            2 => set32(&mut bytes, 0x188 + 36, 0x40000040),
            3 => set32(&mut bytes, 0x540, 0x1170),
            4 => set16(&mut bytes, 0x562, 2),
            5 => set32(&mut bytes, 0x500 + 20, 4097),
            6 => set32(&mut bytes, 0x500 + 24, u32::MAX),
            7 => set32(&mut bytes, 0x550, u32::MAX),
            8 => bytes[0x427] = 0xcc,
            9 => set32(&mut bytes, 0x423, 0x7fffffff),
            10 => bytes[0x800] = b'z',
            11 => bytes[0x840] = 1,
            _ => unreachable!(),
        }
        assert!(HelperLayout::parse(&bytes, SOURCE).is_err(), "fault {fault}");
    }
}

#[test]
fn partially_overlapping_mapped_sections_are_rejected_even_when_only_one_covers_full_code() {
    let mut bytes = image();
    set16(&mut bytes, 0x86, 2);
    let second = 0x188 + 40;
    set32(&mut bytes, second + 8, 4);
    set32(&mut bytes, second + 12, 0x1004);
    set32(&mut bytes, second + 16, 4);
    set32(&mut bytes, second + 20, 0x700);
    set32(&mut bytes, second + 36, 0x60000020);
    assert!(HelperLayout::parse(&bytes, SOURCE).is_err());
}

#[test]
fn capture_checks_file_identity_and_holds_a_read_only_lease_without_executing() {
    let folder = std::env::temp_dir().join(format!("eg-helper-artifact-test-{}-{}", std::process::id(), jiff_free_nonce()));
    std::fs::create_dir(&folder).unwrap();
    let path = folder.join("helper.dll");
    let bytes = image();
    std::fs::write(&path, &bytes).unwrap();
    let hash = eg_core::hashes::of_bytes(&bytes);
    let artifact = HelperArtifact::capture(&path, &hash, SOURCE).unwrap();
    assert_eq!(artifact.bootstrap_rva(), 0x1000);
    assert_eq!(artifact.sha256(), hash);
    artifact.verify_unchanged().unwrap();
    assert!(HelperArtifact::capture(&path, &"0".repeat(64), SOURCE).is_err());
    #[cfg(windows)]
    assert!(std::fs::OpenOptions::new().write(true).open(&path).is_err());
    drop(artifact);
    std::fs::remove_file(&path).unwrap();
    std::fs::remove_dir(folder).unwrap();
}
fn jiff_free_nonce() -> u128 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
}

#[test]
fn bootstrap_binds_controller_birth_as_immutable_reply_data() {
    let config = BootstrapConfig::new(11, SOURCE).unwrap().with_owner(42, 123456).unwrap();
    assert_eq!(u32::from_le_bytes(config.bytes()[80..84].try_into().unwrap()), 42);
    assert_eq!(u64::from_le_bytes(config.bytes()[88..96].try_into().unwrap()), 123456);
    let mut reply = *config.bytes();
    reply[12..16].copy_from_slice(&0_i32.to_le_bytes());
    assert!(config.decode_reply(0, &reply).is_ok());
    reply[88] ^= 1;
    assert!(config.decode_reply(0, &reply).is_err());
    assert!(BootstrapConfig::new(11, SOURCE).unwrap().with_owner(0, 1).is_err());
    assert!(BootstrapConfig::new(11, SOURCE).unwrap().with_owner(42, 0).is_err());
}
