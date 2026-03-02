use std::{env, fs, path::PathBuf};

fn main() {
    println!("cargo:rerun-if-env-changed=ZSTD_LIB_DIR");

    if let Some(lib_dir) = zstd_lib_dir() {
        println!("cargo:rustc-link-search=native={}", lib_dir.display());
    }

    println!("cargo:rustc-link-lib=static=zstd");
    tauri_build::build()
}

fn zstd_lib_dir() -> Option<PathBuf> {
    if let Ok(path) = env::var("ZSTD_LIB_DIR") {
        let lib_dir = PathBuf::from(path);
        if lib_dir.exists() {
            return Some(lib_dir);
        }
    }

    for cellar_root in ["/opt/homebrew/Cellar/zstd", "/usr/local/Cellar/zstd"] {
        let mut versions = fs::read_dir(cellar_root)
            .ok()?
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .collect::<Vec<_>>();
        versions.sort();
        versions.reverse();

        for version_dir in versions {
            let candidate = version_dir.join("lib");
            if candidate.join("libzstd.a").exists() {
                return Some(candidate);
            }
        }
    }

    None
}
