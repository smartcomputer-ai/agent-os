use aos_node::{PersistError, UniverseAdminLifecycle, UniverseId};
use rusqlite::{Connection, OptionalExtension};

use super::util::{encode, parse_universe_id};

pub(super) const SQLITE_SCHEMA_VERSION: i64 = 6;

pub(super) fn initialize(connection: &Connection) -> Result<(), PersistError> {
    connection
        .execute(
            "create table if not exists local_meta (
                singleton integer primary key check (singleton = 1),
                schema_version integer not null
            )",
            [],
        )
        .map_err(|err| PersistError::backend(format!("init sqlite meta table: {err}")))?;

    let current_version: Option<i64> = connection
        .query_row(
            "select schema_version from local_meta where singleton = 1",
            [],
            |row| row.get(0),
        )
        .optional()
        .map_err(|err| PersistError::backend(format!("read sqlite schema version: {err}")))?;

    if current_version != Some(SQLITE_SCHEMA_VERSION) {
        reset_schema(connection)?;
        apply_schema(connection)?;
    }
    Ok(())
}

fn reset_schema(connection: &Connection) -> Result<(), PersistError> {
    connection
        .execute_batch(
            "
            pragma foreign_keys = off;
            drop table if exists local_node_state;
            drop table if exists local_world_handles;
            drop table if exists local_universe_handles;
            drop table if exists local_secret_audit;
            drop table if exists local_secret_versions;
            drop table if exists local_secret_bindings;
            drop table if exists local_segments;
            drop table if exists local_snapshots;
            drop table if exists local_command_records;
            drop table if exists local_inbox_entries;
            drop table if exists local_journal_entries;
            drop table if exists local_worlds;
            drop table if exists local_universes;
            drop table if exists local_meta;
            pragma foreign_keys = on;
            ",
        )
        .map_err(|err| PersistError::backend(format!("reset sqlite schema: {err}")))?;
    Ok(())
}

fn apply_schema(connection: &Connection) -> Result<(), PersistError> {
    connection
        .execute_batch(
            "
            create table local_meta (
                singleton integer primary key check (singleton = 1),
                schema_version integer not null,
                universe_id text not null,
                universe_handle text not null,
                created_at_ns integer not null,
                admin blob not null
            );
            create table local_worlds (
                world_id text not null,
                handle text not null,
                manifest_hash text,
                active_baseline_height integer,
                placement_pin text,
                created_at_ns integer not null,
                lineage blob,
                admin blob not null,
                journal_head integer not null,
                inbox_cursor integer,
                next_inbox_seq integer not null,
                notify_counter integer not null,
                pending_effects_count integer not null default 0,
                next_timer_due_at_ns integer,
                primary key (world_id)
            );
            create table local_world_handles (
                handle text not null,
                world_id text not null,
                primary key (handle),
                unique (world_id),
                foreign key (world_id) references local_worlds(world_id) on delete cascade
            );
            create table local_journal_entries (
                world_id text not null,
                height integer not null,
                bytes blob not null,
                primary key (world_id, height),
                foreign key (world_id) references local_worlds(world_id)
            );
            create table local_inbox_entries (
                world_id text not null,
                seq integer not null,
                item blob not null,
                primary key (world_id, seq),
                foreign key (world_id) references local_worlds(world_id)
            );
            create table local_command_records (
                world_id text not null,
                command_id text not null,
                request_hash text not null,
                record blob not null,
                primary key (world_id, command_id),
                foreign key (world_id) references local_worlds(world_id)
            );
            create table local_snapshots (
                world_id text not null,
                height integer not null,
                record blob not null,
                primary key (world_id, height),
                foreign key (world_id) references local_worlds(world_id)
            );
            create table local_segments (
                world_id text not null,
                end_height integer not null,
                record blob not null,
                primary key (world_id, end_height),
                foreign key (world_id) references local_worlds(world_id)
            );
            create table local_secret_bindings (
                binding_id text not null,
                record blob not null,
                primary key (binding_id)
            );
            create table local_secret_versions (
                binding_id text not null,
                version integer not null,
                record blob not null,
                primary key (binding_id, version),
                foreign key (binding_id) references local_secret_bindings(binding_id)
            );
            create table local_secret_audit (
                ts_ns integer not null,
                binding_id text not null,
                version_key integer not null,
                record blob not null,
                primary key (ts_ns, binding_id, version_key)
            );
            ",
        )
        .map_err(|err| PersistError::backend(format!("apply sqlite schema: {err}")))?;

    let universe_id =
        UniverseId::from(uuid::Uuid::parse_str("11111111-1111-1111-1111-111111111111").unwrap());
    let handle = "local".to_string();
    let admin = encode(&UniverseAdminLifecycle::default())?;
    connection
        .execute(
            "insert into local_meta (
                singleton, schema_version, universe_id, universe_handle, created_at_ns, admin
            ) values (1, ?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![
                SQLITE_SCHEMA_VERSION,
                universe_id.to_string(),
                handle,
                0_u64,
                admin
            ],
        )
        .map_err(|err| PersistError::backend(format!("write sqlite schema version: {err}")))?;
    Ok(())
}

pub(super) fn ensure_local_universe(connection: &Connection) -> Result<UniverseId, PersistError> {
    let existing: Option<String> = connection
        .query_row(
            "select universe_id from local_meta where singleton = 1",
            [],
            |row| row.get(0),
        )
        .optional()
        .map_err(|err| PersistError::backend(format!("load local universe: {err}")))?;
    match existing {
        Some(existing) => parse_universe_id(&existing),
        None => Err(PersistError::backend(
            "local sqlite schema missing singleton universe metadata",
        )),
    }
}
