//! Player.log admission before process access, ported from InjectionGate.Evaluate.
use eg_core::game_log::{GameLogReader, GameLogState, GameLogStatus, GameReadiness};
use jiff::Timestamp;

pub trait LogSource {
    fn read(&mut self, process_start: Timestamp) -> GameLogState;
}
impl LogSource for GameLogReader {
    fn read(&mut self, process_start: Timestamp) -> GameLogState {
        self.read(process_start)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum GatePolicy {
    #[default]
    Release,
    Development,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GateVerdict {
    Allow,
    Wait,
    Probe,
    Refuse,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GateDecision {
    pub verdict: GateVerdict,
    pub readiness: GameReadiness,
    pub log_status: GameLogStatus,
}

pub fn evaluate(log: &GameLogState, started: Timestamp, now: Timestamp, policy: GatePolicy) -> GateDecision {
    let starting = now.as_nanosecond() - started.as_nanosecond() < 60_000_000_000;
    let (verdict, readiness) = if log.status == GameLogStatus::Current && (log.shell_state.is_some() || starting) {
        (
            if log.readiness == GameReadiness::Offline {
                GateVerdict::Allow
            } else {
                GateVerdict::Wait
            },
            log.readiness,
        )
    } else if starting {
        (GateVerdict::Wait, GameReadiness::Starting)
    } else {
        (
            if policy == GatePolicy::Development { GateVerdict::Probe } else { GateVerdict::Refuse },
            GameReadiness::Starting,
        )
    };
    GateDecision {
        verdict,
        readiness,
        log_status: log.status,
    }
}
