use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    sync::atomic::AtomicBool,
};

use gix_features::progress;
use gix_pack::multi_index::{File, chain};

use crate::object_hash;

/// Return true if the fixture cannot be generated as `git multi-pack-index write --incremental`
/// requires git 2.47 or newer.
///
/// Note that [`gix_testtools::should_skip_as_git_version_is_smaller_than()`] cannot be used here
/// as it never skips on CI, where some jobs regenerate all fixtures with a git older than 2.47.
fn should_skip_as_git_cannot_write_chains() -> bool {
    *gix_testtools::GIT_VERSION < (2, 47, 0)
}

fn repo_path(name: &str) -> PathBuf {
    crate::scripted_fixture_read_only("make_pack_multi_index_chain.sh")
        .expect("test fixture exists")
        .join(name)
}

fn v2_repo_path() -> PathBuf {
    gix_testtools::scripted_fixture_read_only_needs_archive("make_pack_multi_index_chain_v2.sh")
        .expect("test fixture exists")
        .join("v2-compacted")
}

fn chain_file_path(repo: &Path) -> PathBuf {
    repo.join(".git/objects/pack")
        .join(chain::DIRECTORY)
        .join(chain::CHAIN_FILE)
}

fn open_chain(repo: &Path) -> File {
    File::at_chain(chain_file_path(repo), None).expect("chain fixture opens")
}

/// All pack indices in `file.pack_dir()`, in the order in which the multi-pack index refers to them.
fn indices_referred_to_by(file: &File) -> Vec<gix_pack::index::File> {
    file.index_names()
        .map(|name| gix_pack::index::File::at(file.pack_dir().join(name), file.object_hash()).expect("index opens"))
        .collect()
}

fn all_object_ids_per_git(repo: &Path) -> BTreeSet<gix_hash::ObjectId> {
    std::fs::read_to_string(repo.join("all-objects"))
        .expect("fixture wrote object list")
        .lines()
        .map(|hex| gix_hash::ObjectId::from_hex(hex.as_bytes()).expect("valid hash"))
        .collect()
}

#[test]
fn access_and_lookup() -> crate::Result {
    if should_skip_as_git_cannot_write_chains() {
        return Ok(());
    }
    for (name, expected_layers) in [("one-layer", 1usize), ("three-layer", 3usize)] {
        let repo = repo_path(name);
        let file = open_chain(&repo);
        assert_eq!(
            File::at_path(chain_file_path(&repo), None)?.num_layers(),
            expected_layers,
            "the chain-aware opener detects the chain file"
        );

        assert_eq!(file.version(), gix_pack::multi_index::Version::V1);
        assert_eq!(
            file.path(),
            chain_file_path(&repo),
            "the chain file is the identity path"
        );
        assert_eq!(
            file.pack_dir(),
            repo.join(".git/objects/pack"),
            "packs are looked up in the pack directory, not in the chain directory"
        );
        assert_eq!(file.object_hash(), object_hash(), "the hash kind is inferred correctly");
        assert_eq!(
            file.num_layers(),
            expected_layers,
            "{name} has {expected_layers} layer(s)"
        );
        assert_eq!(
            file.num_indices() as usize,
            expected_layers,
            "the fixture stores exactly one pack per layer"
        );

        let indices = indices_referred_to_by(&file);
        assert_eq!(
            file.num_objects(),
            indices.iter().map(gix_pack::index::File::num_objects).sum::<u32>(),
            "the object count is the sum over all layers"
        );

        let chain_lines = std::fs::read_to_string(chain_file_path(&repo))?;
        assert_eq!(
            file.checksum().to_string(),
            chain_lines.lines().last().expect("at least one line"),
            "the chain is identified by the checksum of its most recent layer"
        );

        for index in 0..file.num_objects() {
            let oid = file.oid_at_index(index).to_owned();
            assert_eq!(
                file.lookup(oid),
                Some(index),
                "object ids resolve back to their global position"
            );

            let (pack_id, pack_offset) = file.pack_id_and_pack_offset_at_index(index);
            let single_index = &indices[pack_id as usize];
            let entry = single_index
                .lookup(oid)
                .expect("global pack ids point at the index that contains the object");
            assert_eq!(
                single_index.pack_offset_at_index(entry),
                pack_offset,
                "pack offsets match the offset recorded in the per-pack index"
            );

            let prefix = gix_hash::Prefix::new(&oid, file.object_hash().len_in_hex())?;
            assert_eq!(
                file.lookup_prefix(prefix, None),
                Some(Ok(index)),
                "a full-length prefix lookup is unambiguous"
            );
        }

        let mut entries_seen = 0;
        for (index, entry) in file.iter().enumerate() {
            assert_eq!(entry.oid, file.oid_at_index(index as u32));
            entries_seen += 1;
        }
        assert_eq!(
            entries_seen,
            file.num_objects(),
            "iteration yields all objects of all layers"
        );
    }
    Ok(())
}

#[test]
fn object_set_matches_per_pack_indices_and_git() -> crate::Result {
    if should_skip_as_git_cannot_write_chains() {
        return Ok(());
    }
    for name in ["one-layer", "three-layer"] {
        let repo = repo_path(name);
        let file = open_chain(&repo);

        let from_chain: BTreeSet<_> = file.iter().map(|e| e.oid).collect();
        assert_eq!(
            from_chain.len(),
            file.num_objects() as usize,
            "objects are not duplicated across layers"
        );

        let from_indices: BTreeSet<_> = indices_referred_to_by(&file)
            .iter()
            .flat_map(|index| index.iter().map(|e| e.oid))
            .collect();
        assert_eq!(
            from_chain, from_indices,
            "the chain yields the same object set as the per-pack indices"
        );
        assert_eq!(
            from_chain,
            all_object_ids_per_git(&repo),
            "the chain yields the same object set as `git cat-file --batch-all-objects`"
        );
    }
    Ok(())
}

#[test]
fn version_2_compacted_chain_preserves_pack_order() -> crate::Result {
    let repo = v2_repo_path();
    let file = open_chain(&repo);

    assert_eq!(file.version(), gix_pack::multi_index::Version::V2);
    assert_eq!(file.num_layers(), 3);
    assert_eq!(file.num_indices(), 5);

    let pack_labels: Vec<_> = file
        .index_names()
        .map(|name| {
            name.file_stem()
                .and_then(std::ffi::OsStr::to_str)
                .and_then(|name| name.strip_prefix("pack-"))
                .and_then(|name| name.split('-').next())
                .expect("fixture pack names have labels")
                .to_owned()
        })
        .collect();
    assert_eq!(
        pack_labels,
        ["D", "C", "A", "B", "E"],
        "the compacted layer keeps its non-lexicographic pack order"
    );

    let compacted_layer = std::fs::read_to_string(chain_file_path(&repo))?
        .lines()
        .nth(1)
        .map(str::to_owned)
        .expect("compacted middle layer");
    let compacted_layer =
        gix_hash::ObjectId::from_hex(compacted_layer.as_bytes()).expect("fixture contains a valid layer hash");
    let compacted_layer_path = chain_file_path(&repo)
        .parent()
        .expect("chain directory")
        .join(chain::layer_file_name(&compacted_layer));
    let mut v1_data = std::fs::read(&compacted_layer_path)?;
    v1_data[4] = gix_pack::multi_index::Version::V1 as u8;
    let err = File::from_data(v1_data, compacted_layer_path, None)
        .err()
        .expect("version 1 requires lexicographic pack order");
    assert!(matches!(
        err,
        gix_pack::multi_index::init::Error::PackNames(
            gix_pack::multi_index::chunk::index_names::decode::Error::NotOrderedAlphabetically
        )
    ));

    let indices = indices_referred_to_by(&file);
    for entry in file.iter() {
        let index = &indices[entry.pack_index as usize];
        let index_entry = index.lookup(entry.oid).expect("the selected pack contains the object");
        assert_eq!(index.pack_offset_at_index(index_entry), entry.pack_offset);
    }
    assert_eq!(
        file.iter().map(|entry| entry.oid).collect::<BTreeSet<_>>(),
        all_object_ids_per_git(&repo),
        "the compacted chain exposes the same objects as Git"
    );
    Ok(())
}

#[test]
fn lookup_prefix_across_layers() -> crate::Result {
    if should_skip_as_git_cannot_write_chains() {
        return Ok(());
    }
    let repo = repo_path("three-layer");
    let file = open_chain(&repo);

    let Some((prefix, matching_indices)) = shortest_ambiguous_prefix(&file) else {
        // The fixture is large enough for this to be near-impossible, but as its contents depend on
        // the git version, ambiguity cannot be guaranteed when fixtures are regenerated.
        eprintln!("skipping ambiguous prefix assertions as the fixture has no ambiguous object ids");
        return Ok(());
    };
    assert_eq!(
        file.lookup_prefix(prefix, None),
        Some(Err(())),
        "matches on different objects are ambiguous"
    );

    let mut candidates = Vec::new();
    assert_eq!(file.lookup_prefix(prefix, Some(&mut candidates)), Some(Err(())));
    let mut candidate_indices: Vec<_> = candidates.iter().flat_map(Clone::clone).collect();
    candidate_indices.sort_unstable();
    assert_eq!(
        candidate_indices, matching_indices,
        "candidate ranges enumerate all matching entries, possibly across layers"
    );

    let missing = gix_hash::Prefix::new(&file.object_hash().null(), 7)?;
    assert_eq!(
        file.lookup_prefix(missing, None),
        None,
        "misses are not reported as matches"
    );
    assert_eq!(
        file.lookup_prefix(missing, Some(&mut candidates)),
        None,
        "misses with candidates work as well"
    );
    assert_eq!(
        candidates,
        Vec::<std::ops::Range<u32>>::new(),
        "…and leave no candidates"
    );
    Ok(())
}

/// Find a prefix matching more than one object in `file`, with the sorted global indices of all matches.
fn shortest_ambiguous_prefix(file: &File) -> Option<(gix_hash::Prefix, Vec<u32>)> {
    let mut oids: Vec<_> = file.iter().map(|e| e.oid).collect();
    oids.sort_unstable();
    let (oid, hex_len) = oids
        .windows(2)
        .filter_map(|pair| {
            let common_hex_len = pair[0]
                .to_string()
                .chars()
                .zip(pair[1].to_string().chars())
                .take_while(|(lhs, rhs)| lhs == rhs)
                .count();
            (common_hex_len >= 4).then_some((pair[0], common_hex_len))
        })
        .next()?;
    let prefix = gix_hash::Prefix::new(&oid, hex_len).expect("hex_len in range");
    let matches: Vec<_> = (0..file.num_objects())
        .filter(|&index| prefix.cmp_oid(file.oid_at_index(index)) == std::cmp::Ordering::Equal)
        .collect();
    assert!(matches.len() > 1, "the prefix is shared by at least two objects");
    Some((prefix, matches))
}

#[test]
fn verify() -> crate::Result {
    if should_skip_as_git_cannot_write_chains() {
        return Ok(());
    }
    let repo = repo_path("three-layer");
    let file = open_chain(&repo);

    assert_eq!(
        file.verify_checksum(&mut progress::Discard, &AtomicBool::new(false))?,
        file.checksum(),
        "each layer's checksum matches its content"
    );

    let outcome = file
        .verify_integrity(
            &mut progress::Discard,
            &AtomicBool::new(false),
            gix_pack::index::verify::integrity::Options::default(),
        )
        .map_err(|err| format!("verify_integrity failed: {err}"))?;
    assert_eq!(outcome.actual_index_checksum, file.checksum());
    assert_eq!(
        outcome.pack_traverse_statistics.len(),
        file.num_indices() as usize,
        "all packs of all layers are traversed"
    );
    Ok(())
}

mod broken_chains {
    use gix_pack::multi_index::{File, chain};

    use super::{chain_file_path, should_skip_as_git_cannot_write_chains};

    fn writable_three_layer_repo() -> crate::Result<(gix_testtools::tempfile::TempDir, std::path::PathBuf)> {
        let dir = gix_testtools::scripted_fixture_writable("make_pack_multi_index_chain.sh")?;
        let repo = dir.path().join("three-layer");
        Ok((dir, repo))
    }

    #[test]
    fn missing_layer_file() -> crate::Result {
        if should_skip_as_git_cannot_write_chains() {
            return Ok(());
        }
        let (_dir, repo) = writable_three_layer_repo()?;
        let chain_path = chain_file_path(&repo);
        let first_layer = std::fs::read_to_string(&chain_path)?
            .lines()
            .next()
            .expect("at least one line")
            .to_owned();
        let layer_id = gix_hash::ObjectId::from_hex(first_layer.as_bytes())?;
        std::fs::remove_file(
            chain_path
                .parent()
                .expect("chain file within chain directory")
                .join(chain::layer_file_name(&layer_id)),
        )?;

        let err = File::at_chain(&chain_path, None)
            .err()
            .expect("a missing layer breaks the entire chain");
        assert!(
            matches!(&err, chain::Error::Layer(gix_pack::multi_index::init::Error::Io { .. })),
            "the layer file could not be opened: {err:?}"
        );
        Ok(())
    }

    #[test]
    fn layer_with_mismatching_checksum() -> crate::Result {
        if should_skip_as_git_cannot_write_chains() {
            return Ok(());
        }
        let (_dir, repo) = writable_three_layer_repo()?;
        let chain_path = chain_file_path(&repo);
        let chain_dir = chain_path.parent().expect("chain file within chain directory");
        let lines = std::fs::read_to_string(&chain_path)?;
        let mut ids = lines
            .lines()
            .map(|hex| gix_hash::ObjectId::from_hex(hex.as_bytes()).expect("valid hash"));
        let (first, second) = (ids.next().expect("first layer"), ids.next().expect("second layer"));
        // git writes layer files read-only, so they cannot be overwritten in place.
        std::fs::remove_file(chain_dir.join(chain::layer_file_name(&second)))?;
        std::fs::copy(
            chain_dir.join(chain::layer_file_name(&first)),
            chain_dir.join(chain::layer_file_name(&second)),
        )?;

        let err = File::at_chain(&chain_path, None)
            .err()
            .expect("layer content must match the chain file");
        assert!(
            matches!(&err, chain::Error::LayerChecksum { actual, expected, .. }
                if *actual == first && *expected == second),
            "the mismatching trailing checksum is detected: {err:?}"
        );
        Ok(())
    }

    #[test]
    fn invalid_chain_lines() -> crate::Result {
        if should_skip_as_git_cannot_write_chains() {
            return Ok(());
        }
        let (_dir, repo) = writable_three_layer_repo()?;
        let chain_path = chain_file_path(&repo);
        let valid_chain = std::fs::read_to_string(&chain_path)?;

        std::fs::write(&chain_path, "0123-not-a-hash\n")?;
        let err = File::at_chain(&chain_path, None).err().expect("hex parsing fails");
        assert!(
            matches!(&err, chain::Error::InvalidLine { line_number: 1, .. }),
            "lines must be valid hashes: {err:?}"
        );

        std::fs::write(&chain_path, valid_chain.replace('\n', "\r\n"))?;
        File::at_chain(&chain_path, None).expect("CRLF line endings are accepted");

        let mut lines = valid_chain.lines();
        let with_blank_line = format!(
            "{}\n\n{}\n",
            lines.next().expect("first layer"),
            lines.collect::<Vec<_>>().join("\n")
        );
        std::fs::write(&chain_path, with_blank_line)?;
        let err = File::at_chain(&chain_path, None)
            .err()
            .expect("interior blank lines are invalid");
        assert!(matches!(&err, chain::Error::InvalidLine { line_number: 2, .. }));

        std::fs::write(&chain_path, "\n")?;
        let err = File::at_chain(&chain_path, None)
            .err()
            .expect("a chain containing only a newline has no layers");
        assert!(matches!(&err, chain::Error::InvalidLine { line_number: 1, .. }));

        std::fs::write(&chain_path, "")?;
        let err = File::at_chain(&chain_path, None)
            .err()
            .expect("an empty chain file is unusable");
        assert!(
            matches!(&err, chain::Error::Empty { .. }),
            "an empty chain is reported as such, like git treats it as missing: {err:?}"
        );
        Ok(())
    }
}
