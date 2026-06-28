use bstr::ByteSlice;
use gix_hash::Kind;
use gix_reftable::{LogRecord, LogValue, Options, RefRecord, RefValue, Stack};

#[test]
fn stack_transaction_roundtrip() -> Result<(), Box<dyn std::error::Error>> {
    roundtrip(Kind::Sha1)
}

#[test]
fn sha256_stack_transaction_roundtrip() -> Result<(), Box<dyn std::error::Error>> {
    roundtrip(Kind::Sha256)
}

#[test]
fn missing_table_is_reported() -> Result<(), Box<dyn std::error::Error>> {
    let temp = tempfile::tempdir()?;
    let reftable = temp.path().join("reftable");
    std::fs::create_dir(&reftable)?;
    std::fs::write(
        reftable.join("tables.list"),
        b"0x000000000001-0x000000000001-deadbeef.ref\n",
    )?;
    let error = Stack::open(&reftable, Options::default()).expect_err("missing table is rejected");
    assert_eq!(error.code(), gix_reftable::ErrorCode::NotExist);
    Ok(())
}

#[test]
fn truncated_tables_and_manifest_traversal_are_rejected() -> Result<(), Box<dyn std::error::Error>> {
    let temp = tempfile::tempdir()?;
    let reftable = temp.path().join("reftable");
    std::fs::create_dir(&reftable)?;
    let table_name = "0x000000000001-0x000000000001-deadbeef.ref";
    let mut table = vec![0; 29];
    table[..5].copy_from_slice(b"REFT\x01");
    std::fs::write(reftable.join(table_name), table)?;
    std::fs::write(reftable.join("tables.list"), format!("{table_name}\n"))?;
    let error = Stack::open(&reftable, Options::default()).expect_err("truncated table is rejected");
    assert_eq!(error.code(), gix_reftable::ErrorCode::Format);

    std::fs::write(reftable.join("tables.list"), b"../outside.ref\n")?;
    let error = Stack::open(&reftable, Options::default()).expect_err("path traversal is rejected");
    assert_eq!(error.code(), gix_reftable::ErrorCode::Format);
    Ok(())
}

#[test]
fn writer_panics_do_not_cross_the_c_boundary_or_leak_locks() -> Result<(), Box<dyn std::error::Error>> {
    let temp = tempfile::tempdir()?;
    let reftable = temp.path().join("reftable");
    std::fs::create_dir(&reftable)?;
    let stack = Stack::open(&reftable, Options::default())?;
    let mut addition = stack.into_addition()?;
    let error = addition
        .add_table(|_| -> Result<(), gix_reftable::Error> { panic!("callback panic") })
        .expect_err("callback panic becomes an error");
    assert_eq!(error.code(), gix_reftable::ErrorCode::Api);
    drop(addition);

    let stack = Stack::open(&reftable, Options::default())?;
    let addition = stack.into_addition()?;
    drop(addition);
    Ok(())
}

#[test]
fn writer_rejects_object_ids_for_the_wrong_hash() -> Result<(), Box<dyn std::error::Error>> {
    let temp = tempfile::tempdir()?;
    let reftable = temp.path().join("reftable");
    std::fs::create_dir(&reftable)?;
    let stack = Stack::open(&reftable, Options::default())?;
    let mut addition = stack.into_addition()?;
    let update_index = addition.next_update_index();
    let error = addition
        .add_table(|writer| {
            writer.set_limits(update_index, update_index);
            writer.add_ref(RefRecord {
                name: b"refs/heads/wrong-hash".as_bstr(),
                update_index,
                value: RefValue::Object(Kind::Sha256.null_ref()),
            })
        })
        .expect_err("callback panic becomes an error");
    assert_eq!(error.code(), gix_reftable::ErrorCode::Api);
    drop(addition);
    Ok(())
}

fn roundtrip(hash: Kind) -> Result<(), Box<dyn std::error::Error>> {
    let temp = tempfile::tempdir()?;
    let reftable = temp.path().join("reftable");
    std::fs::create_dir(&reftable)?;

    let options = Options {
        hash,
        lock_timeout_ms: 0,
        ..Options::default()
    };
    let stack = Stack::open(&reftable, options)?;
    let mut addition = stack.into_addition()?;
    let update_index = addition.next_update_index();
    let object = gix_hash::ObjectId::from_bytes_or_panic(&vec![1; hash.len_in_bytes()]);
    addition.add_table(|writer| {
        writer.set_limits(update_index, update_index);
        writer.add_ref(RefRecord {
            name: b"refs/heads/main".as_bstr(),
            update_index,
            value: RefValue::Object(object.as_ref()),
        })?;
        writer.add_log(LogRecord {
            name: b"refs/heads/main".as_bstr(),
            update_index,
            value: LogValue::Update {
                old: hash.null_ref(),
                new: object.as_ref(),
                name: b"A U Thor".as_bstr(),
                email: b"author@example.com".as_bstr(),
                time: 1_700_000_000,
                tz_offset: 530,
                message: b"create main".as_bstr(),
            },
        })
    })?;
    let mut stack = addition.commit()?;

    let mut refs = stack.references()?;
    refs.seek(b"refs/heads/main")?;
    assert_eq!(
        refs.next_record()?,
        Some(RefRecord {
            name: b"refs/heads/main".as_bstr(),
            update_index,
            value: RefValue::Object(object.as_ref()),
        })
    );
    drop(refs);

    let mut logs = stack.logs()?;
    logs.seek(b"refs/heads/main")?;
    assert_eq!(
        logs.next_record()?,
        Some(LogRecord {
            name: b"refs/heads/main".as_bstr(),
            update_index,
            value: LogValue::Update {
                old: hash.null_ref(),
                new: object.as_ref(),
                name: b"A U Thor".as_bstr(),
                email: b"author@example.com".as_bstr(),
                time: 1_700_000_000,
                tz_offset: 530,
                message: b"create main\n".as_bstr(),
            },
        })
    );
    drop(logs);
    assert_eq!(stack.fsck()?, 0, "new stack passes upstream checks");
    stack.compact_all()?;
    stack.clean()?;
    Ok(())
}
