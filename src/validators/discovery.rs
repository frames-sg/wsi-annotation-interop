use std::path::{Path, PathBuf};

pub(super) fn executable_available(executable: &str) -> bool {
    executable_path(executable).is_some()
}

pub(super) fn executable_path(executable: &str) -> Option<PathBuf> {
    let path = Path::new(executable);
    if path.components().count() > 1 {
        return path.is_file().then(|| path.to_path_buf());
    }
    std::env::var_os("PATH").and_then(|value| {
        std::env::split_paths(&value)
            .map(|directory| directory.join(executable))
            .find(|candidate| candidate.is_file())
    })
}
