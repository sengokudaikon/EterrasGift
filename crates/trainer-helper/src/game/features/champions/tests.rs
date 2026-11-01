use super::*;
use native_hooks::leaf::LeafGate;
use std::sync::atomic::{AtomicU8, Ordering::SeqCst};
#[repr(C)]
#[derive(Default)]
struct Probe {
    caller: usize,
    calls: usize,
    min: f32,
    max: f32,
    times: i32,
    method: usize,
}
type Call = unsafe extern "C-unwind" fn(*mut Probe, f32, f32, i32, usize) -> f32;
#[unsafe(naked)]
unsafe extern "C-unwind" fn original(_: *mut Probe, _: f32, _: f32, _: i32, _: usize) -> f32 {
    core::arch::naked_asm!(
        "mov r11,[rsp]",
        "mov [rcx],r11",
        "inc qword ptr [rcx+8]",
        "movss [rcx+16],xmm1",
        "movss [rcx+20],xmm2",
        "mov [rcx+24],r9d",
        "mov r11,[rsp+40]",
        "mov [rcx+32],r11",
        "addss xmm1,xmm2",
        "movaps xmm0,xmm1",
        "ret"
    );
}
#[inline(never)]
fn selected(call: Call, probe: &mut Probe) -> f32 {
    // SAFETY: authored leaf/original with exact mixed floating/register/stack ABI over this probe.
    let answer = unsafe { call(probe, 1.25, 2.5, 7, 0x12345678) };
    std::hint::black_box(answer)
}
#[inline(never)]
fn unrelated(call: Call, probe: &mut Probe) -> f32 {
    // SAFETY: distinct native caller, same validated ABI and retained probe.
    let answer = unsafe { call(probe, 2.25, 6.5, 9, 0xabcdef01) };
    std::hint::black_box(answer)
}
#[test]
fn only_the_selected_caller_overrides_the_roll_and_other_calls_keep_every_argument() {
    let online = Box::leak(Box::new(AtomicU8::new(0)));
    // SAFETY: exact five-position float result ABI and retained online byte. The first call probes its own return address.
    let gate = unsafe { LeafGate::prepare_override(effect(NonNull::from(&*online).cast(), 0), None, None).unwrap() };
    // SAFETY: matching original configured while closed.
    unsafe {
        gate.set_original(original as *const ());
    }
    gate.set_enabled(true);
    // SAFETY: leaf preserves XMM1/XMM2/R9 and the stack MethodInfo when forwarding.
    let call: Call = unsafe { std::mem::transmute(gate.entry()) };
    let mut probe = Probe::default();
    assert_eq!(selected(call, &mut probe), 3.75);
    let site = probe.caller;
    gate.set_slot(1, site as u64);
    for count in [0.0f64, 1.0, 2.0, 3.0] {
        gate.set_value(count.to_bits());
        let before = probe.calls;
        assert_eq!(selected(call, &mut probe), count as f32);
        assert_eq!(probe.calls, before);
        assert_eq!(unrelated(call, &mut probe), 8.75);
        assert_eq!(probe.calls, before + 1);
        assert_eq!((probe.min, probe.max, probe.times, probe.method), (2.25, 6.5, 9, 0xabcdef01));
    }
    online.store(1, SeqCst);
    assert_eq!(selected(call, &mut probe), 3.75);
    assert_eq!((probe.min, probe.max, probe.times, probe.method), (1.25, 2.5, 7, 0x12345678));
    online.store(0, SeqCst);
    gate.set_enabled(false);
    assert_eq!(selected(call, &mut probe), 3.75);
}
#[test]
fn caller_recipe_checks_the_exact_float_int_overload_and_rejects_ambiguity() {
    let start = 0x1000u64;
    let target = 0x8000u64;
    let mut code = vec![0x90; 16];
    code.extend(PATTERN.iter().map(|byte| if *byte < 0 { 0 } else { *byte as u8 }));
    code[21..25].copy_from_slice(&((target - (start + 25)) as i32).to_le_bytes());
    assert_eq!(caller(&code, start, target).unwrap(), start + 25);
    assert!(caller(&code, start, target + 1).is_err());
    assert!(caller(&code.repeat(2), start, target).is_err());
    code[16] = 0xcc;
    assert!(caller(&code, start, target).is_err());
}
