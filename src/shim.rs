use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fmt;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::ground_truth::build_core_ground_truth;
use crate::process::{CommandSpec, ProcessError, run};

#[derive(Debug)]
pub struct ShimError(String);

impl fmt::Display for ShimError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for ShimError {}

#[derive(Debug, Deserialize, Serialize)]
pub struct FixtureSet {
    pub source: PathBuf,
    pub pyramid_source: PathBuf,
    pub pyramid_ann: PathBuf,
    pub reordered_seg: PathBuf,
    pub pm: PathBuf,
    pub sr: PathBuf,
    pub sr_seg: PathBuf,
    #[serde(skip_deserializing)]
    pub ground_truth: PathBuf,
    pub ann: BTreeMap<String, PathBuf>,
    pub seg: BTreeMap<String, PathBuf>,
}

impl FixtureSet {
    fn validate(&self) -> Result<(), ShimError> {
        const ANN_FORMS: [&str; 4] = ["2D_FRAME", "2D_VOLUME", "3D_COMMON_Z", "3D_XYZ"];
        const SEG_KINDS: [&str; 3] = ["BINARY", "FRACTIONAL", "LABELMAP"];
        if self.ann.keys().map(String::as_str).collect::<Vec<_>>() != ANN_FORMS
            || self.seg.keys().map(String::as_str).collect::<Vec<_>>() != SEG_KINDS
        {
            return Err(ShimError(
                "reference shim returned an incomplete core fixture matrix".to_owned(),
            ));
        }
        for path in [
            self.source.as_path(),
            self.pyramid_source.as_path(),
            self.pyramid_ann.as_path(),
            self.reordered_seg.as_path(),
            self.pm.as_path(),
            self.sr.as_path(),
            self.sr_seg.as_path(),
            self.ground_truth.as_path(),
        ]
        .into_iter()
        .chain(self.ann.values().map(PathBuf::as_path))
        .chain(self.seg.values().map(PathBuf::as_path))
        {
            if !path.is_file() {
                return Err(ShimError(format!(
                    "reference shim did not create fixture {}",
                    path.display()
                )));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Deserialize)]
pub struct DicomMetadata {
    pub sop_instance_uid: String,
    pub study_instance_uid: String,
    pub series_instance_uid: String,
    pub frame_of_reference_uid: Option<String>,
    pub preserved: BTreeMap<String, Option<String>>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct QualificationResult {
    pub implementation: String,
    pub version: Option<String>,
    pub qualified: bool,
    pub primary_failure: bool,
    pub capabilities: BTreeMap<String, bool>,
    pub reasons: Vec<String>,
}

#[derive(Debug)]
pub struct ReferenceShim {
    command: CommandSpec,
    timeout: Duration,
}

impl ReferenceShim {
    /// Construct the isolated highdicom reference-process adapter.
    ///
    /// # Errors
    ///
    /// Returns an error when the command is empty or the timeout is zero.
    pub fn new(command: Vec<String>, timeout: Duration) -> Result<Self, String> {
        let command = CommandSpec::from_strings(command, "reference shim")?;
        Self::from_spec(command, timeout)
    }

    /// Construct a reference adapter without converting the program or fixed arguments to UTF-8.
    ///
    /// # Errors
    ///
    /// Returns an error for an empty program or zero timeout.
    pub fn from_program(
        program: impl Into<OsString>,
        arguments: Vec<OsString>,
        timeout: Duration,
    ) -> Result<Self, String> {
        Self::from_spec(CommandSpec::new(program.into(), arguments)?, timeout)
    }

    fn from_spec(command: CommandSpec, timeout: Duration) -> Result<Self, String> {
        if timeout.is_zero() {
            return Err("reference shim timeout must be positive".to_owned());
        }
        Ok(Self { command, timeout })
    }

    /// Generate independent highdicom core fixtures.
    ///
    /// # Errors
    ///
    /// Returns an error for subprocess failures, malformed reports, or missing files.
    pub fn generate_core(&self, output: &Path) -> Result<FixtureSet, ShimError> {
        let mut fixtures: FixtureSet = self.execute(vec![
            OsString::from("generate-core"),
            OsString::from("--output"),
            output.as_os_str().to_owned(),
        ])?;
        fixtures.ground_truth = output.join("ground-truth-v1.json");
        let mut serialized = serde_json::to_vec_pretty(&build_core_ground_truth())
            .map_err(|error| ShimError(format!("could not serialize ground truth: {error}")))?;
        serialized.push(b'\n');
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&fixtures.ground_truth)
            .map_err(|error| ShimError(format!("could not create ground truth: {error}")))?;
        file.write_all(&serialized)
            .map_err(|error| ShimError(format!("could not write ground truth: {error}")))?;
        fixtures.validate()?;
        Ok(fixtures)
    }

    /// Normalize one ANN object through the independent reference implementation.
    ///
    /// # Errors
    ///
    /// Returns an error for subprocess failures or malformed JSON.
    pub fn normalize_ann(
        &self,
        annotation: &Path,
        source: &Path,
        canonical_source: Option<&Path>,
    ) -> Result<Value, ShimError> {
        let mut arguments = vec![
            OsString::from("normalize-ann"),
            OsString::from("--source"),
            source.as_os_str().to_owned(),
            OsString::from("--annotation"),
            annotation.as_os_str().to_owned(),
        ];
        if let Some(canonical_source) = canonical_source {
            arguments.extend([
                OsString::from("--canonical-source"),
                canonical_source.as_os_str().to_owned(),
            ]);
        }
        self.execute(arguments)
    }

    /// Normalize one SEG object through the independent reference implementation.
    ///
    /// # Errors
    ///
    /// Returns an error for subprocess failures or malformed JSON.
    pub fn normalize_seg(&self, annotation: &Path, source: &Path) -> Result<Value, ShimError> {
        self.execute(vec![
            OsString::from("normalize-seg"),
            OsString::from("--source"),
            source.as_os_str().to_owned(),
            OsString::from("--annotation"),
            annotation.as_os_str().to_owned(),
        ])
    }

    /// Normalize one Parametric Map through the independent reference implementation.
    ///
    /// # Errors
    ///
    /// Returns an error for subprocess failures or malformed JSON.
    pub fn normalize_pm(&self, dicom: &Path) -> Result<Value, ShimError> {
        self.execute(vec![
            OsString::from("normalize-pm"),
            OsString::from("--dicom"),
            dicom.as_os_str().to_owned(),
        ])
    }

    /// Normalize one Comprehensive 3D SR through the independent reference implementation.
    ///
    /// # Errors
    ///
    /// Returns an error for subprocess failures or malformed JSON.
    pub fn normalize_sr(&self, dicom: &Path) -> Result<Value, ShimError> {
        self.execute(vec![
            OsString::from("normalize-sr"),
            OsString::from("--dicom"),
            dicom.as_os_str().to_owned(),
        ])
    }

    /// Normalize one WSI source through the independent reference implementation.
    ///
    /// # Errors
    ///
    /// Returns an error for subprocess failures or malformed JSON.
    pub fn normalize_wsi(&self, source: &Path) -> Result<Value, ShimError> {
        self.execute(vec![
            OsString::from("normalize-wsi"),
            OsString::from("--source"),
            source.as_os_str().to_owned(),
        ])
    }

    /// Read the identity fields that a rewrite must preserve or replace.
    ///
    /// # Errors
    ///
    /// Returns an error for subprocess failures or malformed metadata.
    pub fn metadata(&self, dicom: &Path) -> Result<DicomMetadata, ShimError> {
        self.execute(vec![
            OsString::from("metadata"),
            OsString::from("--dicom"),
            dicom.as_os_str().to_owned(),
        ])
    }

    /// Build an independent large-coordinate ANN fixture.
    ///
    /// # Errors
    ///
    /// Returns an error for subprocess failures, invalid counts, or a missing output.
    pub fn build_scale_ann(
        &self,
        source: &Path,
        output: &Path,
        coordinate_values: usize,
    ) -> Result<(), ShimError> {
        let _: Value = self.execute(vec![
            OsString::from("build-scale"),
            OsString::from("--source"),
            source.as_os_str().to_owned(),
            OsString::from("--output"),
            output.as_os_str().to_owned(),
            OsString::from("--coordinate-values"),
            OsString::from(coordinate_values.to_string()),
        ])?;
        if !output.is_file() {
            return Err(ShimError(format!(
                "reference shim did not create scale fixture {}",
                output.display()
            )));
        }
        Ok(())
    }

    /// Qualify the optional pydcm ANN/SEG API behavior.
    ///
    /// # Errors
    ///
    /// Returns an error for subprocess failures or malformed qualification output.
    pub fn qualify_pydcm(
        &self,
        source: &Path,
        ann: &Path,
        seg: &Path,
    ) -> Result<QualificationResult, ShimError> {
        self.execute(vec![
            OsString::from("qualify-pydcm"),
            OsString::from("--source"),
            source.as_os_str().to_owned(),
            OsString::from("--ann"),
            ann.as_os_str().to_owned(),
            OsString::from("--seg"),
            seg.as_os_str().to_owned(),
        ])
    }

    /// Report the isolated reference environment and package versions.
    ///
    /// # Errors
    ///
    /// Returns an error for subprocess failures or malformed JSON.
    pub fn environment(&self) -> Result<Value, ShimError> {
        self.execute(vec![OsString::from("environment")])
    }

    fn execute<T: DeserializeOwned>(&self, arguments: Vec<OsString>) -> Result<T, ShimError> {
        let mut command = self.command.clone();
        command.extend(arguments);
        let output = run(&command.process_spec(self.timeout)).map_err(shim_process_error)?;
        if output.stdout.truncated {
            return Err(ShimError(format!(
                "reference shim report exceeded the {} byte stdout limit ({} bytes observed)",
                output.stdout.bytes.len(),
                output.stdout.total_bytes
            )));
        }
        if !output.status.success() {
            let detail = serde_json::from_slice::<Value>(&output.stdout.bytes)
                .ok()
                .and_then(|value| {
                    value
                        .get("error")
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                })
                .unwrap_or_else(|| String::from_utf8_lossy(&output.stderr.bytes).into_owned());
            return Err(ShimError(format!(
                "reference shim exited with status {}: {detail}",
                output.status
            )));
        }
        serde_json::from_slice(&output.stdout.bytes)
            .map_err(|error| ShimError(format!("reference shim emitted invalid JSON: {error}")))
    }
}

fn shim_process_error(error: ProcessError) -> ShimError {
    match error {
        ProcessError::TimedOut { timeout, .. } => ShimError(format!(
            "reference shim timed out after {} seconds",
            timeout.as_secs_f64()
        )),
        error => ShimError(format!("reference shim execution failed: {error}")),
    }
}
