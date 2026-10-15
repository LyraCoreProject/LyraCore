use lyracore_native_host::{Environment, Schema};
use spacetimedb::{table, ReducerContext, Table};

#[table(accessor = native_host_test_row, index(accessor = by_group, btree(columns = [group, value])))]
struct Row {
    #[primary_key]
    id: u64,
    #[unique]
    name: String,
    group: u32,
    value: i64,
}

fn environment() -> Environment {
    let ctx = ReducerContext::__dummy();
    let mut schema = Schema::default();
    schema.register(ctx.db.native_host_test_row()).unwrap();
    Environment::new(schema).unwrap()
}

fn row(id: u64, group: u32, value: i64) -> Row {
    Row {
        id,
        name: format!("row-{id}"),
        group,
        value,
    }
}

#[test]
fn typed_composite_ranges_preserve_signed_order_and_prefix_membership() {
    let mut world = environment();
    world
        .transaction(|| {
            let ctx = ReducerContext::__dummy();
            let rows = ctx.db.native_host_test_row();
            for (id, group, value) in [(1, 7, -300), (2, 7, 256), (3, 7, -1), (4, 8, 0)] {
                rows.insert(row(id, group, value));
            }
            let found: Vec<_> = rows
                .by_group()
                .filter((7u32, -300i64..256i64))
                .map(|r| r.id)
                .collect();
            assert_eq!(found, [1, 3]);
            assert_eq!(rows.by_group().filter(&8u32).next().unwrap().id, 4);
            assert_eq!(rows.id().find(2).unwrap().value, 256);
            Ok(())
        })
        .unwrap();
}

#[test]
fn refusal_and_panic_restore_rows_and_indexes() {
    let mut world = environment();
    world
        .transaction(|| {
            ReducerContext::__dummy()
                .db
                .native_host_test_row()
                .insert(row(1, 7, 10));
            Ok(())
        })
        .unwrap();
    let refused: Result<(), String> = world.transaction(|| {
        let ctx = ReducerContext::__dummy();
        let rows = ctx.db.native_host_test_row();
        rows.id().update(row(1, 8, 20));
        rows.insert(row(2, 7, 30));
        Err("fixture refusal".into())
    });
    assert_eq!(refused.unwrap_err(), "fixture refusal");
    let trapped: Result<(), String> = world.transaction(|| {
        ReducerContext::__dummy()
            .db
            .native_host_test_row()
            .id()
            .delete(1);
        panic!("fixture trap");
    });
    assert!(trapped.unwrap_err().contains("fixture trap"));
    world
        .transaction(|| {
            let ctx = ReducerContext::__dummy();
            let rows = ctx.db.native_host_test_row();
            assert_eq!(rows.count(), 1);
            assert_eq!(rows.by_group().filter(&7u32).next().unwrap().value, 10);
            assert_eq!(rows.by_group().filter(&8u32).count(), 0);
            Ok(())
        })
        .unwrap();
}

#[test]
fn duplicate_rows_are_no_ops_but_unique_conflicts_refuse() {
    let mut world = environment();
    world
        .transaction(|| {
            let ctx = ReducerContext::__dummy();
            let rows = ctx.db.native_host_test_row();
            rows.insert(row(1, 7, 10));
            rows.insert(row(1, 7, 10));
            assert!(rows.try_insert(row(1, 7, 20)).is_err());
            let mut collision = row(2, 8, 20);
            collision.name = "row-1".into();
            assert!(rows.try_insert(collision).is_err());
            assert_eq!(rows.count(), 1);
            Ok(())
        })
        .unwrap();
}

#[test]
fn restoring_a_row_in_one_transaction_produces_no_change() {
    let mut world = environment();
    world
        .transaction(|| {
            ReducerContext::__dummy()
                .db
                .native_host_test_row()
                .insert(row(1, 7, 10));
            Ok(())
        })
        .unwrap();
    assert_eq!(world.changes().len(), 1);
    world
        .transaction(|| {
            let ctx = ReducerContext::__dummy();
            let rows = ctx.db.native_host_test_row();
            rows.id().update(row(1, 7, 20));
            rows.id().update(row(1, 7, 10));
            Ok(())
        })
        .unwrap();
    assert!(world.changes().is_empty());
}

#[test]
fn deleting_and_reinserting_the_same_row_produces_no_change() {
    let mut world = environment();
    world
        .transaction(|| {
            ReducerContext::__dummy()
                .db
                .native_host_test_row()
                .insert(row(1, 7, 10));
            Ok(())
        })
        .unwrap();
    world
        .transaction(|| {
            let ctx = ReducerContext::__dummy();
            let rows = ctx.db.native_host_test_row();
            rows.id().delete(1);
            rows.insert(row(1, 7, 10));
            Ok(())
        })
        .unwrap();
    assert!(world.changes().is_empty());
}

#[test]
fn active_iterators_keep_their_rows_while_indexes_change() {
    let mut world = environment();
    world
        .transaction(|| {
            let ctx = ReducerContext::__dummy();
            let rows = ctx.db.native_host_test_row();
            rows.insert(row(1, 7, 10));
            rows.insert(row(2, 7, 20));
            let found = rows.by_group().filter(&7u32);
            rows.id().delete(2);
            rows.insert(row(3, 7, 30));
            assert_eq!(found.map(|r| r.id).collect::<Vec<_>>(), [1, 2]);
            assert_eq!(
                rows.by_group()
                    .filter(&7u32)
                    .map(|r| r.id)
                    .collect::<Vec<_>>(),
                [1, 3]
            );
            Ok(())
        })
        .unwrap();
}
