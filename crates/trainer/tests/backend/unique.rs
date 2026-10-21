use super::*;
use eg_trainer::edits::{RollChange, RollEdit, RollTarget};
fn shared() -> Arc<Shared> {
    let shared = writes::writable();
    {
        let mut item = shared.forge_item.lock().unwrap();
        let item = item.as_mut().unwrap();
        item.rarity = 9;
        item.identity = Some(backend::UniqueIdentity { id: 911, rolls: vec![17, 81] });
        item.fingerprint = item.legacy_fingerprint();
    }
    *shared.unique_lines.lock().unwrap() = Some(vec![backend::ModifierSnapshot {
        index: 0,
        text: "Current unique Ω".into(),
        range: "[0 to 255]".into(),
        roll_ids: vec![0, 1],
        editable: true,
    }]);
    shared
}
fn draft(value: i32) -> RollEdit {
    RollEdit {
        target: RollTarget::Unique,
        changes: vec![RollChange { index: 0, value }],
        allow_incompatible: false,
    }
}
async fn describe(handle: &BackendHandle) {
    command(
        handle,
        Command::DescribeUnique {
            expected: Box::new(handle.snapshot().expected_observation().unwrap()),
        },
    )
    .await
    .unwrap();
}
fn preview(handle: &BackendHandle, value: i32) -> Command {
    Command::PreviewUnique {
        expected: Box::new(handle.snapshot().expected_observation().unwrap()),
        draft: draft(value),
    }
}
#[test]
fn descriptions_and_preview_are_cached_for_an_exact_item_and_never_create_writes() {
    run(async {
        let shared = shared();
        let (handle, worker) = start(&shared, options());
        connected(&handle).await;
        let before = handle.snapshot();
        describe(&handle).await;
        describe(&handle).await;
        assert_eq!(shared.count("unique-description"), 1);
        command(&handle, preview(&handle, 223)).await.unwrap();
        command(&handle, preview(&handle, 223)).await.unwrap();
        assert_eq!(shared.count("unique-preview"), 1);
        let state = handle.snapshot();
        let result = state.unique_preview.as_ref().unwrap().result.as_ref().unwrap();
        assert_eq!(result.preview().modifiers[0].text, "Draft 223 Ω");
        assert_eq!(state.last_write, before.last_write);
        assert_eq!(state.undo_forge, before.undo_forge);
        command(&handle, Command::ReadForge).await.unwrap();
        describe(&handle).await;
        assert_eq!(shared.count("unique-description"), 1);
        assert_eq!(handle.snapshot().unique_preview, state.unique_preview);
        *shared.forge_item.lock().unwrap() = None;
        command(&handle, Command::ReadForge).await.unwrap();
        assert!(handle.snapshot().unique_preview.is_none());
        shutdown(&handle, worker).await;
    });
}
#[test]
fn unavailable_descriptions_and_preview_preserve_connection_and_refresh_allows_recovery() {
    run(async {
        let shared = shared();
        *shared.unique_lines.lock().unwrap() = None;
        let (handle, worker) = start(&shared, options());
        connected(&handle).await;
        describe(&handle).await;
        assert!(matches!(&handle.snapshot().forge, ForgeState::Item { unique: Some(Err(_)), .. }));
        assert_eq!(handle.snapshot().phase, Phase::Ready);
        describe(&handle).await;
        assert_eq!(shared.count("unique-description"), 1);
        *shared.unique_lines.lock().unwrap() = Some(vec![backend::ModifierSnapshot {
            index: 0,
            text: "Recovered".into(),
            range: "".into(),
            roll_ids: vec![0],
            editable: true,
        }]);
        command(&handle, Command::ReadForge).await.unwrap();
        describe(&handle).await;
        assert_eq!(shared.count("unique-description"), 2);
        shared
            .preview_errors
            .lock()
            .unwrap()
            .push_back(BackendError::blocked(ErrorKind::MappingUnavailable, "formatter refused"));
        command(&handle, preview(&handle, 223)).await.unwrap();
        assert!(handle.snapshot().unique_preview.as_ref().unwrap().result.is_err());
        assert_eq!(handle.snapshot().phase, Phase::Ready);
        command(&handle, Command::ReadForge).await.unwrap();
        command(&handle, preview(&handle, 223)).await.unwrap();
        assert!(handle.snapshot().unique_preview.as_ref().unwrap().result.is_ok());
        shutdown(&handle, worker).await;
    });
}
#[test]
fn queued_cleanup_suppresses_an_older_preview_completion_and_changed_item_rejects_without_io() {
    run(async {
        let shared = shared();
        let (handle, worker) = start(&shared, options());
        connected(&handle).await;
        describe(&handle).await;
        let old = preview(&handle, 223);
        {
            let mut item = shared.forge_item.lock().unwrap();
            let item = item.as_mut().unwrap();
            item.status.level_requirement += 1;
            item.fingerprint = item.legacy_fingerprint();
        }
        command(&handle, Command::ReadForge).await.unwrap();
        assert_eq!(command(&handle, old).await.unwrap_err().kind, ErrorKind::InvalidEdit);
        assert_eq!(shared.count("unique-preview"), 0);
        describe(&handle).await;
        shared.unique_block.arm();
        let pending = handle.try_submit(preview(&handle, 223)).unwrap();
        shared.unique_block.wait_started().await;
        let disconnect = handle.try_submit(Command::Disconnect).unwrap();
        shared.unique_block.finish();
        assert_eq!(pending.wait().await.unwrap_err().kind, ErrorKind::Superseded);
        disconnect.wait().await.unwrap();
        assert!(handle.snapshot().unique_preview.is_none());
        assert_eq!(shared.count("write"), 0);
        shutdown(&handle, worker).await;
    });
}
