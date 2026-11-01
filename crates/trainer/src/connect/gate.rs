//! Player.log admission before process access.
use eg_core::game_log::{GameLogReader, GameLogState, GameLogStatus, GameReadiness};
use jiff::Timestamp;
use spiderland_phrases::phrases;

phrases! {
    LogMissing: "Last Epoch's Player.log was not found, so an offline character cannot be confirmed."
    LogStale: "Last Epoch's Player.log belongs to an earlier game session. Restart Last Epoch with logging enabled."
    LogUnreadable: "Last Epoch's Player.log could not be read: {reason}"
    LogUnreadableUnknown: "Last Epoch's Player.log could not be read, so an offline character cannot be confirmed."
    CharacterNotLogged: "Last Epoch entered play without logging which character was loaded. Return to character select and load an offline character again."
    LogNotUnderstood: "Last Epoch's Player.log does not identify the current character, so an offline character cannot be confirmed."
    WaitingForStartup: "Waiting for Last Epoch to finish starting."
    LoadOffline: "Load an offline character to connect to the trainer."
}

pub trait LogSource {
    fn read(&mut self, process_start: Timestamp) -> GameLogState;
}
impl LogSource for GameLogReader {
    fn read(&mut self, process_start: Timestamp) -> GameLogState {
        self.read(process_start)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GateVerdict {
    Allow,
    Wait,
    Refuse,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GateDecision {
    pub verdict: GateVerdict,
    pub readiness: GameReadiness,
    pub log_status: GameLogStatus,
}

/// A refused connection explains the missing evidence instead of exposing the gate's internal readiness fallback.
pub(crate) fn refusal_message(log: &GameLogState) -> String {
    match log.status {
        GameLogStatus::Missing => LogMissing.get().to_owned(),
        GameLogStatus::Stale => LogStale.get().to_owned(),
        GameLogStatus::Unreadable => log
            .problem
            .as_ref()
            .map_or_else(|| LogUnreadableUnknown.get().to_owned(), |reason| LogUnreadable.with(&[("reason", reason)])),
        GameLogStatus::Unrecognized => CharacterNotLogged.get().to_owned(),
        GameLogStatus::Current if log.shell_state.is_none() => LogNotUnderstood.get().to_owned(),
        GameLogStatus::Current => match log.readiness {
            GameReadiness::Starting => WaitingForStartup.get(),
            GameReadiness::NoCharacter | GameReadiness::Online | GameReadiness::Offline => LoadOffline.get(),
        }
        .to_owned(),
    }
}

pub fn evaluate(log: &GameLogState, started: Timestamp, now: Timestamp) -> GateDecision {
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
        (GateVerdict::Refuse, GameReadiness::Starting)
    };
    GateDecision {
        verdict,
        readiness,
        log_status: log.status,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_online_log_formats_refuse_admission_until_an_offline_character_loads() {
        let started: Timestamp = "2026-10-22T19:18:13Z".parse().unwrap();
        let now: Timestamp = "2026-10-22T19:20:00Z".parse().unwrap();
        for character in [
            "Loading character: Online Mage B6C126989DAFF026 isOnline: True, cycle: Swordfish, class: Mage",
            "Character Created: Online Mage B6C126989DAFF026 isOnline: True, cycle: Swordfish, class: 1",
        ] {
            let mut state = GameLogState::started();
            for message in [
                "Offline Mode: False",
                "ShellSceneManager: State transition Login -> CharacterSelect",
                "[Network] Initializing network service group for online client.",
                character,
                "ShellSceneManager: State transition CharacterSelect -> None",
            ] {
                if let Some((_, Some(event))) = eg_core::game_log::parse(&format!("{now}\tLog\t{message}")) {
                    state = state.apply(event);
                }
                assert_ne!(evaluate(&state, started, now).verdict, GateVerdict::Allow);
            }
            let decision = evaluate(&state, started, now);
            assert_eq!(
                (decision.verdict, decision.readiness, decision.log_status),
                (GateVerdict::Wait, GameReadiness::Online, GameLogStatus::Current)
            );
            for message in [
                "ShellSceneManager: State transition None -> CharacterSelect",
                "[Network] Initializing network service group for offline client.",
                "Loading character: Offline Mage 27 isOnline: False, cycle: Swordfish, class: Mage",
                "ShellSceneManager: State transition CharacterSelect -> None",
            ] {
                if let Some((_, Some(event))) = eg_core::game_log::parse(&format!("{now}\tLog\t{message}")) {
                    state = state.apply(event);
                }
            }
            assert_eq!(evaluate(&state, started, now).verdict, GateVerdict::Allow);
        }
    }
}
