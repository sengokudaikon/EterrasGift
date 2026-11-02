use super::*;

#[test]
fn scoped_fields_restore_after_return_and_native_unwind_with_original_arguments_intact() {
    unsafe extern "C-unwind" fn original(fields: *mut [u32; 4], config: u16, method: usize) -> u64 {
        assert_eq!(config, 0x0101);
        // SAFETY: fixture owns four aligned mutable fields throughout the synchronous call.
        assert_eq!(unsafe { *fields }, [11, 22, 33, 44]);
        if method == 2 {
            panic!("original unwind");
        }
        assert_eq!(method, 1);
        0x123456789abcdef0
    }
    let fields = vec![0, 4, 8, 12];
    let mut c = Code::default();
    Scope::new(3, 4).save(&mut c, &fields);
    for (index, field) in fields.iter().enumerate() {
        c.offset(&[0xc7, 0x81], *field);
        c.emit(&((index as u32 + 1) * 11).to_le_bytes());
    }
    let effect = ScopedEffect {
        effect: PostfixEffect {
            before: c.finish(),
            code: vec![],
            slots: [0; 2],
            positions: 3,
        },
        fields,
    };
    // SAFETY: matching scalar return ABI and four aligned owned fields. Before-code saves before writing.
    let gate = unsafe { PostfixGate::prepare_scoped(&effect, None) }.unwrap();
    // SAFETY: fixture supplies the declared ABI and keeps every receiver live through return/unwind.
    unsafe {
        gate.set_original(original as *const ());
        let call: unsafe extern "C-unwind" fn(*mut [u32; 4], u16, usize) -> u64 = std::mem::transmute(gate.entry());
        let mut values = [1, 2, 3, 4];
        assert_eq!(call(&raw mut values, 0x0101, 1), 0x123456789abcdef0);
        assert_eq!(values, [1, 2, 3, 4]);
        let pointer = &raw mut values;
        assert!(std::panic::catch_unwind(|| call(pointer, 0x0101, 2)).is_err());
        assert_eq!(values, [1, 2, 3, 4]);
    }
}

#[test]
fn nested_calls_keep_separate_restoration_records() {
    #[repr(C)]
    struct Object {
        value: u32,
        inner: *mut Object,
        entry: usize,
    }
    unsafe extern "C-unwind" fn original(this: *mut Object, throws: bool) {
        // SAFETY: fixture owns both receivers; no mutable Rust reference spans a recursive native call.
        unsafe {
            assert_eq!((*this).value, 99);
            if !(*this).inner.is_null() {
                let call: unsafe extern "C-unwind" fn(*mut Object, bool) = std::mem::transmute((*this).entry);
                call((*this).inner, throws);
                assert_eq!((*this).value, 99);
                assert_eq!((*(*this).inner).value, 2);
            } else if throws {
                panic!("inner original unwind");
            }
        }
    }
    let mut c = Code::default();
    Scope::new(2, 1).save(&mut c, &[0]);
    c.emit(&[0xc7, 0x01, 99, 0, 0, 0]);
    let effect = ScopedEffect {
        effect: PostfixEffect {
            before: c.finish(),
            code: vec![],
            slots: [0; 2],
            positions: 2,
        },
        fields: vec![0],
    };
    // SAFETY: exact two-position void ABI and retained aligned receiver fields.
    unsafe {
        let gate = PostfixGate::prepare_scoped(&effect, None).unwrap();
        gate.set_original(original as *const ());
        let mut inner = Object {
            value: 2,
            inner: std::ptr::null_mut(),
            entry: gate.entry() as usize,
        };
        let mut outer = Object {
            value: 1,
            inner: &raw mut inner,
            entry: gate.entry() as usize,
        };
        let call: unsafe extern "C-unwind" fn(*mut Object, bool) = std::mem::transmute(gate.entry());
        call(&raw mut outer, false);
        assert_eq!((outer.value, inner.value), (1, 2));
        let pointer = &raw mut outer;
        assert!(std::panic::catch_unwind(|| call(pointer, true)).is_err());
        assert_eq!((outer.value, inner.value), (1, 2));
    }
}

#[test]
fn an_original_in_flight_restores_after_close_and_owner_release() {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, AtomicU8, AtomicU32},
    };
    #[repr(C)]
    struct Object {
        value: AtomicU32,
        entered: AtomicBool,
        release: AtomicBool,
    }
    unsafe extern "C-unwind" fn original(this: *const Object) {
        // SAFETY: caller retains its Arc through native unwind.
        let this = unsafe { &*this };
        assert_eq!(this.value.load(SeqCst), 99);
        this.entered.store(true, SeqCst);
        while !this.release.load(SeqCst) {
            std::thread::yield_now();
        }
        panic!("unwind after owner left");
    }
    let online = AtomicU8::new(0);
    let object = Arc::new(Object {
        value: AtomicU32::new(7),
        entered: AtomicBool::new(false),
        release: AtomicBool::new(false),
    });
    let mut c = Code::default();
    c.gated(FRAME.switch, FRAME.slots[0]);
    Scope::new(1, 1).save(&mut c, &[0]);
    c.emit(&[0xc7, 0x01, 99, 0, 0, 0]);
    let effect = ScopedEffect {
        effect: PostfixEffect {
            before: c.finish(),
            code: vec![],
            slots: [&online as *const _ as u64, 0],
            positions: 1,
        },
        fields: vec![0],
    };
    // SAFETY: process-retained exact void(pointer) original, owned aligned scalar. Online storage outlives the caller.
    let gate = unsafe { PostfixGate::prepare_scoped(&effect, None) }.unwrap();
    // SAFETY: exact ABI, installed before entry is used.
    unsafe {
        gate.set_original(original as *const ());
    }
    gate.set_enabled(true);
    let entry = gate.entry() as usize;
    let caller = object.clone();
    let thread = std::thread::spawn(move || {
        // SAFETY: retained generated entry/original code; caller's Arc holds storage through exception cleanup.
        let call: unsafe extern "C-unwind" fn(*const Object) = unsafe { std::mem::transmute(entry) };
        // SAFETY: same ABI and live receiver throughout the call.
        assert!(std::panic::catch_unwind(|| unsafe { call(Arc::as_ptr(&caller)) }).is_err());
    });
    while !object.entered.load(SeqCst) {
        std::thread::yield_now();
    }
    gate.set_enabled(false);
    // Dropping a gateway owner releases no executable code, cleanup data or unwind metadata.
    let mut owner = Some(gate);
    owner.take();
    assert!(owner.is_none());
    assert_eq!(object.value.load(SeqCst), 99);
    object.release.store(true, SeqCst);
    thread.join().unwrap();
    assert_eq!(object.value.load(SeqCst), 7);
}
