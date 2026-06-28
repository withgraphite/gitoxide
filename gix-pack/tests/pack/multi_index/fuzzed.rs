use std::{
    panic::{AssertUnwindSafe, catch_unwind},
    path::PathBuf,
    sync::atomic::AtomicBool,
};

use gix_features::progress;

#[test]
fn artifact_inputs_can_be_opened_without_panicking() {
    for path in crate::fuzz_artifact_paths("multi_index_file") {
        _ = gix_pack::multi_index::File::from_data(
            std::fs::read(&path).expect("artifact is readable"),
            path,
            Some(64 * 1024 * 1024),
        );
    }
}

/// Reproducer for the fanout fuzz case: malformed multi-index files must not panic during lookup
/// if the fanout table advertises bounds that exceed the lookup chunk.
#[test]
fn malformed_fanout_is_reported_without_panicking() {
    let result = catch_unwind(AssertUnwindSafe(|| {
        match gix_pack::multi_index::File::from_data(
            malformed_multi_index_with_inconsistent_fanout(),
            PathBuf::from("fuzzed.midx"),
            None,
        ) {
            Ok(index) => {
                let _ = index.lookup(gix_hash::Kind::Sha1.null());
            }
            Err(err) => {
                let _ = err;
            }
        }
    }));

    assert!(result.is_ok(), "malformed multi-index files must not panic");
}

/// Reproducer for the allocation-limit fuzz case: long user-controlled pack names in the `PNAM`
/// chunk must be rejected as `OutOfMemory` when they exceed the configured allocation limit.
#[test]
fn long_pack_names_over_alloc_limit_bytes_are_rejected_as_out_of_memory() {
    let long_name = format!("{}.idx", "a".repeat(65));
    let index = gix_pack::multi_index::File::from_data(
        valid_multi_index_with_index_name(long_name.as_bytes()),
        PathBuf::from("fuzzed-long-name.midx"),
        None,
    )
    .expect("synthetic multi-index is valid");

    assert_eq!(
        index.index_names().cloned().collect::<Vec<_>>(),
        [PathBuf::from(&long_name)]
    );

    assert!(
        matches!(
            gix_pack::multi_index::File::from_data(
                valid_multi_index_with_index_name(long_name.as_bytes()),
                PathBuf::from("fuzzed-long-name.midx"),
                Some(64)
            ),
            Err(gix_pack::multi_index::init::Error::PackNames(
                gix_pack::multi_index::chunk::index_names::decode::Error::OutOfMemory
            ))
        ),
        "multi-index pack names larger than the configured limit must be rejected"
    );
}

/// Reproducer for the fuzz target OOM case: the harness uses an allocation cap so attacker-controlled
/// index counts must fail deterministically before reserving absurd `Vec<PathBuf>` capacities.
#[test]
fn absurd_pack_count_is_rejected_with_fuzz_alloc_limit() {
    assert!(
        matches!(
            gix_pack::multi_index::File::from_data(
                multi_index_with_absurd_pack_count(),
                PathBuf::from("fuzzed-absurd-pack-count.midx"),
                Some(64 * 1024 * 1024)
            ),
            Err(gix_pack::multi_index::init::Error::PackNames(
                gix_pack::multi_index::chunk::index_names::decode::Error::OutOfMemory
            ))
        ),
        "multi-index files advertising absurd pack counts must be rejected under the fuzz allocation cap"
    );
}

#[test]
fn verification_rejects_pack_ids_outside_their_layer() {
    let file = gix_pack::multi_index::File::from_data(
        multi_index_with_offset(1, 0, &[]),
        PathBuf::from("invalid-pack-id.midx"),
        None,
    )
    .expect("synthetic multi-index parses");
    let err = file
        .verify_integrity_fast(&mut progress::Discard, &AtomicBool::new(false))
        .expect_err("pack id is invalid");
    assert!(matches!(
        err,
        gix_pack::multi_index::verify::integrity::Error::PackIndexOutOfBounds {
            pack_index: 1,
            num_indices: 1,
            ..
        }
    ));
}

#[test]
fn verification_rejects_large_offset_rows_outside_their_chunk() {
    let file = gix_pack::multi_index::File::from_data(
        multi_index_with_offset(0, (1 << 31) | 1, &[u64::from(u32::MAX) + 1]),
        PathBuf::from("invalid-large-offset.midx"),
        None,
    )
    .expect("synthetic multi-index parses");
    let err = file
        .verify_integrity_fast(&mut progress::Discard, &AtomicBool::new(false))
        .expect_err("large offset row is invalid");
    assert!(matches!(
        err,
        gix_pack::multi_index::verify::integrity::Error::LargeOffsetOutOfBounds {
            large_offset_index: 1,
            num_large_offsets: 1,
            ..
        }
    ));
}

fn malformed_multi_index_with_inconsistent_fanout() -> Vec<u8> {
    const HEADER_LEN: usize = 12;
    const TOC_LEN: usize = 5 * 12;
    const PNAM_LEN: usize = 6;
    const FAN_LEN: usize = 256 * 4;
    const LOOKUP_LEN: usize = 20;
    const OFFSETS_LEN: usize = 8;
    const TRAILER_LEN: usize = 20;

    let pnam_start = HEADER_LEN + TOC_LEN;
    let fan_start = pnam_start + PNAM_LEN;
    let lookup_start = fan_start + FAN_LEN;
    let offsets_start = lookup_start + LOOKUP_LEN;
    let trailer_start = offsets_start + OFFSETS_LEN;

    let mut data = Vec::with_capacity(trailer_start + TRAILER_LEN);
    data.extend_from_slice(b"MIDX");
    data.push(1);
    data.push(gix_hash::Kind::Sha1 as u8);
    data.push(4);
    data.push(0);
    data.extend_from_slice(&1u32.to_be_bytes());

    push_chunk(&mut data, b"PNAM", pnam_start as u64);
    push_chunk(&mut data, b"OIDF", fan_start as u64);
    push_chunk(&mut data, b"OIDL", lookup_start as u64);
    push_chunk(&mut data, b"OOFF", offsets_start as u64);
    push_chunk(&mut data, b"\0\0\0\0", trailer_start as u64);

    data.extend_from_slice(b"a.idx\0");

    for fan_idx in 0..256 {
        let count = match fan_idx {
            0 => 2u32,
            _ => 1u32,
        };
        data.extend_from_slice(&count.to_be_bytes());
    }

    data.extend_from_slice(gix_hash::Kind::Sha1.null().as_slice());
    data.extend_from_slice(&0u32.to_be_bytes());
    data.extend_from_slice(&0u32.to_be_bytes());
    data.extend_from_slice(&[0; TRAILER_LEN]);
    debug_assert_eq!(data.len(), trailer_start + TRAILER_LEN);
    data
}

fn multi_index_with_absurd_pack_count() -> Vec<u8> {
    const HEADER_LEN: usize = 12;
    const TOC_LEN: usize = 5 * 12;
    const PNAM_LEN: usize = 1;
    const FAN_LEN: usize = 256 * 4;
    const LOOKUP_LEN: usize = 20;
    const OFFSETS_LEN: usize = 8;
    const TRAILER_LEN: usize = 20;

    let pnam_start = HEADER_LEN + TOC_LEN;
    let fan_start = pnam_start + PNAM_LEN;
    let lookup_start = fan_start + FAN_LEN;
    let offsets_start = lookup_start + LOOKUP_LEN;
    let trailer_start = offsets_start + OFFSETS_LEN;

    let mut data = Vec::with_capacity(trailer_start + TRAILER_LEN);
    data.extend_from_slice(b"MIDX");
    data.push(1);
    data.push(gix_hash::Kind::Sha1 as u8);
    data.push(4);
    data.push(0);
    data.extend_from_slice(&u32::MAX.to_be_bytes());

    push_chunk(&mut data, b"PNAM", pnam_start as u64);
    push_chunk(&mut data, b"OIDF", fan_start as u64);
    push_chunk(&mut data, b"OIDL", lookup_start as u64);
    push_chunk(&mut data, b"OOFF", offsets_start as u64);
    push_chunk(&mut data, b"\0\0\0\0", trailer_start as u64);

    data.push(0);

    for fan_idx in 0..256 {
        let count = if fan_idx == 255 { 1u32 } else { 0u32 };
        data.extend_from_slice(&count.to_be_bytes());
    }
    data.extend_from_slice(gix_hash::Kind::Sha1.null().as_slice());
    data.extend_from_slice(&0u32.to_be_bytes());
    data.extend_from_slice(&0u32.to_be_bytes());
    data.extend_from_slice(&[0; TRAILER_LEN]);
    debug_assert_eq!(data.len(), trailer_start + TRAILER_LEN);
    data
}

fn valid_multi_index_with_index_name(index_name: &[u8]) -> Vec<u8> {
    const HEADER_LEN: usize = 12;
    const TOC_LEN: usize = 5 * 12;
    const FAN_LEN: usize = 256 * 4;
    const LOOKUP_LEN: usize = 20;
    const OFFSETS_LEN: usize = 8;
    const TRAILER_LEN: usize = 20;

    let pnam_len = index_name.len() + 1;
    let pnam_start = HEADER_LEN + TOC_LEN;
    let fan_start = pnam_start + pnam_len;
    let lookup_start = fan_start + FAN_LEN;
    let offsets_start = lookup_start + LOOKUP_LEN;
    let trailer_start = offsets_start + OFFSETS_LEN;

    let mut data = Vec::with_capacity(trailer_start + TRAILER_LEN);
    data.extend_from_slice(b"MIDX");
    data.push(1);
    data.push(gix_hash::Kind::Sha1 as u8);
    data.push(4);
    data.push(0);
    data.extend_from_slice(&1u32.to_be_bytes());

    push_chunk(&mut data, b"PNAM", pnam_start as u64);
    push_chunk(&mut data, b"OIDF", fan_start as u64);
    push_chunk(&mut data, b"OIDL", lookup_start as u64);
    push_chunk(&mut data, b"OOFF", offsets_start as u64);
    push_chunk(&mut data, b"\0\0\0\0", trailer_start as u64);

    data.extend_from_slice(index_name);
    data.push(0);

    for fan_idx in 0..256 {
        let count = if fan_idx == 255 { 1u32 } else { 0u32 };
        data.extend_from_slice(&count.to_be_bytes());
    }

    data.extend_from_slice(gix_hash::Kind::Sha1.null().as_slice());
    data.extend_from_slice(&0u32.to_be_bytes());
    data.extend_from_slice(&0u32.to_be_bytes());
    data.extend_from_slice(&[0; TRAILER_LEN]);
    debug_assert_eq!(data.len(), trailer_start + TRAILER_LEN);
    data
}

fn multi_index_with_offset(pack_index: u32, offset: u32, large_offsets: &[u64]) -> Vec<u8> {
    const HEADER_LEN: usize = 12;
    const PNAM_LEN: usize = 6;
    const FAN_LEN: usize = 256 * 4;
    const LOOKUP_LEN: usize = 20;
    const OFFSETS_LEN: usize = 8;

    let num_chunks = if large_offsets.is_empty() { 4 } else { 5 };
    let toc_len = (num_chunks + 1) * 12;
    let pnam_start = HEADER_LEN + toc_len;
    let fan_start = pnam_start + PNAM_LEN;
    let lookup_start = fan_start + FAN_LEN;
    let offsets_start = lookup_start + LOOKUP_LEN;
    let large_offsets_start = offsets_start + OFFSETS_LEN;
    let trailer_start = large_offsets_start + large_offsets.len() * 8;

    let mut data = Vec::with_capacity(trailer_start + 20);
    data.extend_from_slice(b"MIDX");
    data.push(1);
    data.push(gix_hash::Kind::Sha1 as u8);
    data.push(num_chunks as u8);
    data.push(0);
    data.extend_from_slice(&1u32.to_be_bytes());

    push_chunk(&mut data, b"PNAM", pnam_start as u64);
    push_chunk(&mut data, b"OIDF", fan_start as u64);
    push_chunk(&mut data, b"OIDL", lookup_start as u64);
    push_chunk(&mut data, b"OOFF", offsets_start as u64);
    if !large_offsets.is_empty() {
        push_chunk(&mut data, b"LOFF", large_offsets_start as u64);
    }
    push_chunk(&mut data, b"\0\0\0\0", trailer_start as u64);

    data.extend_from_slice(b"a.idx\0");
    for _ in 0..256 {
        data.extend_from_slice(&1u32.to_be_bytes());
    }
    data.extend_from_slice(gix_hash::Kind::Sha1.null().as_slice());
    data.extend_from_slice(&pack_index.to_be_bytes());
    data.extend_from_slice(&offset.to_be_bytes());
    for offset in large_offsets {
        data.extend_from_slice(&offset.to_be_bytes());
    }

    let mut hasher = gix_hash::hasher(gix_hash::Kind::Sha1);
    hasher.update(&data);
    data.extend_from_slice(
        hasher
            .try_finalize()
            .expect("synthetic MIDX checksum can be computed")
            .as_slice(),
    );
    data
}

fn push_chunk(data: &mut Vec<u8>, id: &[u8; 4], offset: u64) {
    data.extend_from_slice(id);
    data.extend_from_slice(&offset.to_be_bytes());
}
