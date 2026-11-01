//! The bootstrap's configuration block: the desktop writes it into the game process, the helper's bootstrap thread
//! reads it in place and writes its status back. One layout for both sides, little endian, 96 bytes.

pub const MAGIC: u32 = 0x4547_4253;
pub const SIZE: usize = 96;
/// The status the desktop writes before the helper answers.
pub const UNANSWERED: i32 = -1;

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BootstrapConfig {
    pub magic: u32,
    pub size: u32,
    pub pid: u32,
    /// The helper's answer: 0 started, anything else refused.
    pub status: i32,
    /// The SHA-256 of the DLL the desktop loaded, as ASCII hex: the ID the helper reports from then on.
    pub build_id: [u8; 64],
    /// The controlling desktop process, whose exit also stops the helper.
    pub owner_pid: u32,
    pub reserved: u32,
    pub owner_created: u64,
}
const _: () = assert!(
    size_of::<BootstrapConfig>() == SIZE
        && std::mem::offset_of!(BootstrapConfig, status) == 12
        && std::mem::offset_of!(BootstrapConfig, build_id) == 16
        && std::mem::offset_of!(BootstrapConfig, owner_pid) == 80
        && std::mem::offset_of!(BootstrapConfig, owner_created) == 88
);

impl BootstrapConfig {
    /// The block the desktop writes: unanswered, its owner still unset.
    pub fn new(pid: u32, build_id: [u8; 64]) -> Self {
        Self {
            magic: MAGIC,
            size: SIZE as u32,
            pid,
            status: UNANSWERED,
            build_id,
            owner_pid: 0,
            reserved: 0,
            owner_created: 0,
        }
    }

    pub fn to_bytes(&self) -> [u8; SIZE] {
        let mut bytes = [0; SIZE];
        bytes[0..4].copy_from_slice(&self.magic.to_le_bytes());
        bytes[4..8].copy_from_slice(&self.size.to_le_bytes());
        bytes[8..12].copy_from_slice(&self.pid.to_le_bytes());
        bytes[12..16].copy_from_slice(&self.status.to_le_bytes());
        bytes[16..80].copy_from_slice(&self.build_id);
        bytes[80..84].copy_from_slice(&self.owner_pid.to_le_bytes());
        bytes[84..88].copy_from_slice(&self.reserved.to_le_bytes());
        bytes[88..96].copy_from_slice(&self.owner_created.to_le_bytes());
        bytes
    }

    pub fn from_bytes(bytes: &[u8; SIZE]) -> Self {
        let u32_at = |at: usize| u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap());
        Self {
            magic: u32_at(0),
            size: u32_at(4),
            pid: u32_at(8),
            status: i32::from_le_bytes(bytes[12..16].try_into().unwrap()),
            build_id: bytes[16..80].try_into().unwrap(),
            owner_pid: u32_at(80),
            reserved: u32_at(84),
            owner_created: u64::from_le_bytes(bytes[88..96].try_into().unwrap()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bytes_round_trip_in_the_documented_layout() {
        let config = BootstrapConfig {
            owner_pid: 42,
            owner_created: 123_456,
            ..BootstrapConfig::new(0x1122_3344, [b'a'; 64])
        };
        let bytes = config.to_bytes();
        assert_eq!(&bytes[0..4], &MAGIC.to_le_bytes());
        assert_eq!(&bytes[8..12], &0x1122_3344u32.to_le_bytes());
        assert_eq!(&bytes[12..16], &(-1i32).to_le_bytes());
        assert_eq!(&bytes[80..84], &42u32.to_le_bytes());
        assert_eq!(BootstrapConfig::from_bytes(&bytes), config);
    }
}
