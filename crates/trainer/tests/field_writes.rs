//! Pure planning against authored owned observations: no runtime/process I/O.
use eg_trainer::{
    backend::{AttackLeechObservation, AttackLeechRead, AttackLeechSnapshot, AttackLeechUnavailable, ObservedObjectIdentity, ObservedRate, ProcessIdentity},
    features::{FeatureKey, FieldSetId, NativeSupport},
    field_writes::{AttackLeechDecision, AttackLeechPlan, AttackLeechTarget, FieldInputError, FieldPlanError},
};

fn identity(wrapper: u64, native: u64) -> ObservedObjectIdentity {
    ObservedObjectIdentity::new(wrapper, native).unwrap()
}
fn snapshot() -> AttackLeechSnapshot {
    AttackLeechSnapshot {
        process: ProcessIdentity {
            pid: 42,
            created: 100,
            executable: std::env::temp_dir().join("authored-fixture.exe"),
        },
        generation: 7,
        sequence: 9,
        manager: 0x1000,
        read: AttackLeechRead::Available(AttackLeechObservation {
            manager: identity(0x1000, 0x2000),
            actor: identity(0x3000, 0x4000),
            stats: identity(0x5000, 0x6000),
            attack_rate: ObservedRate { raw_bits: 1.0_f32.to_bits() },
            increased_leech_rate: ObservedRate { raw_bits: 0.5_f32.to_bits() },
        }),
    }
}
fn change(expected: &AttackLeechSnapshot, target: AttackLeechTarget, value: f64) -> AttackLeechPlan {
    match AttackLeechPlan::prepare(expected, target, value).unwrap() {
        AttackLeechDecision::Change(plan) => plan,
        AttackLeechDecision::NoOp => panic!("expected a changed projection"),
    }
}
fn observation_mut(snapshot: &mut AttackLeechSnapshot) -> &mut AttackLeechObservation {
    match &mut snapshot.read {
        AttackLeechRead::Available(value) => value,
        _ => panic!("authored available fixture"),
    }
}

#[test]
fn only_the_two_source_fields_are_supported_without_advertising_native_support() {
    let set = FieldSetId::parse("attack-leech").unwrap();
    assert_eq!(AttackLeechTarget::from_field(set, 0), Ok(AttackLeechTarget::AttackRate));
    assert_eq!(AttackLeechTarget::from_field(set, 1), Ok(AttackLeechTarget::IncreasedLeechRate));
    assert_eq!(AttackLeechTarget::from_field(set, 2), Err(FieldPlanError::UnsupportedField));
    assert_eq!(
        AttackLeechTarget::from_field(FieldSetId::parse("currency").unwrap(), 0),
        Err(FieldPlanError::UnsupportedField)
    );
    let descriptor = FeatureKey::parse("attack-leech").unwrap().descriptor();
    assert_eq!(descriptor.native_support, NativeSupport::Unsupported);
    assert_eq!(AttackLeechTarget::AttackRate.descriptor(), &descriptor.fields[0]);
    assert_eq!(AttackLeechTarget::IncreasedLeechRate.descriptor(), &descriptor.fields[1]);
    assert_eq!((descriptor.fields[0].minimum, descriptor.fields[0].maximum), (0.01, 20.0));
    assert_eq!((descriptor.fields[1].minimum, descriptor.fields[1].maximum), (-1.0, 100.0));
    assert!(matches!(
        AttackLeechPlan::prepare(&snapshot(), AttackLeechTarget::AttackRate, 2.0),
        Ok(AttackLeechDecision::Change(_))
    ));
}

#[test]
fn double_lower_boundary_is_checked_before_float_projection_and_never_clamped() {
    let expected = snapshot();
    let plan = change(&expected, AttackLeechTarget::AttackRate, 0.01);
    assert_eq!(plan.requested_value(), 0.01_f64);
    assert_eq!(plan.projected_bits(), 0.01_f32.to_bits());
    let widened = f64::from(0.01_f32);
    assert!(widened < 0.01_f64);
    assert_eq!(
        AttackLeechPlan::prepare(&expected, AttackLeechTarget::AttackRate, widened),
        Err(FieldPlanError::InvalidInput(FieldInputError::OutsideEditorRange))
    );
    for input in [f64::from_bits(0.01_f64.to_bits() - 1), f64::from_bits(20.0_f64.to_bits() + 1)] {
        // Both would round to allowed f32 endpoint bits, so checking after the cast is incorrect.
        assert!((0.01_f32..=20.0_f32).contains(&(input as f32)));
        assert_eq!(
            AttackLeechPlan::prepare(&expected, AttackLeechTarget::AttackRate, input),
            Err(FieldPlanError::InvalidInput(FieldInputError::OutsideEditorRange))
        );
    }
}

#[test]
fn decimal_display_is_not_widened_float_equality_or_step_quantization() {
    let expected = snapshot();
    let plan = change(&expected, AttackLeechTarget::AttackRate, 1.1);
    assert_eq!(plan.requested_value(), 1.1_f64);
    assert_eq!(plan.projected_bits(), 1.1_f32.to_bits());
    assert_ne!(f64::from(plan.projected_value()), plan.requested_value());
    // Rust's shortest roundtrip rendering demonstrates the same display concern as the source's
    // shortestDecimal helper. It does not make a native formatter/locale conformance claim.
    assert_eq!(plan.projected_value().to_string(), "1.1");
    assert_eq!(0.01_f32.to_string().parse::<f64>().unwrap(), 0.01_f64);
    let unquantized = change(&expected, AttackLeechTarget::AttackRate, 1.23456789);
    assert_eq!(unquantized.projected_bits(), (1.23456789_f64 as f32).to_bits());
    assert_ne!(unquantized.projected_bits(), 1.23_f32.to_bits());
}

#[test]
fn nonfinite_and_out_of_source_range_input_is_rejected_for_both_targets() {
    let expected = snapshot();
    for target in [AttackLeechTarget::AttackRate, AttackLeechTarget::IncreasedLeechRate] {
        for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, f64::from_bits(0x7ff0000000000001)] {
            assert_eq!(
                AttackLeechPlan::prepare(&expected, target, value),
                Err(FieldPlanError::InvalidInput(FieldInputError::NonFinite))
            );
        }
        let descriptor = target.descriptor();
        for value in [descriptor.minimum - 1.0, descriptor.maximum + 1.0] {
            assert_eq!(
                AttackLeechPlan::prepare(&expected, target, value),
                Err(FieldPlanError::InvalidInput(FieldInputError::OutsideEditorRange))
            );
        }
        for value in [descriptor.minimum, descriptor.maximum] {
            assert!(AttackLeechPlan::prepare(&expected, target, value).is_ok());
        }
    }
}

#[test]
fn projection_preserves_all_other_tokens_and_raw_bits_including_nonfinite_payloads() {
    for target in [AttackLeechTarget::AttackRate, AttackLeechTarget::IncreasedLeechRate] {
        for other_bits in [0x7fc01234, 0x7f800001, f32::INFINITY.to_bits(), (-0.0_f32).to_bits()] {
            let mut expected = snapshot();
            let before = observation_mut(&mut expected);
            match target {
                AttackLeechTarget::AttackRate => before.increased_leech_rate.raw_bits = other_bits,
                AttackLeechTarget::IncreasedLeechRate => before.attack_rate.raw_bits = other_bits,
            }
            let mut projected = *before;
            match target {
                AttackLeechTarget::AttackRate => projected.attack_rate.raw_bits = 2.0_f32.to_bits(),
                AttackLeechTarget::IncreasedLeechRate => projected.increased_leech_rate.raw_bits = 2.0_f32.to_bits(),
            }
            let plan = change(&expected, target, 2.0);
            assert_eq!(plan.before(), &expected);
            assert_eq!(plan.projected(), &projected);
            assert_eq!(plan.target(), target);
            assert_eq!(plan.recheck(&expected), Ok(()));
        }
    }
    // A finite, in-range requested value may repair an observed nonfinite selected field.
    let mut expected = snapshot();
    observation_mut(&mut expected).attack_rate.raw_bits = 0x7fc01234;
    assert_eq!(change(&expected, AttackLeechTarget::AttackRate, 2.0).projected_bits(), 2.0_f32.to_bits());
}

#[test]
fn projected_same_bits_are_explicit_noop_but_signed_zero_changes_are_preserved() {
    let expected = snapshot();
    for value in [1.0_f64, f64::from_bits(1.0_f64.to_bits() + 1)] {
        assert_eq!(AttackLeechPlan::prepare(&expected, AttackLeechTarget::AttackRate, value), Ok(AttackLeechDecision::NoOp));
    }
    let mut expected = snapshot();
    observation_mut(&mut expected).increased_leech_rate.raw_bits = (-0.0_f32).to_bits();
    assert_eq!(
        AttackLeechPlan::prepare(&expected, AttackLeechTarget::IncreasedLeechRate, -0.0),
        Ok(AttackLeechDecision::NoOp)
    );
    let plan = change(&expected, AttackLeechTarget::IncreasedLeechRate, 0.0);
    assert_eq!(plan.projected_bits(), 0.0_f32.to_bits());
    assert_eq!(plan.requested_value().to_bits(), 0.0_f64.to_bits());
    observation_mut(&mut expected).increased_leech_rate.raw_bits = 0.0_f32.to_bits();
    let negative = change(&expected, AttackLeechTarget::IncreasedLeechRate, -0.0);
    assert_eq!(negative.projected_bits(), (-0.0_f32).to_bits());
    assert_eq!(negative.requested_value().to_bits(), (-0.0_f64).to_bits());
}

#[test]
fn unavailable_and_invalid_observation_are_distinct_from_invalid_input() {
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
        let mut expected = snapshot();
        expected.read = AttackLeechRead::Unavailable(reason);
        assert_eq!(
            AttackLeechPlan::prepare(&expected, AttackLeechTarget::AttackRate, 2.0),
            Err(FieldPlanError::Unavailable(reason))
        );
    }
    for fault in 0..7 {
        let mut expected = snapshot();
        match fault {
            0 => expected.process.pid = 0,
            1 => expected.process.created = 0,
            2 => expected.process.executable = "relative.exe".into(),
            3 => expected.generation = 0,
            4 => expected.sequence = 0,
            5 => expected.manager = 0,
            _ => expected.manager += 1,
        }
        assert_eq!(
            AttackLeechPlan::prepare(&expected, AttackLeechTarget::AttackRate, 2.0),
            Err(FieldPlanError::InvalidObservation)
        );
    }
}

#[test]
fn recheck_rejects_every_process_generation_sequence_manager_token_and_raw_bit_drift() {
    let expected = snapshot();
    let plan = change(&expected, AttackLeechTarget::AttackRate, 2.0);
    assert_eq!(plan.recheck(&expected.clone()), Ok(()));
    for fault in 0..16 {
        let mut changed = expected.clone();
        match fault {
            0 => changed.process.pid += 1,
            1 => changed.process.created += 1,
            2 => changed.process.executable = std::env::temp_dir().join("other-fixture.exe"),
            3 => changed.generation += 1,
            4 => changed.sequence += 1,
            5 => changed.manager += 1,
            6 => observation_mut(&mut changed).manager = identity(0x1001, 0x2000),
            7 => observation_mut(&mut changed).manager = identity(0x1000, 0x2001),
            8 => observation_mut(&mut changed).actor = identity(0x3001, 0x4000),
            9 => observation_mut(&mut changed).actor = identity(0x3000, 0x4001),
            10 => observation_mut(&mut changed).stats = identity(0x5001, 0x6000),
            11 => observation_mut(&mut changed).stats = identity(0x5000, 0x6001),
            12 => observation_mut(&mut changed).attack_rate.raw_bits ^= 1,
            13 => observation_mut(&mut changed).increased_leech_rate.raw_bits ^= 1,
            14 => changed.read = AttackLeechRead::Unavailable(AttackLeechUnavailable::GraphChanged),
            _ => {
                changed.sequence += 1;
                changed.generation += 1;
            }
        }
        assert_eq!(plan.recheck(&changed), Err(FieldPlanError::StaleObservation), "drift case {fault}");
    }
}

#[test]
fn recheck_preserves_nan_payload_and_signed_zero_distinctions_in_the_untouched_field() {
    let mut expected = snapshot();
    observation_mut(&mut expected).increased_leech_rate.raw_bits = 0x7fc01234;
    let plan = change(&expected, AttackLeechTarget::AttackRate, 2.0);
    assert_eq!(plan.recheck(&expected), Ok(()));
    observation_mut(&mut expected).increased_leech_rate.raw_bits = 0x7fc01235;
    assert_eq!(plan.recheck(&expected), Err(FieldPlanError::StaleObservation));
    observation_mut(&mut expected).increased_leech_rate.raw_bits = (-0.0_f32).to_bits();
    let plan = change(&expected, AttackLeechTarget::AttackRate, 2.0);
    observation_mut(&mut expected).increased_leech_rate.raw_bits = 0.0_f32.to_bits();
    assert_eq!(plan.recheck(&expected), Err(FieldPlanError::StaleObservation));
}
