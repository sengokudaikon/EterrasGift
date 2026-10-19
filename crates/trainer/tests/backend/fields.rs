use super::*;
use eg_game_access::fields::{AttackLeechObservation, AttackLeechRead, AttackLeechUnavailable, ObservedObjectIdentity, ObservedRate};

fn available(manager: u64) -> AttackLeechRead {
    AttackLeechRead::Available(AttackLeechObservation {
        manager: ObservedObjectIdentity::new(manager, 0x2000).unwrap(),
        actor: ObservedObjectIdentity::new(0x3000, 0x4000).unwrap(),
        stats: ObservedObjectIdentity::new(0x5000, 0x6000).unwrap(),
        attack_rate: ObservedRate { raw_bits: 0x7fc01234 },
        increased_leech_rate: ObservedRate { raw_bits: (-0.0_f32).to_bits() },
    })
}

#[test]
fn attack_leech_capability_is_explicit_and_never_enables_generic_fields_or_writes() {
    run(async {
        let shared = Shared::new();
        let (handle, worker) = start(&shared, options(), None);
        connected(&handle).await;
        assert!(!handle.snapshot().capabilities.read_attack_leech);
        assert_eq!(command(&handle, Command::ReadAttackLeech).await.unwrap_err().kind, ErrorKind::NotReady);
        assert_eq!(shared.count("attack-leech"), 0);
        shared.attack_leech_enabled.store(true, Ordering::SeqCst);
        command(&handle, Command::ReadForge).await.unwrap();
        let caps = handle.snapshot().capabilities;
        assert!(caps.read_attack_leech);
        assert!(!caps.fields && !caps.hooks && !caps.write_forge && !caps.level_requirement);
        shutdown(&handle, worker).await;
    });
}

#[test]
fn attack_leech_observations_bind_process_generation_and_monotonic_sequence_including_local_unavailable() {
    run(async {
        let shared = Shared::new();
        shared.attack_leech_enabled.store(true, Ordering::SeqCst);
        *shared.attack_leech_read.lock().unwrap() = available(0x1000);
        let (handle, worker) = start(&shared, options(), None);
        connected(&handle).await;
        command(&handle, Command::ReadAttackLeech).await.unwrap();
        let first = handle.snapshot().attack_leech.unwrap();
        assert_eq!(first.process, target());
        assert_eq!(first.generation, handle.snapshot().generation);
        assert_eq!(first.manager, 0x1000);
        assert_eq!(first.sequence, 1);
        assert_eq!(first.read, available(0x1000));
        for reason in [
            AttackLeechUnavailable::ActorCacheAbsent,
            AttackLeechUnavailable::StatsAbsent,
            AttackLeechUnavailable::NativeObjectDestroyed,
            AttackLeechUnavailable::GraphChanged,
            AttackLeechUnavailable::UnsupportedStatsClass,
        ] {
            *shared.attack_leech_read.lock().unwrap() = AttackLeechRead::Unavailable(reason);
            let previous = handle.snapshot().attack_leech.unwrap().sequence;
            command(&handle, Command::ReadAttackLeech).await.unwrap();
            let state = handle.snapshot();
            assert_eq!(state.phase, Phase::Ready);
            assert_eq!(state.attack_leech.as_ref().unwrap().sequence, previous + 1);
            assert_eq!(state.attack_leech.unwrap().read, AttackLeechRead::Unavailable(reason));
        }
        let previous = handle.snapshot().attack_leech.unwrap().sequence;
        command(&handle, Command::Disconnect).await.unwrap();
        assert!(handle.snapshot().attack_leech.is_none());
        command(&handle, Command::Connect).await.unwrap();
        command(&handle, Command::ReadAttackLeech).await.unwrap();
        let next = handle.snapshot().attack_leech.unwrap();
        assert_eq!(next.sequence, previous + 1);
        assert_ne!(next.generation, first.generation);
        shutdown(&handle, worker).await;
    });
}

#[test]
fn attack_leech_manager_drift_clears_previous_observation_and_wrong_wrapper_never_publishes() {
    run(async {
        let shared = Shared::new();
        shared.attack_leech_enabled.store(true, Ordering::SeqCst);
        *shared.attack_leech_read.lock().unwrap() = available(0x1000);
        let (handle, worker) = start(&shared, options(), None);
        connected(&handle).await;
        command(&handle, Command::ReadAttackLeech).await.unwrap();
        shared.status.lock().unwrap().manager = 0x9999;
        command(&handle, Command::ReadForge).await.unwrap();
        assert!(handle.snapshot().attack_leech.is_none());
        assert_eq!(command(&handle, Command::ReadAttackLeech).await.unwrap_err().kind, ErrorKind::Protocol);
        assert!(handle.snapshot().attack_leech.is_none());
        assert!(handle.snapshot().attached.is_none());
        assert_eq!(handle.snapshot().phase, Phase::Blocked);
        shutdown(&handle, worker).await;
    });
}

#[test]
fn attack_leech_global_refusals_revoke_backend_admission_without_cleanup_io() {
    run(async {
        for reason in [
            AttackLeechUnavailable::OfflineRequired,
            AttackLeechUnavailable::ManagerAbsent,
            AttackLeechUnavailable::ManagerChanged,
        ] {
            let shared = Shared::new();
            shared.attack_leech_enabled.store(true, Ordering::SeqCst);
            *shared.attack_leech_read.lock().unwrap() = AttackLeechRead::Unavailable(reason);
            let (handle, worker) = start(&shared, options(), None);
            connected(&handle).await;
            assert_eq!(command(&handle, Command::ReadAttackLeech).await.unwrap_err().kind, ErrorKind::Gate);
            assert_eq!(handle.snapshot().phase, Phase::WaitingForCharacter);
            assert!(handle.snapshot().attack_leech.is_none());
            assert!(handle.snapshot().attached.is_none());
            assert_eq!(shared.count("disconnect"), 0);
            shutdown(&handle, worker).await;
        }
    });
}

#[test]
fn attack_leech_lost_log_after_read_discards_reply_without_cleanup_io() {
    run(async {
        let shared = Shared::new();
        shared.attack_leech_enabled.store(true, Ordering::SeqCst);
        *shared.attack_leech_read.lock().unwrap() = available(0x1000);
        let (handle, worker) = start(&shared, options(), None);
        connected(&handle).await;
        shared.attack_leech_block.arm();
        let receipt = handle.try_submit(Command::ReadAttackLeech).unwrap();
        shared.attack_leech_block.wait_started().await;
        *shared.log.lock().unwrap() = log(GameReadiness::Online);
        shared.attack_leech_block.finish();
        assert_eq!(receipt.wait().await.unwrap_err().kind, ErrorKind::Gate);
        assert!(handle.snapshot().attack_leech.is_none());
        assert!(handle.snapshot().attached.is_none());
        assert_eq!(shared.count("disconnect"), 0);
        shutdown(&handle, worker).await;
    });
}

#[test]
fn attack_leech_cleanup_barrier_suppresses_inflight_publication_and_drains_once() {
    run(async {
        let shared = Shared::new();
        shared.attack_leech_enabled.store(true, Ordering::SeqCst);
        *shared.attack_leech_read.lock().unwrap() = available(0x1000);
        let (handle, worker) = start(&shared, options(), None);
        connected(&handle).await;
        shared.attack_leech_block.arm();
        let read = handle.try_submit(Command::ReadAttackLeech).unwrap();
        shared.attack_leech_block.wait_started().await;
        let disconnect = handle.try_submit(Command::Disconnect).unwrap();
        shared.attack_leech_block.finish();
        assert_eq!(read.wait().await.unwrap_err().kind, ErrorKind::Superseded);
        disconnect.wait().await.unwrap();
        assert!(handle.snapshot().attack_leech.is_none());
        assert_eq!(shared.count("attack-leech"), 1);
        assert_eq!(shared.count("disconnect"), 1);
        assert_eq!(shared.aborted.load(Ordering::SeqCst), 0);
        shutdown(&handle, worker).await;
    });
}
