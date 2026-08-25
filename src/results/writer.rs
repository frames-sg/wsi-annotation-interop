use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use tempfile::{Builder as TempDirBuilder, TempDir};

use super::manifest::{
    collect_files, sha256_file, sync_directory, sync_tree, utc_timestamp, verify_manifest,
};
use super::provenance::{Provenance, collect_provenance};
use super::{
    RunError, summarize, write_csv_exclusive, write_figures, write_json_exclusive,
    write_jsonl_exclusive,
};

#[derive(Debug)]
pub struct RunWriter {
    root: PathBuf,
    final_path: PathBuf,
    staging: Option<TempDir>,
    run_id: String,
    metadata: Value,
    provenance: Provenance,
    observations_written: bool,
    finalized: bool,
}

impl RunWriter {
    /// Create an unpublished sibling staging directory for a never-overwritten study run.
    ///
    /// # Errors
    ///
    /// Returns an error for an unsafe identifier, an existing final run, or an I/O failure.
    pub fn new(root: &Path, run_id: &str, metadata: Value) -> Result<Self, RunError> {
        Self::new_with_probe(root, run_id, metadata, None)
    }

    /// Create an unpublished run while retaining the evaluated probe's source and binary identity.
    ///
    /// # Errors
    ///
    /// Returns an error for unsafe identifiers, provenance collection, collisions, or I/O.
    pub fn new_with_probe(
        root: &Path,
        run_id: &str,
        metadata: Value,
        annotation_probe: Option<&Path>,
    ) -> Result<Self, RunError> {
        if !safe_run_id(run_id) {
            return Err(RunError(
                "run_id must be a safe filename component".to_owned(),
            ));
        }
        fs::create_dir_all(root).map_err(|error| {
            RunError(format!(
                "could not create results root {}: {error}",
                root.display()
            ))
        })?;
        let final_path = root.join(run_id);
        if final_path.exists() {
            return Err(RunError(format!(
                "run directory {} already exists",
                final_path.display()
            )));
        }
        let staging = TempDirBuilder::new()
            .prefix(&format!(".{run_id}.incomplete-"))
            .tempdir_in(root)
            .map_err(|error| {
                RunError(format!(
                    "could not create staging directory in {}: {error}",
                    root.display()
                ))
            })?;
        let mut provenance =
            collect_provenance(Path::new(env!("CARGO_MANIFEST_DIR")), annotation_probe);
        if let Some(packages) = metadata
            .pointer("/reference/packages")
            .and_then(Value::as_object)
        {
            provenance.build.python_packages = packages
                .iter()
                .filter_map(|(name, version)| {
                    version
                        .as_str()
                        .map(|version| (name.clone(), version.to_owned()))
                })
                .collect();
            provenance.build.python_packages_unknown_reason = None;
        }
        provenance.study.profile = metadata
            .get("profile")
            .and_then(Value::as_str)
            .map(str::to_owned);
        provenance.study.dicom_edition = metadata
            .get("dicom_edition")
            .and_then(Value::as_str)
            .map(str::to_owned);
        Ok(Self {
            root: root.to_path_buf(),
            final_path,
            staging: Some(staging),
            run_id: run_id.to_owned(),
            metadata,
            provenance,
            observations_written: false,
            finalized: false,
        })
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        self.staging
            .as_ref()
            .map_or(self.final_path.as_path(), TempDir::path)
    }

    /// Write immutable observation and summary tables plus study figures into staging.
    ///
    /// # Errors
    ///
    /// Returns an error if observations were already written or any artifact fails.
    pub fn write_observations(&mut self, observations: &[Value]) -> Result<(), RunError> {
        self.ensure_open()?;
        if self.observations_written {
            return Err(RunError(
                "observations have already been written".to_owned(),
            ));
        }
        for (index, observation) in observations.iter().enumerate() {
            if !observation.is_object() {
                return Err(RunError(format!(
                    "observation at index {index} must be a JSON object"
                )));
            }
        }
        let path = self.path();
        write_jsonl_exclusive(&path.join("observations.jsonl"), observations)?;
        write_csv_exclusive(&path.join("observations.csv"), observations)?;
        let summary = summarize(observations);
        write_json_exclusive(&path.join("summary.json"), &summary)?;
        write_csv_exclusive(&path.join("summary.csv"), std::slice::from_ref(&summary))?;
        write_figures(&path.join("figures"), observations)?;
        self.observations_written = true;
        Ok(())
    }

    /// Verify and atomically publish the complete staged run with a manifest written last.
    ///
    /// # Errors
    ///
    /// Returns an error before observations exist, after finalization, or on I/O failure.
    pub fn finalize(&mut self) -> Result<PathBuf, RunError> {
        self.ensure_open()?;
        if !self.observations_written {
            return Err(RunError(
                "observations must be written before finalizing a run".to_owned(),
            ));
        }
        let staging_path = self.path().to_path_buf();
        let mut paths = Vec::new();
        collect_files(&staging_path, &mut paths)?;
        paths.sort();
        let artifacts = paths
            .into_iter()
            .filter(|path| path.file_name().and_then(|name| name.to_str()) != Some("manifest.json"))
            .map(|path| {
                let relative = path.strip_prefix(&staging_path).map_err(|error| {
                    RunError(format!("could not relativize artifact: {error}"))
                })?;
                Ok(json!({
                    "path": relative.to_string_lossy().replace('\\', "/"),
                    "bytes": fs::metadata(&path)
                        .map_err(|error| RunError(format!("could not stat {}: {error}", path.display())))?
                        .len(),
                    "sha256": sha256_file(&path)?,
                }))
            })
            .collect::<Result<Vec<_>, RunError>>()?;
        let manifest = json!({
            "schema_version": 2,
            "run_id": self.run_id,
            "created_at": utc_timestamp()?,
            "metadata": self.metadata,
            "provenance": self.provenance,
            "environment": {
                "harness": env!("CARGO_PKG_VERSION"),
                "os": std::env::consts::OS,
                "architecture": std::env::consts::ARCH,
            },
            "artifacts": artifacts,
        });
        crate::schema::validate_run_manifest(&manifest).map_err(RunError)?;
        write_json_exclusive(&staging_path.join("manifest.json"), &manifest)?;
        verify_manifest(&staging_path, &manifest)?;
        sync_tree(&staging_path)?;
        sync_directory(&staging_path)?;
        if self.final_path.exists() {
            return Err(RunError(format!(
                "run directory {} already exists",
                self.final_path.display()
            )));
        }
        let staging = self
            .staging
            .take()
            .ok_or_else(|| RunError("run staging directory is unavailable".to_owned()))?;
        fs::rename(staging.path(), &self.final_path).map_err(|error| {
            RunError(format!(
                "could not publish {} as {}: {error}",
                staging.path().display(),
                self.final_path.display()
            ))
        })?;
        drop(staging);
        sync_directory(&self.root)?;
        self.finalized = true;
        Ok(self.final_path.join("manifest.json"))
    }

    fn ensure_open(&self) -> Result<(), RunError> {
        if self.finalized {
            Err(RunError("run is already finalized".to_owned()))
        } else {
            Ok(())
        }
    }
}

fn safe_run_id(value: &str) -> bool {
    !value.is_empty()
        && value != "."
        && value != ".."
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}
