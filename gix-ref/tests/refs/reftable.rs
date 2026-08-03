use gix_object::bstr::{BString, ByteSlice};

fn store() -> crate::Result<Option<gix_ref::Store>> {
    let root = match crate::scripted_fixture_read_only("make_reftable_repo.sh") {
        Ok(root) => root,
        Err(err) if *gix_testtools::GIT_VERSION < (2, 44, 0) => {
            eprintln!("Fixture script failure ignored as it looks like Git isn't recent enough: {err}");
            return Ok(None);
        }
        Err(err) => return Err(err),
    };
    Ok(Some(gix_ref::Store::at_reftable(
        root.join("reftable-clone/.git"),
        crate::file::store_options(),
    )?))
}

fn writable_store() -> crate::Result<Option<(tempfile::TempDir, gix_ref::Store)>> {
    let root = match crate::scripted_fixture_writable("make_reftable_repo.sh") {
        Ok(root) => root,
        Err(err) if *gix_testtools::GIT_VERSION < (2, 44, 0) => {
            eprintln!("Fixture script failure ignored as it looks like Git isn't recent enough: {err}");
            return Ok(None);
        }
        Err(err) => return Err(err),
    };
    let store = gix_ref::Store::at_reftable(root.path().join("reftable-clone/.git"), crate::file::store_options())?;
    Ok(Some((root, store)))
}

#[test]
fn find_head_and_branch() -> crate::Result {
    let Some(store) = store()? else {
        return Ok(());
    };

    let head = store.find("HEAD")?;
    assert_eq!(
        head.target.try_name().expect("HEAD is symbolic").as_bstr(),
        "refs/heads/main"
    );
    let branch = store.find("main")?;
    assert!(branch.target.try_id().is_some(), "main resolves to an object id");
    Ok(())
}

#[test]
fn iter_and_reflog_reads() -> crate::Result {
    let Some(store) = store()? else {
        return Ok(());
    };

    let refs = store
        .iter()?
        .all()?
        .map(|reference| reference.map(|reference| reference.name.as_bstr().to_owned()))
        .collect::<Result<Vec<_>, _>>()?;
    assert!(
        refs.iter().all(|name| name.as_bstr() != "HEAD"),
        "all refs exclude pseudo refs: {refs:?}"
    );
    assert!(
        refs.iter().any(|name| name.as_bstr() == "refs/heads/main"),
        "all refs include the main branch: {refs:?}"
    );
    let pseudo = store
        .iter()?
        .pseudo()?
        .map(|reference| reference.map(|reference| reference.name.as_bstr().to_owned()))
        .collect::<Result<Vec<_>, _>>()?;
    assert!(
        pseudo.iter().any(|name| name.as_bstr() == "HEAD"),
        "pseudo refs include HEAD"
    );

    let head: &gix_ref::FullNameRef = "HEAD".try_into()?;
    assert!(store.reflog_exists(head)?, "HEAD reflog is readable through reftable");
    let mut log = store.reflog_iter(head);
    assert!(
        log.rev()?.is_some_and(|mut iter| iter.next().is_some()),
        "HEAD has at least one reflog entry"
    );
    Ok(())
}

#[test]
fn transaction_create_update_log_and_delete() -> crate::Result {
    use gix_ref::transaction::{Change, LogChange, PreviousValue, RefEdit, RefLog};

    let Some((fixture, store)) = writable_store()? else {
        return Ok(());
    };
    let target = store.find("main")?.target.try_id().expect("main is direct").to_owned();
    let name: gix_ref::FullName = "refs/heads/from-gix".try_into()?;
    let edit = RefEdit {
        change: Change::Update {
            log: LogChange {
                mode: RefLog::AndReference,
                force_create_reflog: true,
                message: "create from gix".into(),
            },
            expected: PreviousValue::MustNotExist,
            new: target.into(),
        },
        name: name.clone(),
        deref: false,
    };
    let committer = gix_actor::SignatureRef {
        name: "A U Thor".into(),
        email: "author@example.com".into(),
        time: "1700000000 +0530",
    };
    store
        .transaction()
        .prepare(
            [edit],
            gix_lock::acquire::Fail::Immediately,
            gix_lock::acquire::Fail::Immediately,
        )?
        .commit(Some(committer))?;

    assert_eq!(store.find(name.as_ref())?.target.try_id(), Some(target.as_ref()));
    let repo_dir = fixture.path().join("reftable-clone");
    let git_value = std::process::Command::new("git")
        .args([
            "-C",
            repo_dir.to_str().expect("UTF-8 fixture path"),
            "rev-parse",
            "refs/heads/from-gix",
        ])
        .output()?;
    assert!(git_value.status.success(), "Git reads the gix-written reference");
    assert_eq!(
        String::from_utf8(git_value.stdout)?.trim(),
        target.to_string(),
        "Git observes the same object ID"
    );
    let mut log = store.reflog_iter(name.as_ref());
    let newest = log.rev()?.expect("reflog exists").next().expect("one entry")?;
    assert_eq!(newest.signature.time.offset, 5 * 60 * 60 + 30 * 60);
    assert_eq!(newest.message, "create from gix");

    let external_name: gix_ref::FullName = "refs/heads/from-git".try_into()?;
    let target_hex = target.to_string();
    let status = std::process::Command::new("git")
        .args([
            "-C",
            repo_dir.to_str().expect("UTF-8 fixture path"),
            "update-ref",
            "refs/heads/from-git",
            &target_hex,
        ])
        .status()?;
    assert!(status.success(), "Git writes an external reftable update");
    assert_eq!(
        store.find(external_name.as_ref())?.target.try_id(),
        Some(target.as_ref()),
        "the open gix store reloads externally written tables"
    );

    store
        .transaction()
        .prepare(
            [RefEdit {
                change: Change::Delete {
                    expected: PreviousValue::MustExist,
                    log: RefLog::AndReference,
                },
                name: name.clone(),
                deref: false,
            }],
            gix_lock::acquire::Fail::Immediately,
            gix_lock::acquire::Fail::Immediately,
        )?
        .commit(Some(committer))?;
    assert!(store.try_find(name.as_ref())?.is_none(), "deleted ref is hidden");
    assert!(!store.reflog_exists(name.as_ref())?, "deletion tombstones the reflog");
    Ok(())
}

#[test]
fn transaction_constraints_conflicts_and_lock_contention() -> crate::Result {
    use gix_ref::transaction::{Change, LogChange, PreviousValue, RefEdit, RefLog};

    let Some((_fixture, store)) = writable_store()? else {
        return Ok(());
    };
    let target = store.find("main")?.target.try_id().expect("main is direct").to_owned();
    let edit = |name: &str| -> crate::Result<RefEdit> {
        Ok(RefEdit {
            change: Change::Update {
                log: LogChange {
                    mode: RefLog::AndReference,
                    force_create_reflog: false,
                    message: "update".into(),
                },
                expected: PreviousValue::Any,
                new: target.into(),
            },
            name: name.try_into()?,
            deref: false,
        })
    };

    let conflict = store
        .transaction()
        .prepare(
            [edit("refs/heads/main/child")?],
            gix_lock::acquire::Fail::Immediately,
            gix_lock::acquire::Fail::Immediately,
        )
        .err()
        .expect("a child of an existing ref is rejected");
    assert!(
        matches!(
            conflict,
            gix_ref::store::transaction::Error::Reftable(
                gix_ref::store::reftable::TransactionError::NameConflict { .. }
            )
        ),
        "reftable enforces refname D/F conflicts"
    );

    let held = store.transaction().prepare(
        [edit("refs/heads/held")?],
        gix_lock::acquire::Fail::Immediately,
        gix_lock::acquire::Fail::Immediately,
    )?;
    let contention = store
        .transaction()
        .prepare(
            [edit("refs/heads/contended")?],
            gix_lock::acquire::Fail::Immediately,
            gix_lock::acquire::Fail::Immediately,
        )
        .err()
        .expect("the first prepared transaction holds tables.list.lock");
    assert!(
        matches!(
            contention,
            gix_ref::store::transaction::Error::Reftable(
                gix_ref::store::reftable::TransactionError::Reftable(ref err)
            ) if err.code() == gix_reftable::ErrorCode::Lock
        ),
        "lock contention remains typed"
    );
    drop(held);
    Ok(())
}

#[test]
fn linked_worktree_reads_and_multi_stack_transaction() -> crate::Result {
    use gix_ref::transaction::{Change, LogChange, PreviousValue, RefEdit, RefLog};

    let fixture = match crate::scripted_fixture_writable("make_reftable_repo.sh") {
        Ok(root) => root,
        Err(err) if *gix_testtools::GIT_VERSION < (2, 44, 0) => {
            eprintln!("Fixture script failure ignored as it looks like Git isn't recent enough: {err}");
            return Ok(());
        }
        Err(err) => return Err(err),
    };
    let repository = fixture.path().join("reftable-clone");
    let worktree = fixture.path().join("linked");
    let status = std::process::Command::new("git")
        .args([
            "-C",
            repository.to_str().expect("UTF-8 fixture path"),
            "update-ref",
            "refs/bisect/main-private",
            "main",
        ])
        .status()?;
    assert!(status.success(), "Git creates a main-worktree-private ref");
    let status = std::process::Command::new("git")
        .args([
            "-C",
            repository.to_str().expect("UTF-8 fixture path"),
            "worktree",
            "add",
            "-b",
            "linked",
            worktree.to_str().expect("UTF-8 fixture path"),
            "main",
        ])
        .status()?;
    assert!(status.success(), "Git creates a linked reftable worktree");

    let mut options = crate::file::store_options();
    options.write_reflog = gix_ref::store::WriteReflog::Disable;
    let linked = gix_ref::Store::reftable_for_linked_worktree(
        repository.join(".git/worktrees/linked"),
        repository.join(".git"),
        options,
    )?;
    assert_eq!(
        linked
            .find("HEAD")?
            .target
            .try_name()
            .expect("linked HEAD is symbolic")
            .as_bstr(),
        "refs/heads/linked"
    );
    let target = linked.find("main")?.target.try_id().expect("main is direct").to_owned();
    let edit = |name: &str| -> crate::Result<RefEdit> {
        Ok(RefEdit {
            change: Change::Update {
                log: LogChange {
                    mode: RefLog::AndReference,
                    force_create_reflog: false,
                    message: BString::default(),
                },
                expected: PreviousValue::MustNotExist,
                new: target.into(),
            },
            name: name.try_into()?,
            deref: false,
        })
    };
    let mut namespaced_main = gix_ref::Store::at_reftable(repository.join(".git"), options)?;
    namespaced_main.set_namespace(gix_ref::namespace::expand("tenant")?);
    namespaced_main
        .transaction()
        .prepare(
            [edit("refs/bisect/namespaced-main-private")?],
            gix_lock::acquire::Fail::Immediately,
            gix_lock::acquire::Fail::Immediately,
        )?
        .commit(None)?;
    let mut namespaced_linked = linked.clone();
    namespaced_linked.set_namespace(gix_ref::namespace::expand("tenant")?);
    let namespaced_linked_names = namespaced_linked
        .iter()?
        .all()?
        .map(|reference| reference.map(|reference| reference.name))
        .collect::<Result<Vec<_>, _>>()?;
    assert!(
        namespaced_linked_names
            .iter()
            .all(|name| name.as_bstr() != "refs/bisect/namespaced-main-private"),
        "namespaced main-worktree-private refs do not leak into linked iteration"
    );
    linked
        .transaction()
        .prepare(
            [
                edit("refs/heads/shared-from-linked")?,
                edit("refs/bisect/private-from-linked")?,
            ],
            gix_lock::acquire::Fail::Immediately,
            gix_lock::acquire::Fail::Immediately,
        )?
        .commit(None)?;
    assert!(linked.find("refs/heads/shared-from-linked").is_ok());
    assert!(linked.find("refs/bisect/private-from-linked").is_ok());
    let linked_names = linked
        .iter()?
        .all()?
        .map(|reference| reference.map(|reference| reference.name))
        .collect::<Result<Vec<_>, _>>()?;
    assert!(
        linked_names
            .iter()
            .all(|name| name.as_bstr() != "refs/bisect/main-private"),
        "main-worktree-private refs do not leak into linked iteration"
    );

    let main = gix_ref::Store::at_reftable(repository.join(".git"), options)?;
    assert!(
        main.find("refs/heads/shared-from-linked").is_ok(),
        "shared ref is in the common stack"
    );
    assert!(
        main.try_find("refs/bisect/private-from-linked")?.is_none(),
        "worktree-private ref is absent from the main stack"
    );
    Ok(())
}

#[test]
fn object_context_writes_peeled_tag_values() -> crate::Result {
    use gix_ref::transaction::{Change, LogChange, PreviousValue, RefEdit, RefLog};

    let Some((fixture, store)) = writable_store()? else {
        return Ok(());
    };
    let repository = fixture.path().join("reftable-clone");
    let status = std::process::Command::new("git")
        .args([
            "-C",
            repository.to_str().expect("UTF-8 fixture path"),
            "tag",
            "-a",
            "annotated",
            "-m",
            "annotated tag",
        ])
        .status()?;
    assert!(status.success(), "Git creates an annotated tag");
    let resolve = |name: &str| -> crate::Result<gix_hash::ObjectId> {
        let output = std::process::Command::new("git")
            .args([
                "-C",
                repository.to_str().expect("UTF-8 fixture path"),
                "rev-parse",
                name,
            ])
            .output()?;
        assert!(output.status.success(), "Git resolves {name}");
        Ok(gix_hash::ObjectId::from_hex(
            String::from_utf8(output.stdout)?.trim().as_bytes(),
        )?)
    };
    let tag = resolve("refs/tags/annotated")?;
    let peeled = resolve("refs/tags/annotated^{}")?;
    let objects = crate::file::odb_at(repository.join(".git/objects"))?;
    store
        .transaction()
        .objects(&objects)
        .prepare(
            [RefEdit {
                change: Change::Update {
                    log: LogChange {
                        mode: RefLog::AndReference,
                        force_create_reflog: false,
                        message: BString::default(),
                    },
                    expected: PreviousValue::MustNotExist,
                    new: tag.into(),
                },
                name: "refs/tags/copied".try_into()?,
                deref: false,
            }],
            gix_lock::acquire::Fail::Immediately,
            gix_lock::acquire::Fail::Immediately,
        )?
        .commit(None)?;
    assert_eq!(store.find("refs/tags/copied")?.peeled, Some(peeled));
    Ok(())
}

#[test]
fn sha256_repository_reads_and_writes() -> crate::Result {
    use gix_ref::transaction::{Change, LogChange, PreviousValue, RefEdit, RefLog};

    if *gix_testtools::GIT_VERSION < (2, 44, 0) {
        return Ok(());
    }
    let temp = tempfile::tempdir()?;
    let source = temp.path().join("source");
    let repository = temp.path().join("reftable");
    let run = |args: &[&str]| -> crate::Result {
        let status = std::process::Command::new("git").args(args).status()?;
        assert!(status.success(), "git {args:?} succeeds");
        Ok(())
    };
    run(&[
        "init",
        "--object-format=sha256",
        "-b",
        "main",
        source.to_str().expect("UTF-8 fixture path"),
    ])?;
    run(&[
        "-C",
        source.to_str().expect("UTF-8 fixture path"),
        "config",
        "user.name",
        "committer",
    ])?;
    run(&[
        "-C",
        source.to_str().expect("UTF-8 fixture path"),
        "config",
        "user.email",
        "committer@example.com",
    ])?;
    std::fs::write(source.join("file"), b"sha256\n")?;
    run(&["-C", source.to_str().expect("UTF-8 fixture path"), "add", "file"])?;
    run(&[
        "-C",
        source.to_str().expect("UTF-8 fixture path"),
        "commit",
        "-m",
        "initial",
    ])?;
    run(&[
        "clone",
        "--ref-format=reftable",
        source.to_str().expect("UTF-8 fixture path"),
        repository.to_str().expect("UTF-8 fixture path"),
    ])?;

    let mut options = crate::file::store_options();
    options.object_hash = gix_hash::Kind::Sha256;
    options.write_reflog = gix_ref::store::WriteReflog::Disable;
    let store = gix_ref::Store::at_reftable(repository.join(".git"), options)?;
    let target = store.find("main")?.target.try_id().expect("main is direct").to_owned();
    assert_eq!(target.kind(), gix_hash::Kind::Sha256);
    store
        .transaction()
        .prepare(
            [RefEdit {
                change: Change::Update {
                    log: LogChange {
                        mode: RefLog::AndReference,
                        force_create_reflog: false,
                        message: BString::default(),
                    },
                    expected: PreviousValue::MustNotExist,
                    new: target.into(),
                },
                name: "refs/heads/sha256-from-gix".try_into()?,
                deref: false,
            }],
            gix_lock::acquire::Fail::Immediately,
            gix_lock::acquire::Fail::Immediately,
        )?
        .commit(None)?;
    assert_eq!(
        store.find("refs/heads/sha256-from-gix")?.target.try_id(),
        Some(target.as_ref())
    );
    Ok(())
}

#[test]
fn symbolic_dereference_updates_referent_and_head_log() -> crate::Result {
    use gix_ref::transaction::{Change, LogChange, PreviousValue, RefEdit, RefLog};

    let Some((fixture, store)) = writable_store()? else {
        return Ok(());
    };
    let repository = fixture.path().join("reftable-clone");
    let output = std::process::Command::new("git")
        .args([
            "-C",
            repository.to_str().expect("UTF-8 fixture path"),
            "rev-parse",
            "main^",
        ])
        .output()?;
    assert!(output.status.success(), "fixture has a parent commit");
    let previous_commit = gix_hash::ObjectId::from_hex(String::from_utf8(output.stdout)?.trim().as_bytes())?;
    let old_main = store.find("main")?.target.try_id().expect("main is direct").to_owned();
    let committer = gix_actor::SignatureRef {
        name: "A U Thor".into(),
        email: "author@example.com".into(),
        time: "1700000001 -0230",
    };
    let edits = store
        .transaction()
        .prepare(
            [RefEdit {
                change: Change::Update {
                    log: LogChange {
                        mode: RefLog::AndReference,
                        force_create_reflog: true,
                        message: "move through HEAD".into(),
                    },
                    expected: PreviousValue::Any,
                    new: previous_commit.into(),
                },
                name: "HEAD".try_into()?,
                deref: true,
            }],
            gix_lock::acquire::Fail::Immediately,
            gix_lock::acquire::Fail::Immediately,
        )?
        .commit(Some(committer))?;
    assert_eq!(edits.len(), 2, "symbolic update is split into HEAD and its referent");
    assert_eq!(
        store.find("main")?.target.try_id(),
        Some(previous_commit.as_ref()),
        "the branch referent is updated"
    );
    assert_eq!(
        store
            .find("HEAD")?
            .target
            .try_name()
            .expect("HEAD remains symbolic")
            .as_bstr(),
        "refs/heads/main"
    );
    let head: &gix_ref::FullNameRef = "HEAD".try_into()?;
    let newest = store
        .reflog_iter(head)
        .rev()?
        .expect("HEAD log exists")
        .next()
        .expect("HEAD log has an entry")?;
    assert_eq!(newest.previous_oid, old_main);
    assert_eq!(newest.new_oid, previous_commit);
    assert_eq!(newest.signature.time.offset, -(2 * 60 * 60 + 30 * 60));
    Ok(())
}

#[test]
fn direct_head_referent_update_also_updates_head_reflog() -> crate::Result {
    use gix_ref::transaction::{Change, LogChange, PreviousValue, RefEdit, RefLog};

    let Some((fixture, store)) = writable_store()? else {
        return Ok(());
    };
    let repository = fixture.path().join("reftable-clone");
    let output = std::process::Command::new("git")
        .args([
            "-C",
            repository.to_str().expect("UTF-8 fixture path"),
            "rev-parse",
            "main^",
        ])
        .output()?;
    let previous_commit = gix_hash::ObjectId::from_hex(String::from_utf8(output.stdout)?.trim().as_bytes())?;
    let old_main = store.find("main")?.target.try_id().expect("main is direct").to_owned();
    let committer = gix_actor::SignatureRef {
        name: "A U Thor".into(),
        email: "author@example.com".into(),
        time: "1700000002 +0000",
    };
    let edits = store
        .transaction()
        .prepare(
            [RefEdit {
                change: Change::Update {
                    log: LogChange {
                        mode: RefLog::AndReference,
                        force_create_reflog: true,
                        message: "move branch directly".into(),
                    },
                    expected: PreviousValue::MustExistAndMatch(old_main.into()),
                    new: previous_commit.into(),
                },
                name: "refs/heads/main".try_into()?,
                deref: false,
            }],
            gix_lock::acquire::Fail::Immediately,
            gix_lock::acquire::Fail::Immediately,
        )?
        .commit(Some(committer))?;
    assert_eq!(edits.len(), 2, "an implicit HEAD log-only update is added");
    let head: &gix_ref::FullNameRef = "HEAD".try_into()?;
    let newest = store
        .reflog_iter(head)
        .rev()?
        .expect("HEAD log exists")
        .next()
        .expect("HEAD log has an entry")?;
    assert_eq!(newest.previous_oid, old_main);
    assert_eq!(newest.new_oid, previous_commit);

    store
        .transaction()
        .prepare(
            [RefEdit {
                change: Change::Delete {
                    expected: PreviousValue::MustExistAndMatch(previous_commit.into()),
                    log: RefLog::AndReference,
                },
                name: "main-worktree/refs/heads/main".try_into()?,
                deref: false,
            }],
            gix_lock::acquire::Fail::Immediately,
            gix_lock::acquire::Fail::Immediately,
        )?
        .commit(Some(committer))?;
    let newest = store
        .reflog_iter(head)
        .rev()?
        .expect("HEAD log exists")
        .next()
        .expect("HEAD log has a deletion entry")?;
    assert_eq!(newest.previous_oid, previous_commit);
    assert_eq!(newest.new_oid, previous_commit.kind().null());
    Ok(())
}

#[test]
fn namespace_is_transparent_for_reads_and_writes() -> crate::Result {
    use gix_ref::transaction::{Change, LogChange, PreviousValue, RefEdit, RefLog};

    let Some((_fixture, mut store)) = writable_store()? else {
        return Ok(());
    };
    store.set_namespace(gix_ref::namespace::expand("tenant")?);
    store.set_write_reflog(gix_ref::store::WriteReflog::Disable);
    assert!(
        store.try_find("refs/heads/main")?.is_none(),
        "the unnamespaced main ref is hidden"
    );

    let target = crate::hex_to_id("1111111111111111111111111111111111111111");
    store
        .transaction()
        .prepare(
            [RefEdit {
                change: Change::Update {
                    log: LogChange {
                        mode: RefLog::AndReference,
                        force_create_reflog: false,
                        message: BString::default(),
                    },
                    expected: PreviousValue::MustNotExist,
                    new: target.into(),
                },
                name: "refs/heads/namespaced".try_into()?,
                deref: false,
            }],
            gix_lock::acquire::Fail::Immediately,
            gix_lock::acquire::Fail::Immediately,
        )?
        .commit(None)?;
    assert_eq!(
        store.find("refs/heads/namespaced")?.target.try_id(),
        Some(target.as_ref())
    );
    let names = store
        .iter()?
        .all()?
        .map(|reference| reference.map(|reference| reference.name))
        .collect::<Result<Vec<_>, _>>()?;
    assert_eq!(names, ["refs/heads/namespaced".try_into()?]);
    let pseudo = store
        .iter()?
        .pseudo()?
        .map(|reference| reference.map(|reference| reference.name))
        .collect::<Result<Vec<_>, _>>()?;
    assert!(pseudo.iter().any(|name| name.as_bstr() == "HEAD"));
    Ok(())
}

#[test]
fn worktree_aliases_cannot_bypass_namespaces_or_duplicate_physical_keys() -> crate::Result {
    use gix_ref::transaction::{Change, LogChange, PreviousValue, RefEdit, RefLog};

    let Some((_fixture, mut store)) = writable_store()? else {
        return Ok(());
    };
    store.set_namespace(gix_ref::namespace::expand("tenant")?);
    store.set_write_reflog(gix_ref::store::WriteReflog::Disable);
    let target = crate::hex_to_id("2222222222222222222222222222222222222222");
    let edit = |name: &str| -> crate::Result<RefEdit> {
        Ok(RefEdit {
            change: Change::Update {
                log: LogChange {
                    mode: RefLog::AndReference,
                    force_create_reflog: false,
                    message: BString::default(),
                },
                expected: PreviousValue::Any,
                new: target.into(),
            },
            name: name.try_into()?,
            deref: false,
        })
    };
    store
        .transaction()
        .prepare(
            [edit("main-worktree/refs/heads/alias")?],
            gix_lock::acquire::Fail::Immediately,
            gix_lock::acquire::Fail::Immediately,
        )?
        .commit(None)?;
    assert_eq!(store.find("refs/heads/alias")?.target.try_id(), Some(target.as_ref()));
    assert_eq!(
        store.find("main-worktree/refs/heads/alias")?.target.try_id(),
        Some(target.as_ref())
    );

    let duplicate = store
        .transaction()
        .prepare(
            [edit("refs/heads/alias")?, edit("main-worktree/refs/heads/alias")?],
            gix_lock::acquire::Fail::Immediately,
            gix_lock::acquire::Fail::Immediately,
        )
        .err()
        .expect("aliases of the same physical key are rejected");
    assert!(matches!(
        duplicate,
        gix_ref::store::transaction::Error::Reftable(
            gix_ref::store::reftable::TransactionError::DuplicateRoutedName { .. }
        )
    ));

    store.take_namespace();
    assert!(
        store.try_find("refs/heads/alias")?.is_none(),
        "the alias was stored only inside the namespace"
    );
    Ok(())
}

#[test]
fn reflog_existence_markers_are_not_exposed_as_entries() -> crate::Result {
    use gix_ref::transaction::{Change, PreviousValue, RefEdit, RefLog};

    let Some((fixture, mut store)) = writable_store()? else {
        return Ok(());
    };
    let repository = fixture.path().join("reftable-clone");
    let hash = crate::fixture_hash_kind();
    let stack = gix_reftable::Stack::open(
        &repository.join(".git/reftable"),
        gix_reftable::Options {
            hash,
            lock_timeout_ms: 0,
            ..gix_reftable::Options::default()
        },
    )?;
    let mut addition = stack.into_addition()?;
    let update_index = addition.next_update_index();
    addition.add_table(|writer| {
        writer.set_limits(update_index, update_index);
        writer.add_log(gix_reftable::LogRecord {
            name: b"refs/heads/existing-only".as_bstr(),
            update_index,
            value: gix_reftable::LogValue::Update {
                old: hash.null_ref(),
                new: hash.null_ref(),
                name: b"".as_bstr(),
                email: b"".as_bstr(),
                time: 0,
                tz_offset: 0,
                message: b"".as_bstr(),
            },
        })?;
        writer.add_log(gix_reftable::LogRecord {
            name: b"refs/heads/marker-only".as_bstr(),
            update_index,
            value: gix_reftable::LogValue::Update {
                old: hash.null_ref(),
                new: hash.null_ref(),
                name: b"".as_bstr(),
                email: b"".as_bstr(),
                time: 0,
                tz_offset: 0,
                message: b"".as_bstr(),
            },
        })
    })?;
    drop(addition.commit()?);
    let name: &gix_ref::FullNameRef = "refs/heads/marker-only".try_into()?;
    assert!(store.reflog_exists(name)?, "the raw marker denotes reflog existence");
    assert!(
        store.reflog_iter(name).rev()?.is_none(),
        "the null-to-null marker is hidden from callers"
    );
    store.set_write_reflog(gix_ref::store::WriteReflog::Existing);
    let existing_only: gix_ref::FullName = "refs/heads/existing-only".try_into()?;
    let target = store.find("main")?.target.try_id().expect("main is direct").to_owned();
    let committer = gix_actor::SignatureRef {
        name: "A U Thor".into(),
        email: "author@example.com".into(),
        time: "1700000000 +0000",
    };
    store
        .transaction()
        .prepare(
            [RefEdit {
                change: Change::Update {
                    log: gix_ref::transaction::LogChange {
                        mode: RefLog::AndReference,
                        force_create_reflog: false,
                        message: "existing only".into(),
                    },
                    expected: PreviousValue::MustNotExist,
                    new: target.into(),
                },
                name: existing_only.clone(),
                deref: false,
            }],
            gix_lock::acquire::Fail::Immediately,
            gix_lock::acquire::Fail::Immediately,
        )?
        .commit(Some(committer))?;
    assert!(
        store.reflog_iter(existing_only.as_ref()).rev()?.is_some(),
        "Existing mode updates a marker-only reflog"
    );
    store
        .transaction()
        .prepare(
            [RefEdit {
                change: Change::Delete {
                    expected: PreviousValue::Any,
                    log: RefLog::AndReference,
                },
                name: name.to_owned(),
                deref: false,
            }],
            gix_lock::acquire::Fail::Immediately,
            gix_lock::acquire::Fail::Immediately,
        )?
        .commit(None)?;
    assert!(
        !store.reflog_exists(name)?,
        "deleting a missing ref also removes its orphan reflog"
    );
    Ok(())
}

#[test]
fn reflog_messages_are_validated_and_truncated_before_publish() -> crate::Result {
    use gix_ref::transaction::{Change, LogChange, PreviousValue, RefEdit, RefLog};

    let Some((_fixture, store)) = writable_store()? else {
        return Ok(());
    };
    let target = store.find("main")?.target.try_id().expect("main is direct").to_owned();
    let committer = gix_actor::SignatureRef {
        name: "A U Thor".into(),
        email: "author@example.com".into(),
        time: "1700000000 +0000",
    };
    let edit = |name: &str, message: BString| -> crate::Result<RefEdit> {
        Ok(RefEdit {
            change: Change::Update {
                log: LogChange {
                    mode: RefLog::AndReference,
                    force_create_reflog: true,
                    message,
                },
                expected: PreviousValue::MustNotExist,
                new: target.into(),
            },
            name: name.try_into()?,
            deref: false,
        })
    };
    let long_message = BString::from(vec![b'x'; 3000]);
    let long_name: gix_ref::FullName = "refs/heads/long-log".try_into()?;
    store
        .transaction()
        .prepare(
            [edit("refs/heads/long-log", long_message)?],
            gix_lock::acquire::Fail::Immediately,
            gix_lock::acquire::Fail::Immediately,
        )?
        .commit(Some(committer))?;
    let line = store
        .reflog_iter(long_name.as_ref())
        .rev()?
        .expect("log exists")
        .next()
        .expect("one line")?;
    assert_eq!(
        line.message.len(),
        2048,
        "default block size limits the message to half a block"
    );

    let invalid = store
        .transaction()
        .prepare(
            [edit("refs/heads/invalid-log", "line one\nline two".into())?],
            gix_lock::acquire::Fail::Immediately,
            gix_lock::acquire::Fail::Immediately,
        )?
        .commit(Some(committer))
        .expect_err("multiline messages are rejected");
    assert!(matches!(
        invalid,
        gix_ref::store::transaction::Error::Reftable(gix_ref::store::reftable::TransactionError::InvalidLogMessage)
    ));
    assert!(
        store.try_find("refs/heads/invalid-log")?.is_none(),
        "failed staging publishes nothing"
    );
    Ok(())
}

mod iter_from {
    use gix_object::bstr::{BString, ByteSlice};
    use gix_ref::transaction::{Change, LogChange, PreviousValue, RefEdit, RefLog};

    use super::writable_store;

    fn create(name: &str, target: gix_hash::ObjectId) -> crate::Result<RefEdit> {
        Ok(RefEdit {
            change: Change::Update {
                log: LogChange {
                    mode: RefLog::AndReference,
                    force_create_reflog: false,
                    message: BString::default(),
                },
                expected: PreviousValue::MustNotExist,
                new: target.into(),
            },
            name: name.try_into()?,
            deref: false,
        })
    }

    /// Return a store with a few extra branches and tags, one of which was deleted again so the
    /// stack may contain a tombstone, along with all listable ref names in order.
    fn prepared_store() -> crate::Result<Option<(tempfile::TempDir, gix_ref::Store, Vec<BString>)>> {
        let Some((fixture, mut store)) = writable_store()? else {
            return Ok(None);
        };
        store.set_write_reflog(gix_ref::store::WriteReflog::Disable);
        let target = store.find("main")?.target.try_id().expect("main is direct").to_owned();
        store
            .transaction()
            .prepare(
                [
                    create("refs/heads/feature/a", target)?,
                    create("refs/heads/feature/b", target)?,
                    create("refs/heads/gone", target)?,
                    create("refs/heads/zulu", target)?,
                    create("refs/tags/v1", target)?,
                    create("refs/tags/v2", target)?,
                ],
                gix_lock::acquire::Fail::Immediately,
                gix_lock::acquire::Fail::Immediately,
            )?
            .commit(None)?;
        store
            .transaction()
            .prepare(
                [RefEdit {
                    change: Change::Delete {
                        expected: PreviousValue::MustExistAndMatch(target.into()),
                        log: RefLog::AndReference,
                    },
                    name: "refs/heads/gone".try_into()?,
                    deref: false,
                }],
                gix_lock::acquire::Fail::Immediately,
                gix_lock::acquire::Fail::Immediately,
            )?
            .commit(None)?;

        let all = store
            .iter()?
            .all()?
            .map(|reference| reference.map(|reference| reference.name.into_inner()))
            .collect::<Result<Vec<_>, _>>()?;
        assert!(
            all.iter().all(|name| name != "refs/heads/gone"),
            "the deleted ref is not listable: {all:?}"
        );
        assert!(
            all.windows(2).all(|pair| pair[0] < pair[1]),
            "iteration is sorted by name without duplicates: {all:?}"
        );
        Ok(Some((fixture, store, all)))
    }

    #[test]
    fn is_equivalent_to_skipping_names_before_from() -> crate::Result {
        let Some((_fixture, store, all)) = prepared_store()? else {
            return Ok(());
        };

        for (idx, name) in all.iter().enumerate() {
            let resumed = store
                .iter()?
                .all_from(name.as_bstr())?
                .map(|reference| reference.map(|reference| reference.name.into_inner()))
                .collect::<Result<Vec<_>, _>>()?;
            assert_eq!(
                resumed,
                all[idx..],
                "all_from({name}) yields the given name and everything after it"
            );
        }
        Ok(())
    }

    #[test]
    fn from_skips_deleted_names_and_out_of_range_bounds() -> crate::Result {
        let Some((_fixture, store, all)) = prepared_store()? else {
            return Ok(());
        };

        let resumed = store
            .iter()?
            .all_from(b"refs/heads/gone".as_bstr())?
            .map(|reference| reference.map(|reference| reference.name.into_inner()))
            .collect::<Result<Vec<_>, _>>()?;
        let expected: Vec<_> = all
            .iter()
            .filter(|name| name.as_bstr() >= b"refs/heads/gone".as_bstr())
            .cloned()
            .collect();
        assert_eq!(
            resumed, expected,
            "resuming at a deleted name starts at the next listable one"
        );

        let resumed = store
            .iter()?
            .all_from(b"refs/a".as_bstr())?
            .map(|reference| reference.map(|reference| reference.name.into_inner()))
            .collect::<Result<Vec<_>, _>>()?;
        assert_eq!(resumed, all, "a lower bound before all names yields everything");

        assert_eq!(
            store.iter()?.all_from(b"refs/tags/zzz".as_bstr())?.count(),
            0,
            "a lower bound after all names yields nothing"
        );
        Ok(())
    }

    #[test]
    fn combines_with_prefix() -> crate::Result {
        let Some((_fixture, store, _all)) = prepared_store()? else {
            return Ok(());
        };

        let resumed = store
            .iter()?
            .prefixed_from(b"refs/heads/".try_into()?, b"refs/heads/feature/b".as_bstr())?
            .map(|reference| reference.map(|reference| reference.name.into_inner()))
            .collect::<Result<Vec<_>, _>>()?;
        assert_eq!(
            resumed,
            ["refs/heads/feature/b", "refs/heads/main", "refs/heads/zulu"],
            "iteration starts at `from` and remains limited to the prefix"
        );

        let resumed = store
            .iter()?
            .prefixed_from(b"refs/tags/".try_into()?, b"refs/heads/".as_bstr())?
            .map(|reference| reference.map(|reference| reference.name.into_inner()))
            .collect::<Result<Vec<_>, _>>()?;
        assert_eq!(
            resumed,
            ["refs/tags/v1", "refs/tags/v2"],
            "a lower bound before the prefix is clamped to the prefix"
        );

        assert_eq!(
            store
                .iter()?
                .prefixed_from(b"refs/heads/".try_into()?, b"refs/tags/".as_bstr())?
                .count(),
            0,
            "a lower bound after the prefixed range yields nothing"
        );
        Ok(())
    }

    #[test]
    fn with_namespace() -> crate::Result {
        let Some((_fixture, mut store, _all)) = prepared_store()? else {
            return Ok(());
        };
        store.set_namespace(gix_ref::namespace::expand("tenant")?);
        let target = crate::hex_to_id("1111111111111111111111111111111111111111");
        store
            .transaction()
            .prepare(
                [
                    create("refs/heads/ns-a", target)?,
                    create("refs/heads/ns-b", target)?,
                    create("refs/tags/ns-tag", target)?,
                ],
                gix_lock::acquire::Fail::Immediately,
                gix_lock::acquire::Fail::Immediately,
            )?
            .commit(None)?;

        let all = store
            .iter()?
            .all()?
            .map(|reference| reference.map(|reference| reference.name.into_inner()))
            .collect::<Result<Vec<_>, _>>()?;
        assert_eq!(
            all,
            ["refs/heads/ns-a", "refs/heads/ns-b", "refs/tags/ns-tag"],
            "only namespaced refs are visible, without their namespace"
        );
        for (idx, name) in all.iter().enumerate() {
            let resumed = store
                .iter()?
                .all_from(name.as_bstr())?
                .map(|reference| reference.map(|reference| reference.name.into_inner()))
                .collect::<Result<Vec<_>, _>>()?;
            assert_eq!(
                resumed,
                all[idx..],
                "all_from({name}) applies the namespace to the lower bound as well"
            );
        }
        Ok(())
    }

    #[test]
    fn pages_can_be_stitched_together() -> crate::Result {
        let Some((_fixture, store, all)) = prepared_store()? else {
            return Ok(());
        };

        let page_size = 2;
        let mut pages = Vec::new();
        let mut cursor: Option<BString> = None;
        loop {
            let platform = store.iter()?;
            let iter = match cursor.as_ref() {
                Some(cursor) => platform.all_from(cursor.as_bstr())?,
                None => platform.all()?,
            };
            let page: Vec<_> = iter
                .filter_map(|reference| {
                    let name = reference.expect("no errors in fixture").name.into_inner();
                    // Resume inclusively at the cursor, so drop the cursor name itself.
                    (Some(&name) != cursor.as_ref()).then_some(name)
                })
                .take(page_size)
                .collect();
            if page.is_empty() {
                break;
            }
            cursor = page.last().cloned();
            pages.push(page);
        }
        let stitched: Vec<_> = pages.iter().flatten().cloned().collect();
        assert_eq!(
            stitched, all,
            "resuming with the last returned name reconstructs the full listing"
        );
        assert!(
            pages.iter().all(|page| page.len() <= page_size),
            "pages honor their size limit"
        );
        Ok(())
    }
}

#[test]
fn linked_worktree_iteration_from_merges_stacks() -> crate::Result {
    use gix_ref::transaction::{Change, LogChange, PreviousValue, RefEdit, RefLog};

    let fixture = match crate::scripted_fixture_writable("make_reftable_repo.sh") {
        Ok(root) => root,
        Err(err) if *gix_testtools::GIT_VERSION < (2, 44, 0) => {
            eprintln!("Fixture script failure ignored as it looks like Git isn't recent enough: {err}");
            return Ok(());
        }
        Err(err) => return Err(err),
    };
    let repository = fixture.path().join("reftable-clone");
    let worktree = fixture.path().join("linked");
    let status = std::process::Command::new("git")
        .args([
            "-C",
            repository.to_str().expect("UTF-8 fixture path"),
            "worktree",
            "add",
            "-b",
            "linked",
            worktree.to_str().expect("UTF-8 fixture path"),
            "main",
        ])
        .status()?;
    assert!(status.success(), "Git creates a linked reftable worktree");

    let mut options = crate::file::store_options();
    options.write_reflog = gix_ref::store::WriteReflog::Disable;
    let linked = gix_ref::Store::reftable_for_linked_worktree(
        repository.join(".git/worktrees/linked"),
        repository.join(".git"),
        options,
    )?;
    let target = linked.find("main")?.target.try_id().expect("main is direct").to_owned();
    let edit = |name: &str| -> crate::Result<RefEdit> {
        Ok(RefEdit {
            change: Change::Update {
                log: LogChange {
                    mode: RefLog::AndReference,
                    force_create_reflog: false,
                    message: BString::default(),
                },
                expected: PreviousValue::MustNotExist,
                new: target.into(),
            },
            name: name.try_into()?,
            deref: false,
        })
    };
    linked
        .transaction()
        .prepare(
            [
                edit("refs/heads/shared-from-linked")?,
                edit("refs/bisect/private-a")?,
                edit("refs/bisect/private-z")?,
            ],
            gix_lock::acquire::Fail::Immediately,
            gix_lock::acquire::Fail::Immediately,
        )?
        .commit(None)?;

    let all = linked
        .iter()?
        .all()?
        .map(|reference| reference.map(|reference| reference.name.into_inner()))
        .collect::<Result<Vec<_>, _>>()?;
    assert!(
        all.iter().any(|name| name == "refs/bisect/private-a")
            && all.iter().any(|name| name == "refs/heads/shared-from-linked"),
        "iteration merges worktree-private and shared refs: {all:?}"
    );
    assert!(
        all.windows(2).all(|pair| pair[0] < pair[1]),
        "the merged iteration is sorted by name without duplicates: {all:?}"
    );

    for (idx, name) in all.iter().enumerate() {
        let resumed = linked
            .iter()?
            .all_from(name.as_bstr())?
            .map(|reference| reference.map(|reference| reference.name.into_inner()))
            .collect::<Result<Vec<_>, _>>()?;
        assert_eq!(
            resumed,
            all[idx..],
            "all_from({name}) resumes the merged iteration at the given name"
        );
    }
    Ok(())
}
