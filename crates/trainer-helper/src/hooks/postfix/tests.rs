use super::*;
use crate::hooks::emit::Code;

extern "C-unwind" fn checked(a: u64, b: u64, c: u64, d: u64, e: u64, f: u64, g: u64, h: u64, i: u64, j: u64, k: u64, l: u64) -> u64 {
    assert_eq!([a, b, c, d, e, f, g, h, i, j, k, l], [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12]);
    0x1122334455667788
}
#[test]
fn before_snapshots_survive_original_home_writes_without_clobbering_twelve_argument_positions() {
    #[unsafe(naked)]
    unsafe extern "C-unwind" fn original(_: u64, _: u64, _: u64, _: u64, _: u64, _: u64, _: u64, _: u64, _: u64, _: u64, _: u64, _: u64) -> u64 {
        core::arch::naked_asm!("mov [rsp+8],rcx","mov [rsp+16],rdx","mov [rsp+24],r8","mov [rsp+32],r9","jmp {}",sym checked);
    }
    let words = snapshots(12).map(u32::from);
    assert_eq!(words, [112, 128]);
    let mut before = Code::default();
    before.offset(&[0x48, 0x89, 0x94, 0x24], words[0]);
    before.offset(&[0x4c, 0x89, 0x84, 0x24], words[1]);
    let mut after = Code::default();
    after.offset(&[0x48, 0x83, 0xbc, 0x24], words[0]);
    after.emit(&[2]);
    let bad = after.branch(0x85);
    after.offset(&[0x48, 0x83, 0xbc, 0x24], words[1]);
    after.emit(&[3]);
    let also_bad = after.branch(0x85);
    after.done();
    after.bind(bad);
    after.bind(also_bad);
    after.emit(&[0x45, 0x33, 0xd2]);
    let effect = PostfixEffect {
        before: before.finish(),
        code: after.finish(),
        slots: [0; 2],
        positions: 12,
    };
    // SAFETY: complete twelve-position scalar ABI, no hidden buffer; snapshot code uses only the gateway's home slots.
    let gate = unsafe { PostfixGate::prepare(&effect, None).unwrap() };
    // SAFETY: exact matching original, set before any generated call.
    unsafe {
        gate.set_original(original as *const ());
    }
    type Call = extern "C-unwind" fn(u64, u64, u64, u64, u64, u64, u64, u64, u64, u64, u64, u64) -> u64;
    // SAFETY: all twelve real argument positions are supplied, with writable caller home space per Windows x64.
    let call: Call = unsafe { std::mem::transmute(gate.entry()) };
    assert_eq!(call(1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12), 0x1122334455667788);
}
