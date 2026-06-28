use std::{
    collections::{BTreeMap, BTreeSet},
    error::Error,
    fs,
    path::PathBuf,
};

const LOCALLY_PATCHED: &[&str] = &[
    "vendor/reftable/basics.c",
    "vendor/reftable/basics.h",
    "vendor/reftable/block.c",
    "vendor/reftable/record.c",
    "vendor/reftable/reftable-block.h",
    "vendor/reftable/reftable-writer.h",
    "vendor/reftable/stack.c",
    "vendor/reftable/table.c",
    "vendor/reftable/writer.c",
];

#[test]
fn vendored_files_match_recorded_git_blobs() -> Result<(), Box<dyn Error>> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let manifest = fs::read_to_string(root.join("UPSTREAM-PROVENANCE.tsv"))?;
    let mut recorded = BTreeSet::new();

    for line in manifest
        .lines()
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
    {
        let mut fields = line.split('\t');
        let path = fields.next().ok_or("missing provenance path")?;
        let expected_blob = fields.next().ok_or("missing provenance blob")?;
        let _command = fields.next().ok_or("missing provenance command")?;
        if fields.next().is_some() {
            return Err(format!("{path}: too many provenance fields").into());
        }
        let content = fs::read(root.join(path))?;
        let actual = gix_object::compute_hash(gix_hash::Kind::Sha1, gix_object::Kind::Blob, &content)?;
        if LOCALLY_PATCHED.contains(&path) {
            assert_ne!(
                actual.to_string(),
                expected_blob,
                "{path}: local patch unexpectedly vanished"
            );
        } else {
            assert_eq!(actual.to_string(), expected_blob, "{path}: vendored content drifted");
        }
        recorded.insert(path.to_owned());
    }

    let mut packaged = BTreeSet::new();
    for entry in fs::read_dir(root.join("vendor/reftable"))? {
        let entry = entry?;
        if entry.file_type()?.is_file() {
            packaged.insert(format!("vendor/reftable/{}", entry.file_name().to_string_lossy()));
        }
    }
    assert_eq!(packaged, recorded, "every vendored file has provenance");
    assert!(!packaged.contains("vendor/reftable/system.c"));
    assert!(!packaged.contains("vendor/reftable/reftable-system.h"));
    assert!(
        LOCALLY_PATCHED.iter().all(|path| packaged.contains(*path)),
        "all declared local patches are packaged"
    );
    Ok(())
}

#[test]
fn local_patch_names_the_exact_upstream_and_vendored_blobs() -> Result<(), Box<dyn Error>> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let manifest = fs::read_to_string(root.join("UPSTREAM-PROVENANCE.tsv"))?;
    let expected_blobs = manifest
        .lines()
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(|line| {
            let mut fields = line.split('\t');
            let path = fields.next().ok_or("missing provenance path")?;
            let blob = fields.next().ok_or("missing provenance blob")?;
            Ok((path, blob))
        })
        .collect::<Result<BTreeMap<_, _>, Box<dyn Error>>>()?;
    let patch = fs::read_to_string(root.join("LOCAL-PATCHES.patch"))?;
    let mut lines = patch.lines();
    let mut patched = BTreeSet::new();

    while let Some(line) = lines.next() {
        let Some(paths) = line.strip_prefix("diff --git a/") else {
            continue;
        };
        let (old_path, new_path) = paths.split_once(" b/").ok_or("malformed patch paths")?;
        if old_path != new_path {
            return Err(format!("{old_path}: patch changes paths").into());
        }
        let index = lines
            .next()
            .and_then(|line| line.strip_prefix("index "))
            .and_then(|line| line.split_once(' ').map(|(blobs, _mode)| blobs))
            .ok_or("missing patch index")?;
        let (old_blob, new_blob) = index.split_once("..").ok_or("malformed patch index")?;
        let expected_blob = expected_blobs
            .get(old_path)
            .ok_or_else(|| format!("{old_path}: missing provenance"))?;
        assert!(
            expected_blob.starts_with(old_blob),
            "{old_path}: patch does not start at the recorded upstream blob"
        );
        let content = fs::read(root.join(old_path))?;
        let actual = gix_object::compute_hash(gix_hash::Kind::Sha1, gix_object::Kind::Blob, &content)?;
        assert!(
            actual.to_string().starts_with(new_blob),
            "{old_path}: patch does not produce the vendored blob"
        );
        patched.insert(old_path);
    }

    assert_eq!(
        patched,
        LOCALLY_PATCHED.iter().copied().collect(),
        "the patch covers every locally modified vendored file"
    );
    Ok(())
}
