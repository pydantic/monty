use std::sync::LazyLock;

use ruff_db::vendored::VendoredFileSystem;

/// The source commit of the vendored typeshed.
pub const SOURCE_COMMIT: &str = include_str!("../vendor/typeshed/source_commit.txt").trim_ascii_end();

// The file path here is hardcoded in this crate's `build.rs` script.
// Luckily this crate will fail to build if this file isn't available at build time.
static TYPESHED_ZIP_BYTES: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/zipped_typeshed.zip"));

#[must_use]
pub fn file_system() -> &'static VendoredFileSystem {
    static VENDORED_TYPESHED_STUBS: LazyLock<VendoredFileSystem> =
        LazyLock::new(|| VendoredFileSystem::new_static(TYPESHED_ZIP_BYTES).unwrap());
    &VENDORED_TYPESHED_STUBS
}

#[cfg(test)]
mod tests {
    use std::io::{self, Read};

    use super::*;

    #[test]
    fn test_commit() {
        assert_eq!(SOURCE_COMMIT.len(), 40);
    }

    #[test]
    fn typeshed_zip_created_at_build_time() {
        let mut typeshed_zip_archive = zip::ZipArchive::new(io::Cursor::new(TYPESHED_ZIP_BYTES)).unwrap();

        let mut builtins_stub = typeshed_zip_archive.by_name("stdlib/builtins.pyi").unwrap();
        assert!(builtins_stub.is_file());

        let mut builtins_source = String::new();
        builtins_stub.read_to_string(&mut builtins_source).unwrap();

        assert!(builtins_source.contains("class int:"));
    }

    #[test]
    fn typeshed_versions_file_exists() {
        let mut typeshed_zip_archive = zip::ZipArchive::new(io::Cursor::new(TYPESHED_ZIP_BYTES)).unwrap();

        let mut versions_file = typeshed_zip_archive.by_name("stdlib/VERSIONS").unwrap();
        assert!(versions_file.is_file());

        let mut versions_content = String::new();
        versions_file.read_to_string(&mut versions_content).unwrap();

        // VERSIONS file should contain module version info like "builtins: 3.0-"
        assert!(versions_content.contains("builtins:"));
    }
}
