//! The authored game binary several tests share: an inert one-section PE32+ whose executable section holds the
//! dispatcher's bytes. It is parsed as bytes only, never loaded or executed.
use eg_trainer_wire::protocol::DispatcherSpec;

pub fn dispatcher() -> DispatcherSpec {
    DispatcherSpec {
        rva: 0x1000,
        bytes: [0x48, 0x83, 0xec, 0x28, 0x80, 0x3d, 1, 2, 3, 4, 0, 0x75, 0x13, 0x48, 0x8d, 0x0d],
    }
}

pub fn image(dispatcher: &DispatcherSpec) -> Vec<u8> {
    let mut image = vec![0u8; 0x400];
    image[..2].copy_from_slice(b"MZ");
    image[0x3c..0x40].copy_from_slice(&0x80u32.to_le_bytes());
    image[0x80..0x84].copy_from_slice(b"PE\0\0");
    image[0x84..0x86].copy_from_slice(&0x8664u16.to_le_bytes());
    image[0x86..0x88].copy_from_slice(&1u16.to_le_bytes());
    image[0x94..0x96].copy_from_slice(&240u16.to_le_bytes());
    image[0x98..0x9a].copy_from_slice(&0x20bu16.to_le_bytes());
    image[0x194..0x198].copy_from_slice(&0x1000u32.to_le_bytes());
    image[0x198..0x19c].copy_from_slice(&0x200u32.to_le_bytes());
    image[0x19c..0x1a0].copy_from_slice(&0x200u32.to_le_bytes());
    image[0x1ac..0x1b0].copy_from_slice(&0x6000_0020u32.to_le_bytes());
    image[0x200..0x210].copy_from_slice(&dispatcher.bytes);
    image
}
