use crate::provider::VoxProvider;
use miette::IntoDiagnostic;

#[derive(Debug)]
/// A provider of the Vox build system that reads & writes from the file system.
pub struct FsProvider;
impl VoxProvider for FsProvider {
    fn read_to_string(path: impl AsRef<std::path::Path>) -> miette::Result<String> {
        std::fs::read_to_string(path).into_diagnostic()
    }
    fn write_file(
        path: impl AsRef<std::path::Path> + Clone,
        contents: impl AsRef<[u8]>,
    ) -> miette::Result<()> {
        if let Some(parent_path) = path.as_ref().parent() {
            std::fs::create_dir_all(parent_path).into_diagnostic()?;
        }
        std::fs::write(path, contents).into_diagnostic()
    }
    fn remove_file(path: impl AsRef<std::path::Path>) -> miette::Result<()> {
        std::fs::remove_file(path).into_diagnostic()
    }
    fn list_vox_files() -> miette::Result<Vec<std::path::PathBuf>> {
        Ok(glob::glob("**/*.vox")
            .into_diagnostic()?
            .filter_map(Result::ok)
            .collect())
    }
    fn list_snippets() -> miette::Result<Vec<std::path::PathBuf>> {
        Ok(glob::glob("snippets/**/*")
            .into_diagnostic()?
            .filter_map(Result::ok)
            .collect())
    }
}