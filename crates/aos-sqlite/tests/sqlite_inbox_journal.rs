mod common;

use aos_cbor::Hash;
use aos_node::{
    CborPayload, CommandIngress, CommandRecord, CommandStatus, CommandStore, InboxItem,
    PersistConflict, PersistError, TimerFiredIngress, UniverseId, WorldAdminStore, WorldId,
    WorldLineage, WorldStore,
};
use aos_sqlite::SqliteNodeStore;
use uuid::Uuid;

use common::{open_store, temp_state_root, universe};

fn world() -> WorldId {
    WorldId::from(Uuid::parse_str("22222222-2222-2222-2222-222222222222").unwrap())
}

fn prepare_bootstrap_world(
    store: &SqliteNodeStore,
    universe: UniverseId,
    world: WorldId,
    handle: &str,
) -> Hash {
    assert_eq!(universe, common::universe());
    let manifest_hash = store
        .cas_put_verified(universe, b"bootstrap-manifest")
        .expect("store manifest");
    store
        .world_prepare_manifest_bootstrap(
            universe,
            world,
            manifest_hash,
            handle.into(),
            None,
            22,
            WorldLineage::Genesis { created_at_ns: 22 },
        )
        .expect("prepare bootstrap world");
    manifest_hash
}

fn queued_command(command_id: &str, command: &str, submitted_at_ns: u64) -> CommandRecord {
    CommandRecord {
        command_id: command_id.into(),
        command: command.into(),
        status: CommandStatus::Queued,
        submitted_at_ns,
        started_at_ns: None,
        finished_at_ns: None,
        journal_height: None,
        manifest_hash: None,
        result_payload: None,
        error: None,
    }
}

fn command_ingress(command_id: &str, payload: Vec<u8>, submitted_at_ns: u64) -> CommandIngress {
    CommandIngress {
        command_id: command_id.into(),
        command: "event-send".into(),
        actor: Some("tester".into()),
        payload: CborPayload::inline(payload),
        submitted_at_ns,
    }
}

fn timer(seed: u8) -> InboxItem {
    InboxItem::TimerFired(TimerFiredIngress {
        timer_id: format!("timer-{seed}"),
        payload: aos_node::CborPayload::inline(vec![seed]),
        correlation_id: Some(format!("corr-{seed}")),
    })
}

#[test]
fn inbox_enqueue_orders_items_and_supports_pagination() {
    let (_temp, paths) = temp_state_root();
    let store = open_store(&paths);
    prepare_bootstrap_world(&store, universe(), world(), "hello-world");

    let seq1 = store.inbox_enqueue(universe(), world(), timer(1)).unwrap();
    let seq2 = store.inbox_enqueue(universe(), world(), timer(2)).unwrap();
    let seq3 = store
        .inbox_enqueue(
            universe(),
            world(),
            InboxItem::Control(command_ingress("cmd-3", vec![7; 5_000], 3)),
        )
        .unwrap();

    let first_page = store
        .inbox_read_after(universe(), world(), None, 2)
        .unwrap();
    assert_eq!(first_page.len(), 2);
    assert_eq!(first_page[0].0, seq1);
    assert_eq!(first_page[1].0, seq2);

    let second_page = store
        .inbox_read_after(universe(), world(), Some(seq2), 2)
        .unwrap();
    assert_eq!(second_page.len(), 1);
    assert_eq!(second_page[0].0, seq3);
    match &second_page[0].1 {
        InboxItem::Control(control) => {
            assert!(control.payload.inline_cbor.is_none());
            assert!(control.payload.cbor_ref.is_some());
            assert_eq!(control.payload.cbor_size, Some(5_000));
        }
        other => panic!("expected control ingress, got {other:?}"),
    }
}

#[test]
fn inbox_commit_cursor_and_drain_to_journal_use_compare_and_swap() {
    let (_temp, paths) = temp_state_root();
    let store = open_store(&paths);
    prepare_bootstrap_world(&store, universe(), world(), "hello-world");

    let seq1 = store.inbox_enqueue(universe(), world(), timer(1)).unwrap();
    let seq2 = store.inbox_enqueue(universe(), world(), timer(2)).unwrap();
    store
        .inbox_commit_cursor(universe(), world(), None, seq1.clone())
        .unwrap();

    let err = store
        .inbox_commit_cursor(universe(), world(), None, seq2.clone())
        .unwrap_err();
    assert!(matches!(
        err,
        PersistError::Conflict(PersistConflict::InboxCursorAdvanced { .. })
    ));

    store
        .journal_append_batch(universe(), world(), 0, &[b"existing".to_vec()])
        .unwrap();
    let err = store
        .drain_inbox_to_journal(
            universe(),
            world(),
            Some(seq1),
            seq2,
            0,
            &[b"new-entry".to_vec()],
        )
        .unwrap_err();
    assert!(matches!(
        err,
        PersistError::Conflict(PersistConflict::HeadAdvanced {
            expected: 0,
            actual: 1
        })
    ));
    assert_eq!(store.journal_head(universe(), world()).unwrap(), 1);
}

#[test]
fn journal_append_and_command_records_survive_reopen() {
    let (_temp, paths) = temp_state_root();
    let store = open_store(&paths);
    prepare_bootstrap_world(&store, universe(), world(), "hello-world");

    store
        .journal_append_batch(
            universe(),
            world(),
            0,
            &[b"entry-0".to_vec(), b"entry-1".to_vec()],
        )
        .unwrap();
    store
        .journal_append_batch(universe(), world(), 2, &[b"entry-2".to_vec()])
        .unwrap();

    let ingress = command_ingress("cmd-1", vec![1, 2, 3], 44);
    let initial = queued_command("cmd-1", "event-send", 44);
    let queued = store
        .submit_command(universe(), world(), ingress.clone(), initial.clone())
        .unwrap();
    let queued_again = store
        .submit_command(universe(), world(), ingress, initial.clone())
        .unwrap();
    assert_eq!(queued, queued_again);
    assert_eq!(
        store
            .inbox_read_after(universe(), world(), None, 16)
            .unwrap()
            .len(),
        1
    );

    let err = store
        .submit_command(
            universe(),
            world(),
            command_ingress("cmd-1", vec![9, 9], 44),
            initial,
        )
        .unwrap_err();
    assert!(matches!(
        err,
        PersistError::Conflict(PersistConflict::CommandRequestMismatch { .. })
    ));

    let mut succeeded = queued;
    succeeded.status = aos_node::CommandStatus::Succeeded;
    succeeded.started_at_ns = Some(45);
    succeeded.finished_at_ns = Some(46);
    succeeded.journal_height = Some(2);
    store
        .update_command_record(universe(), world(), succeeded.clone())
        .unwrap();
    drop(store);

    let reopened = open_store(&paths);
    assert_eq!(
        reopened
            .journal_read_range(universe(), world(), 1, 2)
            .unwrap(),
        vec![(1, b"entry-1".to_vec()), (2, b"entry-2".to_vec())]
    );
    assert_eq!(
        reopened
            .command_record(universe(), world(), "cmd-1")
            .unwrap(),
        Some(succeeded)
    );
}
