use super::*;
use eg_trainer_screens::{
    backend_session::BackendSession,
    session::{FieldRequest, FieldResult, FieldWrite, TrainerSession},
};
use eg_trainer_wire::field_edits::{Field, FieldSet, Snapshot};
async fn answer(request: FieldRequest) -> FieldResult {
    match request {
        FieldRequest::Done(result) => result,
        FieldRequest::Pending(answer) => answer.await,
    }
}
#[test]
fn numeric_editors_share_the_serialized_worker_and_reject_stale_scope_before_submission() {
    run(async {
        let shared = Shared::new();
        for set in FieldSet::ALL {
            let count = set.field_count().unwrap_or(2) as u64;
            shared.counts.lock().unwrap().insert(
                set,
                Snapshot {
                    set,
                    manager: 0x1000,
                    container: 100 + set as u64,
                    fields: (0..count)
                        .map(|index| Field {
                            key: 200 + index,
                            identity: 300 + index,
                            item_type: if set == FieldSet::Keys { 104 } else { 107 },
                            sub_type: index as u16,
                            label: if set.held_items() { format!("Item #{index}") } else { String::new() },
                            value: if set == FieldSet::MonolithRun && index == 2 { 1 } else { 20 },
                        })
                        .collect(),
                },
            );
        }
        let (handle, worker) = start(&shared, options());
        connected(&handle).await;
        let session = BackendSession::new(handle.clone());
        for set in FieldSet::ALL {
            assert!(session.unavailable(set.id()).is_none());
            let read = answer(session.read_fields(set.id())).await.unwrap();
            let mut stale = handle.snapshot().fields[&set].clone();
            stale.generation += 1;
            let before = shared.count("fields-write");
            assert_eq!(
                command(
                    &handle,
                    Command::WriteFields {
                        expected: Box::new(stale),
                        values: vec![Some(7); read.len()]
                    }
                )
                .await
                .unwrap_err()
                .kind,
                ErrorKind::InvalidEdit
            );
            assert_eq!(shared.count("fields-write"), before);
            let mut writes: Vec<_> = read
                .iter()
                .enumerate()
                .map(|(index, field)| FieldWrite {
                    read: field.value,
                    wanted: if index == 0 || (set == FieldSet::MonolithRun && index == 2) { 0.0 } else { 40.0 },
                    label: if set.held_items() { field.label.clone() } else { "Translated field label".into() },
                })
                .collect();
            writes[0].read = 99.0;
            assert!(answer(session.write_fields(set.id(), &writes)).await.unwrap_err().nothing_sent);
            assert_eq!(shared.count("fields-write"), before);
            writes[0].read = 20.0;
            let actual = answer(session.write_fields(set.id(), &writes)).await.unwrap();
            assert_eq!(shared.count("fields-write"), before + 1);
            assert_eq!(actual.len(), if set.held_items() { 1 } else { read.len() });
            assert_eq!(actual.last().unwrap().value, if set == FieldSet::MonolithRun { 0.0 } else { 40.0 });
            if !set.held_items() {
                assert_eq!(actual[0].value, 0.0, "numeric zero keeps its field");
            }
            assert_eq!(handle.snapshot().phase, Phase::Ready);
        }
        assert!(!session.is_busy());
        shared.counts_kept.store(false, Ordering::SeqCst);
        let read = answer(session.read_fields("keys")).await.unwrap();
        let writes = read
            .iter()
            .map(|field| FieldWrite {
                read: field.value,
                wanted: 60.0,
                label: field.label.clone(),
            })
            .collect::<Vec<_>>();
        assert!(
            !answer(session.write_fields("keys", &writes)).await.unwrap_err().nothing_sent,
            "different subject/count readback cannot be shown confirmed"
        );
        assert_eq!(handle.snapshot().phase, Phase::Ready, "known readback still leaves the connection usable");
        let pending = session.read_fields("woven-echoes");
        assert!(session.is_busy());
        drop(pending);
        assert!(!session.is_busy(), "dropping a view's answer cannot leave the UI busy");
        shutdown(&handle, worker).await;
    });
}
