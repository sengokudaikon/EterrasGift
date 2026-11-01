//! Before-original numeric gains. Each feature owns its signature, selection and precedence; the shared arithmetic
//! rounds positive gains halfway away from zero and clamps before converting, including at the Int64 boundary.
//! All paths tail-call the original exactly once. No helper address, stack frame, runtime call or rollback is needed.
use crate::game::*;
use eg_trainer_wire::hooks::GameplayHook;
use native_hooks::{
    emit::{self, LeafCode},
    leaf::{FRAME, LeafEffect},
};

/// R10 is the signed gain, XMM1 the nonnegative f64 multiplier. Produces R10; scratches R11/XMM0/XMM2.
/// Nonpositive gains stay unchanged. The cap is Int32::MAX, or max(gain, Int32::MAX) for wide XP gains.
fn scale(c: &mut emit::Code, preserve_large: bool) {
    c.emit(&[0x4d, 0x85, 0xd2]);
    let skip = c.branch(0x8e);
    c.emit(&[0xf2, 0x49, 0x0f, 0x2a, 0xc2, 0xf2, 0x0f, 0x59, 0xc1]); // gain * multiplier
    c.emit(&[0x41, 0xbb, 0xff, 0xff, 0xff, 0x7f]);
    if preserve_large {
        c.emit(&[0x4d, 0x39, 0xda, 0x4d, 0x0f, 0x4f, 0xda]); // cap = max(gain, Int32::MAX)
    }
    c.emit(&[0xf2, 0x49, 0x0f, 0x2a, 0xd3, 0x66, 0x0f, 0x2f, 0xc2]);
    let convert = c.branch(0x82);
    c.emit(&[0x4d, 0x89, 0xda]); // cap: retain the exact integer, even when double(cap) is 2^63
    let done = c.branch(0x83); // same comparison flags; mov did not change them
    c.bind(convert);
    c.emit(&[0xf2, 0x4c, 0x0f, 0x2c, 0xd0]); // cvttsd2si r10,xmm0
    c.emit(&[0xf2, 0x49, 0x0f, 0x2a, 0xd2, 0xf2, 0x0f, 0x5c, 0xc2]); // fractional part = scaled - floor(scaled)
    c.emit(&[0x49, 0xbb]);
    c.emit(&0.5f64.to_bits().to_le_bytes());
    c.emit(&[0x66, 0x49, 0x0f, 0x6e, 0xd3, 0x66, 0x0f, 0x2f, 0xc2]);
    let round_down = c.branch(0x82);
    c.emit(&[0x49, 0xff, 0xc2]); // fraction >= .5 rounds away from zero
    c.bind(round_down);
    c.bind(done);
    c.bind(skip);
}

pub(crate) mod experience_multiplier;
pub(crate) mod faction_gains;
pub(crate) mod rank_up_per_gain;
#[cfg(test)]
mod tests;
