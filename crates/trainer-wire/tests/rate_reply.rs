use eg_trainer_wire::{
    fields::{self, AttackLeechObservation, AttackLeechRead, AttackLeechTarget, AttackLeechUnavailable, ObservedObjectIdentity, ObservedRate},
    protocol::{DispatcherSpec, MAX_RESPONSE_SIZE, Response, ResponseCode},
    rate_write::{AttackLeechRateWrite, RateWriteReply, classify_response},
    write::MAX_ERROR_TEXT,
};
fn request() -> AttackLeechRateWrite {
    let identity = |n| ObservedObjectIdentity::new(n, n + 1).unwrap();
    AttackLeechRateWrite::new(
        DispatcherSpec { rva: 7, bytes: [1; 16] },
        AttackLeechObservation {
            manager: identity(1),
            actor: identity(3),
            stats: identity(5),
            attack_rate: ObservedRate { raw_bits: 0x7fc12345 },
            increased_leech_rate: ObservedRate { raw_bits: 0xff812345 },
        },
        AttackLeechTarget::AttackRate,
        1.1_f32.to_bits(),
    )
    .unwrap()
}
fn response(code: ResponseCode, payload: Vec<u8>) -> Response {
    Response { code, payload }
}
#[test]
fn exact_full_confirmation_preserves_untouched_signaling_nan_payload() {
    let w = request();
    let projected = w.projected();
    let reply = response(ResponseCode::Done, fields::encode(&AttackLeechRead::Available(projected)));
    assert_eq!(classify_response(&reply, &w), RateWriteReply::Confirmed(projected));
    assert_eq!(projected.increased_leech_rate.raw_bits, 0xff812345);
}
#[test]
fn every_changed_success_byte_and_every_wrong_length_is_uncertain() {
    let w = request();
    let bytes = fields::encode(&AttackLeechRead::Available(w.projected()));
    assert_eq!(bytes.len(), 64);
    for offset in 0..64 {
        let mut changed = bytes.clone();
        changed[offset] ^= 1;
        assert!(
            matches!(classify_response(&response(ResponseCode::Done, changed), &w), RateWriteReply::Uncertain { .. }),
            "offset {offset}"
        );
    }
    for length in 0..64 {
        assert!(matches!(
            classify_response(&response(ResponseCode::Done, bytes[..length].to_vec()), &w),
            RateWriteReply::Uncertain { .. }
        ));
    }
    for extra in [1, MAX_RESPONSE_SIZE] {
        let mut changed = bytes.clone();
        changed.resize(64 + extra, 0);
        assert!(matches!(classify_response(&response(ResponseCode::Done, changed), &w), RateWriteReply::Uncertain { .. }));
    }
}
#[test]
fn every_unavailable_reason_is_uncertain_after_done_never_a_prestore_refusal() {
    let w = request();
    for reason in [
        AttackLeechUnavailable::OfflineRequired,
        AttackLeechUnavailable::ManagerAbsent,
        AttackLeechUnavailable::ManagerChanged,
        AttackLeechUnavailable::ActorCacheAbsent,
        AttackLeechUnavailable::StatsAbsent,
        AttackLeechUnavailable::NativeObjectDestroyed,
        AttackLeechUnavailable::GraphChanged,
        AttackLeechUnavailable::UnsupportedStatsClass,
    ] {
        assert!(matches!(
            classify_response(&response(ResponseCode::Done, fields::encode(&AttackLeechRead::Unavailable(reason))), &w),
            RateWriteReply::Uncertain { .. }
        ));
    }
}
#[test]
fn trusted_codes_require_bounded_utf8_and_failed_is_always_uncertain() {
    let w = request();
    for code in [
        ResponseCode::Failed,
        ResponseCode::NotReady,
        ResponseCode::StaleItem,
        ResponseCode::Refused,
        ResponseCode::OfflineRequired,
        ResponseCode::RestartRequired,
    ] {
        for length in [0, MAX_ERROR_TEXT] {
            let r = classify_response(&response(code, vec![b'x'; length]), &w);
            assert_eq!(matches!(r, RateWriteReply::Uncertain { .. }), code == ResponseCode::Failed);
            if code != ResponseCode::Failed {
                assert_eq!(
                    r,
                    RateWriteReply::NotApplied {
                        code,
                        message: "x".repeat(length)
                    }
                );
            }
        }
        for payload in [vec![0xff], vec![b'x'; MAX_ERROR_TEXT + 1], vec![b'x'; MAX_RESPONSE_SIZE + 1]] {
            assert!(matches!(classify_response(&response(code, payload), &w), RateWriteReply::Uncertain { .. }));
        }
    }
}
