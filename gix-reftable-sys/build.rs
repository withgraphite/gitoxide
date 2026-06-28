use std::{env, path::PathBuf};

fn main() {
    let manifest_dir = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("manifest directory is set"));
    let vendor = manifest_dir.join("vendor/reftable");
    let include = manifest_dir.join("include");

    let mut build = cc::Build::new();
    build
        .include(&include)
        .include(&vendor)
        .warnings(false)
        .flag_if_supported("-std=c99")
        .define("_FILE_OFFSET_BITS", "64")
        .define("_POSIX_C_SOURCE", "200809L");

    if build.get_compiler().is_like_msvc() {
        build.define("_CRT_SECURE_NO_WARNINGS", None);
    }

    if let Some(paths) = env::var_os("DEP_Z_INCLUDE") {
        for path in paths.to_string_lossy().split(',').filter(|path| !path.is_empty()) {
            build.include(path);
        }
    }

    for source in [
        "basics.c",
        "block.c",
        "blocksource.c",
        "error.c",
        "fsck.c",
        "iter.c",
        "merged.c",
        "pq.c",
        "record.c",
        "stack.c",
        "table.c",
        "tree.c",
        "writer.c",
    ] {
        build.file(vendor.join(source));
    }
    build.file(manifest_dir.join("c/compat.c"));
    if env::var_os("CARGO_FEATURE_RUST_PLATFORM").is_none() {
        build.file(manifest_dir.join("c/platform.c"));
        if env::var_os("CARGO_CFG_TARGET_OS").is_some_and(|os| os == "windows") {
            println!("cargo:rustc-link-lib=advapi32");
        }
    }
    build.compile("gix_reftable");

    println!("cargo:rerun-if-changed=c");
    println!("cargo:rerun-if-changed=include");
    println!("cargo:rerun-if-changed=vendor/reftable");
    println!("cargo:rerun-if-changed=UPSTREAM-PROVENANCE.tsv");
    println!("cargo:rerun-if-env-changed=CARGO_FEATURE_RUST_PLATFORM");
}
