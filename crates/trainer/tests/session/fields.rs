use super::*;
use eg_trainer::write::{WriteControl, WriteOutcome};
use eg_trainer_wire::field_edits::{Edit, Field, FieldSet, Snapshot};
struct Allowed;
impl WriteControl for Allowed {
    fn allowed(&mut self) -> bool {
        true
    }
}
fn snapshot() -> Snapshot {
    Snapshot {
        set: FieldSet::Keys,
        manager: 0x1234,
        container: 10,
        fields: vec![Field {
            key: 11,
            identity: 12,
            item_type: 104,
            sub_type: 2,
            label: "Key".into(),
            value: 20,
        }],
    }
}
#[test]
fn count_edits_require_independent_subject_and_quantity_readback_and_never_retry_a_failed_store() {
    for (actual, wanted) in [(0, 0), (40, 40), (30, 40)] {
        let fixture = Fixture::rehashing();
        let wire = FakeTransport::new(fixture.target.clone(), [fixture.discover(), status(false, 0x1234, 0)]);
        let mut session = fixture.session(&wire);
        session.enable();
        run(session.connect()).unwrap();
        let expected = snapshot();
        wire.push(done(expected.encode().unwrap()));
        assert_eq!(run(session.read_fields(FieldSet::Keys)).unwrap(), expected);
        let edit = Edit {
            expected: expected.clone(),
            values: vec![Some(wanted)],
        };
        let mut observed = expected;
        observed.fields[0].value = actual;
        if actual == 0 {
            observed.fields.clear();
        }
        wire.push(done(vec![]));
        wire.push(done(observed.encode().unwrap()));
        let result = run(session.apply_fields(&edit, &mut Allowed)).unwrap();
        assert_eq!(
            result,
            if actual == wanted {
                WriteOutcome::Confirmed(observed)
            } else {
                WriteOutcome::NotKept(observed)
            }
        );
        let sent = wire.requests();
        assert!(matches!(sent[sent.len() - 2], Request::WriteFields { .. }));
        assert!(matches!(sent[sent.len() - 1], Request::ReadFields { set: FieldSet::Keys, .. }));
        wire.push(Step::Reply(Ok(Response {
            code: ResponseCode::Failed,
            payload: b"setter may have run".to_vec(),
        })));
        let before = wire.requests().len();
        assert!(matches!(run(session.apply_fields(&edit, &mut Allowed)).unwrap(), WriteOutcome::Uncertain { .. }));
        assert_eq!(wire.requests().len(), before + 1);
        assert!(run(session.apply_fields(&edit, &mut Allowed)).is_err());
        assert_eq!(wire.requests().len(), before + 1);
    }
}
