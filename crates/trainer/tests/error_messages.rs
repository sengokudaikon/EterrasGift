use eg_core::game_log::GameReadiness;
use eg_game_access::{
    protocol::ResponseCode,
    transport::{Delivery, TransportError, TransportFailure},
};
use eg_trainer::session::SessionError;

#[test]
fn session_messages_explain_the_fault_without_debug_enum_wrappers() {
    assert_eq!(SessionError::Mapping("Actual values are unavailable.".into()).to_string(), "Actual values are unavailable.");
    assert!(SessionError::NotOffline(GameReadiness::Online).to_string().contains("offline character"));
    let message = SessionError::Helper {
        code: ResponseCode::Failed,
        message: "The helper failed.".into(),
        may_have_run: true,
    }
    .to_string();
    assert!(message.contains("may have reached"));
    assert!(!message.contains("Helper {"));
    let message = SessionError::Transport(TransportError {
        delivery: Delivery::MayHaveRun,
        failure: TransportFailure::Timeout,
    })
    .to_string();
    assert!(message.contains("did not respond in time"));
    assert!(message.contains("do not automatically retry"));
    assert!(!message.contains("MayHaveRun"));
}
