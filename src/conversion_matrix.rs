use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::{Value, json};

use crate::probe::{ProbeObservation, ViewerProbe};
use crate::results::sha256_file;
use crate::shim::{FixtureSet, ReferenceShim};

mod geojson;
mod inputs;
mod parametric_map;

#[derive(Debug, Serialize)]
pub struct ConversionObservation {
    pub matrix: String,
    pub case_id: String,
    pub target: String,
    pub status: String,
    pub highdicom_readable: bool,
    pub output_paths: Vec<PathBuf>,
    pub report: Value,
    pub normalized: Value,
    pub command: Vec<String>,
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
                .all(|observation| observation.status == "passed")
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
    record(
        &mut observations,
        &[("geojson-ann", "ann"), ("sr-direct", "sr")],
        geojson::run_direct(fixtures, reference, probe, output_directory, &inputs),
    );
    record(
        &mut observations,
        &[("geojson-seg", "seg"), ("sr-seg-reference", "sr")],
        geojson::run_seg_reference(fixtures, reference, probe, output_directory, &inputs),
    );
    record(
        &mut observations,
        &[("pm-float32", "pm")],
        parametric_map::run_single(fixtures, reference, probe, output_directory, &inputs),
    );
    record(
        &mut observations,
        &[("pm-concatenation", "pm")],
        parametric_map::run_concatenation(fixtures, reference, probe, output_directory, &inputs),
    );
    Ok(ConversionMatrixResult { observations })
}

fn record(
    observations: &mut Vec<ConversionObservation>,
    expected: &[(&str, &str)],
    result: Result<Vec<ConversionObservation>, String>,
) {
    match result {
        Ok(items) => observations.extend(items),
        Err(error) => {
            observations.extend(
                expected
                    .iter()
                    .map(|(case_id, target)| ConversionObservation {
                        matrix: matrix_name(target).to_owned(),
                        case_id: (*case_id).to_owned(),
                        target: (*target).to_owned(),
                        status: "failed".to_owned(),
                        highdicom_readable: false,
                        output_paths: Vec::new(),
                        report: json!({}),
                        normalized: Value::Null,
                        command: Vec::new(),
                        runtime_ms: 0.0,
                        peak_rss_bytes: 0,
                        peak_tracked_heap_bytes: 0,
                        message: error.clone(),
                    }),
            );
        }
    }
}

pub(super) fn passed(
    matrix: &str,
    case_id: &str,
    target: &str,
    observation: &ProbeObservation,
    output_paths: Vec<PathBuf>,
    normalized: Value,
) -> ConversionObservation {
    ConversionObservation {
        matrix: matrix.to_owned(),
        case_id: case_id.to_owned(),
        target: target.to_owned(),
        status: "passed".to_owned(),
        highdicom_readable: true,
        output_paths,
        report: observation.report.clone(),
        normalized,
        command: observation.command.clone(),
        runtime_ms: observation.elapsed_ms,
        peak_rss_bytes: observation.peak_rss_bytes,
        peak_tracked_heap_bytes: observation.report["peak_tracked_heap_bytes"]
            .as_u64()
            .unwrap_or(0),
        message: String::new(),
    }
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

fn matrix_name(target: &str) -> &'static str {
    match target {
        "ann" | "seg" => "ann-seg",
        "sr" => "sr",
        "pm" => "pm",
        _ => "conversion",
    }
}
