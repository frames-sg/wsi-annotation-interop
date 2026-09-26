use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::Value;

use crate::probe::{ProbeObservation, ViewerProbe};
use crate::results::sha256_file;
use crate::shim::{FixtureSet, ReferenceShim};

mod expected;
mod geojson;
mod inputs;
mod observation;
mod parametric_map;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum ConversionMatrixKind {
    #[serde(rename = "ann-seg")]
    AnnSeg,
    #[serde(rename = "sr")]
    Sr,
    #[serde(rename = "pm")]
    Pm,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ConversionTarget {
    Ann,
    Seg,
    Sr,
    Pm,
}

impl ConversionTarget {
    const fn matrix(self) -> ConversionMatrixKind {
        match self {
            Self::Ann | Self::Seg => ConversionMatrixKind::AnnSeg,
            Self::Sr => ConversionMatrixKind::Sr,
            Self::Pm => ConversionMatrixKind::Pm,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ConversionStatus {
    Passed,
    Failed,
}

impl ConversionStatus {
    #[must_use]
    pub const fn is_passed(self) -> bool {
        matches!(self, Self::Passed)
    }
}

#[derive(Debug, Serialize)]
pub struct ConversionObservation {
    pub matrix: ConversionMatrixKind,
    pub case_id: String,
    pub target: ConversionTarget,
    pub status: ConversionStatus,
    pub highdicom_readable: bool,
    pub output_paths: Vec<PathBuf>,
    pub report: Value,
    pub normalized: Value,
    pub command: Vec<String>,
    /// Legacy aggregate; zero when unavailable. Failed rows retain nullable measurements in `report`.
    pub runtime_ms: f64,
    pub peak_rss_bytes: u64,
    pub peak_tracked_heap_bytes: u64,
    pub message: String,
}

#[derive(Debug, Serialize)]
pub struct ConversionMatrixResult {
    pub observations: Vec<ConversionObservation>,
}

impl ConversionMatrixResult {
    #[must_use]
    pub fn is_ok(&self) -> bool {
        !self.observations.is_empty()
            && self
                .observations
                .iter()
                .all(|observation| observation.status.is_passed())
    }
}

/// Run independent `GeoJSON`, SR, and Parametric Map conversion matrices.
///
/// # Errors
///
/// Returns an error when the immutable input area cannot be prepared. Individual
/// converter or oracle failures are retained as failed observations.
pub fn run_conversion_matrices(
    fixtures: &FixtureSet,
    reference: &ReferenceShim,
    probe: &ViewerProbe,
    output_directory: &Path,
) -> Result<ConversionMatrixResult, String> {
    fs::create_dir(output_directory).map_err(|error| {
        format!(
            "could not create conversion matrix directory {}: {error}",
            output_directory.display()
        )
    })?;
    let inputs = inputs::prepare(&output_directory.join("inputs"))?;
    let mut observations = Vec::with_capacity(6);
    observations.extend(geojson::run_direct(
        fixtures,
        reference,
        probe,
        output_directory,
        &inputs,
    ));
    observations.extend(geojson::run_seg_reference(
        fixtures,
        reference,
        probe,
        output_directory,
        &inputs,
    ));
    observations.extend(parametric_map::run_single(
        fixtures,
        reference,
        probe,
        output_directory,
        &inputs,
    ));
    observations.extend(parametric_map::run_concatenation(
        fixtures,
        reference,
        probe,
        output_directory,
        &inputs,
    ));
    Ok(ConversionMatrixResult { observations })
}

pub(super) fn verify_report_outputs(
    observation: &ProbeObservation,
    target: &str,
    expected_paths: &[PathBuf],
) -> Result<(), String> {
    let outputs = observation.report["outputs"]
        .as_array()
        .ok_or_else(|| "conversion report outputs must be an array".to_owned())?
        .iter()
        .filter(|output| output["target"].as_str() == Some(target))
        .collect::<Vec<_>>();
    if outputs.len() != expected_paths.len() {
        return Err(format!(
            "conversion report listed {} {target} outputs; expected {}",
            outputs.len(),
            expected_paths.len()
        ));
    }
    for (output, path) in outputs.into_iter().zip(expected_paths) {
        if !path.is_file() {
            return Err(format!("conversion output is missing: {}", path.display()));
        }
        let canonical = path
            .canonicalize()
            .map_err(|error| format!("could not canonicalize {}: {error}", path.display()))?;
        if output["path"].as_str() != Some(canonical.to_string_lossy().as_ref()) {
            return Err(format!(
                "conversion report path does not match {}",
                canonical.display()
            ));
        }
        let bytes = fs::metadata(path)
            .map_err(|error| format!("could not stat {}: {error}", path.display()))?
            .len();
        if output["bytes"].as_u64() != Some(bytes) {
            return Err(format!(
                "conversion report byte count differs for {}",
                path.display()
            ));
        }
        let checksum = sha256_file(path).map_err(|error| error.to_string())?;
        if output["sha256"].as_str() != Some(checksum.as_str()) {
            return Err(format!(
                "conversion report checksum differs for {}",
                path.display()
            ));
        }
    }
    Ok(())
}
