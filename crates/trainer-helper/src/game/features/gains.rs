//! Before-original numeric gains. Each feature owns its signature, selection and precedence; the shared arithmetic
//! rounds positive gains halfway away from zero and clamps before converting, including at the Int64 boundary.
//! All paths tail-call the original exactly once. No helper address, stack frame, runtime call or rollback is needed.
use crate::{
    game::*,
    hooks::{
        emit,
        leaf::{FRAME, GeneratedEffect, LeafEffect},
    },
};
use eg_trainer_wire::hooks::GameplayHook;
use std::ops::{Deref, DerefMut};

struct Code {
    code: emit::Code,
    online: NonNull<c_void>,
}
impl Code {
    fn new(online: NonNull<c_void>) -> Self {
        let mut code = emit::Code::default();
        code.emit(&[0x4c, 0x8b, 0x58, FRAME.switch, 0x49, 0x83, 0x3b, 0]);
        code.exit(0x84);
        code.emit(&[0x4c, 0x8b, 0x58, FRAME.effect[0], 0x41, 0x80, 0x3b, 0]);
        code.exit(0x85);
        code.nonnull(&[0x48, 0x85, 0xc9]);
        Self { code, online }
    }
    fn finish(self, extra: [u64; 3]) -> LeafEffect {
        LeafEffect::Generated(GeneratedEffect {
            code: self.code.finish(),
            slots: [self.online.as_ptr() as u64, extra[0], extra[1], extra[2]],
        })
    }
    /// R10 is the signed gain, XMM1 the nonnegative f64 multiplier. Produces R10; scratches R11/XMM0/XMM2.
    /// Nonpositive gains stay unchanged. The cap is Int32::MAX, or max(gain, Int32::MAX) for wide XP gains.
    fn scale(&mut self, preserve_large: bool) {
        self.emit(&[0x4d, 0x85, 0xd2]);
        let skip = self.branch(0x8e);
        self.emit(&[0xf2, 0x49, 0x0f, 0x2a, 0xc2, 0xf2, 0x0f, 0x59, 0xc1]); // gain * multiplier
        self.emit(&[0x41, 0xbb, 0xff, 0xff, 0xff, 0x7f]);
        if preserve_large {
            self.emit(&[0x4d, 0x39, 0xda, 0x4d, 0x0f, 0x4f, 0xda]); // cap = max(gain, Int32::MAX)
        }
        self.emit(&[0xf2, 0x49, 0x0f, 0x2a, 0xd3, 0x66, 0x0f, 0x2f, 0xc2]);
        let convert = self.branch(0x82);
        self.emit(&[0x4d, 0x89, 0xda]); // cap: retain the exact integer, even when double(cap) is 2^63
        let done = self.branch(0x83); // same comparison flags; mov did not change them
        self.bind(convert);
        self.emit(&[0xf2, 0x4c, 0x0f, 0x2c, 0xd0]); // cvttsd2si r10,xmm0
        self.emit(&[0xf2, 0x49, 0x0f, 0x2a, 0xd2, 0xf2, 0x0f, 0x5c, 0xc2]); // fractional part = scaled - floor(scaled)
        self.emit(&[0x49, 0xbb]);
        self.emit(&0.5f64.to_bits().to_le_bytes());
        self.emit(&[0x66, 0x49, 0x0f, 0x6e, 0xd3, 0x66, 0x0f, 0x2f, 0xc2]);
        let round_down = self.branch(0x82);
        self.emit(&[0x49, 0xff, 0xc2]); // fraction >= .5 rounds away from zero
        self.bind(round_down);
        self.bind(done);
        self.bind(skip);
    }
}
impl Deref for Code {
    type Target = emit::Code;
    fn deref(&self) -> &emit::Code {
        &self.code
    }
}
impl DerefMut for Code {
    fn deref_mut(&mut self) -> &mut emit::Code {
        &mut self.code
    }
}

pub(crate) mod experience_multiplier;
pub(crate) mod faction_gains;
pub(crate) mod rank_up_per_gain;
#[cfg(test)]
mod tests;
