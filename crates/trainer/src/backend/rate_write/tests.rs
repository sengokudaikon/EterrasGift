use super::*;
struct Locator;
impl ProcessLocator for Locator {
    async fn find(&mut self) -> Result<Vec<ProcessIdentity>, BackendError> {
        panic!("overflow must precede process lookup")
    }
}
struct Connect;
impl Connector for Connect {
    type Connection = Connection;
    async fn connect(&mut self, _: ProcessIdentity, _: Timestamp) -> Result<Connection, BackendError> {
        panic!("overflow must precede preparation")
    }
}
struct Logs;
impl LogSource for Logs {
    fn read(&mut self, _: Timestamp) -> eg_core::game_log::GameLogState {
        panic!("overflow must precede log/transport IO")
    }
}
struct Time;
impl Clock for Time {
    fn now(&self) -> Timestamp {
        "2026-10-19T10:00:00Z".parse().unwrap()
    }
}
struct Connection {
    target: ProcessIdentity,
}
impl BackendConnection for Connection {
    fn target(&self) -> &ProcessIdentity {
        &self.target
    }
    async fn refresh(&mut self, _: Timestamp) -> Result<RuntimeStatus, BackendError> {
        panic!("overflow must precede refresh")
    }
    async fn capture_catalog(&mut self, _: Timestamp) -> Result<GameCatalog, BackendError> {
        panic!("no capture")
    }
    async fn read_forge(&mut self, _: Timestamp) -> Result<Option<ForgeSnapshot>, BackendError> {
        panic!("no forge")
    }
    async fn disconnect(&mut self) -> Cleanup {
        panic!("overflow cannot trigger cleanup IO")
    }
    fn supports_attack_leech_rate(&self) -> bool {
        true
    }
    async fn set_attack_leech_rate(&mut self, _: Timestamp, _: &AttackLeechPlan, _: &mut dyn WriteControl) -> Result<RateWriteStatus, BackendError> {
        panic!("overflow must precede write")
    }
}
#[test]
fn sequence_overflow_refuses_complete_worker_before_log_status_or_store() {
    let target = ProcessIdentity {
        pid: 42,
        created: 1234,
        executable: std::path::PathBuf::from(r"C:\owned\fixture.exe"),
    };
    let id = |n| ObservedObjectIdentity::new(n, n + 1).unwrap();
    let observation = AttackLeechObservation {
        manager: id(10),
        actor: id(20),
        stats: id(30),
        attack_rate: ObservedRate { raw_bits: 1_f32.to_bits() },
        increased_leech_rate: ObservedRate { raw_bits: 0 },
    };
    let current = AttackLeechSnapshot {
        process: target.clone(),
        generation: 1,
        sequence: u64::MAX,
        manager: 10,
        read: AttackLeechRead::Available(observation),
    };
    let AttackLeechDecision::Change(plan) = AttackLeechPlan::prepare(&current, AttackLeechTarget::AttackRate, 1.1).unwrap() else {
        panic!()
    };
    let (handle, mut worker) = service(Locator, Connect, Logs, Time, BackendOptions::default(), None);
    worker.state.enabled = true;
    worker.state.phase = Phase::Ready;
    worker.state.generation = 1;
    worker.state.attached = Some(target.clone());
    worker.state.runtime_status = Some(RuntimeStatus {
        online: false,
        manager: 10,
        features_on: 0,
    });
    worker.state.attack_leech = Some(current);
    worker.attack_leech_sequence = u64::MAX;
    worker.connection = Some(Connection { target });
    let result = tokio::runtime::Builder::new_current_thread().build().unwrap().block_on(worker.set_attack_leech_rate(plan));
    assert_eq!(result.unwrap_err().kind, ErrorKind::Fault);
    assert!(worker.state.last_rate_write.is_none());
    assert!(worker.state.attack_leech.is_none());
    assert_eq!(worker.state.phase, Phase::Blocked);
    assert_eq!(worker.attack_leech_sequence, u64::MAX);
    drop(handle);
}
#[test]
fn default_rate_capability_is_false_and_default_submission_is_preflight_only() {
    struct NoRate(Connection);
    impl BackendConnection for NoRate {
        fn target(&self) -> &ProcessIdentity {
            &self.0.target
        }
        async fn refresh(&mut self, _: Timestamp) -> Result<RuntimeStatus, BackendError> {
            panic!("no refresh")
        }
        async fn capture_catalog(&mut self, _: Timestamp) -> Result<GameCatalog, BackendError> {
            panic!("no capture")
        }
        async fn read_forge(&mut self, _: Timestamp) -> Result<Option<ForgeSnapshot>, BackendError> {
            panic!("no forge")
        }
        async fn disconnect(&mut self) -> Cleanup {
            panic!("no cleanup")
        }
    }
    let mut connection = NoRate(Connection {
        target: ProcessIdentity {
            pid: 1,
            created: 1,
            executable: std::path::PathBuf::from(r"C:\owned\fixture.exe"),
        },
    });
    assert!(!connection.supports_attack_leech_rate());
    let id = |n| ObservedObjectIdentity::new(n, n + 1).unwrap();
    let current = AttackLeechSnapshot {
        process: connection.target().clone(),
        generation: 1,
        sequence: 1,
        manager: 10,
        read: AttackLeechRead::Available(AttackLeechObservation {
            manager: id(10),
            actor: id(20),
            stats: id(30),
            attack_rate: ObservedRate { raw_bits: 1_f32.to_bits() },
            increased_leech_rate: ObservedRate { raw_bits: 0 },
        }),
    };
    let AttackLeechDecision::Change(plan) = AttackLeechPlan::prepare(&current, AttackLeechTarget::AttackRate, 1.1).unwrap() else {
        panic!()
    };
    let mut control = CurrentRateIntent {
        intent: Arc::new(Mutex::new(0)),
        epoch: 0,
    };
    let result = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap()
        .block_on(connection.set_attack_leech_rate(Time.now(), &plan, &mut control));
    assert_eq!(result.unwrap_err().kind, ErrorKind::NotReady);
}
