use eg_game_access::pe::Layout;
use std::io;

const OPTIONAL_AT: usize = 0x58;
const RAW_AT: usize = 0x200;

fn set16(bytes: &mut [u8], at: usize, value: u16) {
    bytes[at..at + 2].copy_from_slice(&value.to_le_bytes());
}
fn set32(bytes: &mut [u8], at: usize, value: u32) {
    bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
}
fn image(optional_size: u16, directory_count: u32) -> Vec<u8> {
    let mut bytes = vec![0; 0x400];
    bytes[..2].copy_from_slice(b"MZ");
    set32(&mut bytes, 0x3c, 0x40);
    bytes[0x40..0x44].copy_from_slice(b"PE\0\0");
    set16(&mut bytes, 0x44, 0x8664);
    set16(&mut bytes, 0x46, 1);
    set16(&mut bytes, 0x54, optional_size);
    set16(&mut bytes, OPTIONAL_AT, 0x20b);
    set32(&mut bytes, OPTIONAL_AT + 56, 0x2000);
    set32(&mut bytes, OPTIONAL_AT + 108, directory_count);
    let section = OPTIONAL_AT + usize::from(optional_size);
    set32(&mut bytes, section + 8, 0x200);
    set32(&mut bytes, section + 12, 0x1000);
    set32(&mut bytes, section + 16, 0x200);
    set32(&mut bytes, section + 20, RAW_AT as u32);
    set32(&mut bytes, section + 36, 0x6000_0020);
    bytes
}

#[test]
fn streaming_reader_needs_only_headers_to_map_retained_section_data() {
    for (optional_size, directory_count) in [(112, 0), (120, 1), (136, 3), (144, 4)] {
        let bytes = image(optional_size, directory_count);
        let headers_end = OPTIONAL_AT + usize::from(optional_size) + 40;
        let layout = Layout::read(bytes.len() as u64, |at, output| {
            let at = at as usize;
            assert!(at + output.len() <= headers_end, "reader requested section data");
            output.copy_from_slice(&bytes[at..at + output.len()]);
            Ok(())
        })
        .unwrap();
        assert_eq!(layout.file_offset(0x1000, 0x200, true).unwrap(), RAW_AT as u64);
        assert!(layout.file_offset(0x1000, 0x201, true).is_err());
    }
}

#[test]
fn declared_directories_must_fit_the_optional_header() {
    for (optional_size, directory_count, reason) in [(112, 1, "PE data directory size"), (136, 4, "PE exception directory size")] {
        let error = Layout::from_bytes(&image(optional_size, directory_count)).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert_eq!(error.to_string(), reason);
    }
}

#[test]
fn streaming_reader_propagates_io_errors_without_retrying() {
    let bytes = image(144, 4);
    let mut failed = false;
    let error = Layout::read(bytes.len() as u64, |at, output| {
        assert!(!failed, "reader retried after an I/O error");
        let at = at as usize;
        if at == OPTIONAL_AT + 112 + 3 * 8 {
            failed = true;
            return Err(io::Error::new(io::ErrorKind::PermissionDenied, "retained file read failed"));
        }
        output.copy_from_slice(&bytes[at..at + output.len()]);
        Ok(())
    })
    .unwrap_err();
    assert!(failed);
    assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
    assert_eq!(error.to_string(), "retained file read failed");
}
