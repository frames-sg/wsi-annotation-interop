use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use serde::Serialize;
use serde_json::Value;

use crate::compare::compare_ann;
use crate::probe::{PayloadMode, ProbeObservation, ViewerProbe};
use crate::shim::ReferenceShim;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScaleCase {
    pub coordinate_values: usize,
    pub required: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ScaleStatus {
    Passed,
    Failed,
}

impl ScaleCase {
    #[must_use]
    pub fn id(self) -> String {
        format!("ann-scale-{}", self.coordinate_values)
    }
}

#[derive(Debug, Serialize)]
pub struct ScaleObservation {
    pub case_id: String,
    pub coordinate_values: usize,
    pub required: bool,
    pub status: ScaleStatus,
    pub input_bytes: Option<u64>,
    pub output_bytes: Option<u64>,
    pub runtime_ms: Option<f64>,
    pub peak_rss_bytes: Option<u64>,
    pub viewer_runtime: BTreeMap<String, Value>,
    pub peak_tracked_heap_bytes: Option<u64>,
    pub viewer_implementation: Value,
    pub probe_commands: BTreeMap<String, Vec<String>>,
    pub coordinate_error_max_px: Option<f64>,
    pub message: String,
}

#[must_use]
pub fn default_scale_cases() -> Vec<ScaleCase> {
    vec![
        ScaleCase {
            coordinate_values: 1_000,
            required: false,
        },
        ScaleCase {
            coordinate_values: 10_000,
            required: false,
        },
        ScaleCase {
            coordinate_values: 100_000,
            required: false,
        },
        ScaleCase {
            coordinate_values: 1_000_000,
            required: true,
        },
        ScaleCase {
            coordinate_values: 5_000_000,
            required: false,
        },
    ]
}

/// Generate and roundtrip large-coordinate ANN cases with digest payloads.
///
/// # Errors
///
/// Returns an error only when the scale output directory cannot be created.
/// Each workload failure is retained in its observation.
pub fn run_scale_cases(
    reference: &ReferenceShim,
    probe: &ViewerProbe,
    source: &Path,
    directory: &Path,
    cases: &[ScaleCase],
) -> Result<Vec<ScaleObservation>, String> {
    fs::create_dir_all(directory)
        .map_err(|error| format!("could not create scale directory: {error}"))?;
    Ok(cases
        .iter()
        .copied()
        .map(|case| {
            run_scale_case(reference, probe, source, directory, case)
                .unwrap_or_else(|message| failed(directory, case, message))
        })
        .collect())
}

fn run_scale_case(
    reference: &ReferenceShim,
    probe: &ViewerProbe,
    source: &Path,
    directory: &Path,
    case: ScaleCase,
) -> Result<ScaleObservation, String> {
    let case_id = case.id();
    let input = directory.join(format!("{case_id}-input.dcm"));
    let output = directory.join(format!("{case_id}-roundtrip.dcm"));
    reference
        .build_scale_ann(source, &input, case.coordinate_values)
        .map_err(|error| error.to_string())?;
    let inspected = probe
        .inspect(source, &input, None, PayloadMode::Digest)
        .map_err(|error| error.to_string())?;
    let roundtripped = probe
        .roundtrip(source, &input, &output, None, PayloadMode::Digest, false)
        .map_err(|error| error.to_string())?;
    let comparison = compare_ann(&inspected.report, &roundtripped.report, 1e-6, 1e-9)?;
    let passed = comparison.is_ok();
    Ok(ScaleObservation {
        case_id,
        coordinate_values: case.coordinate_values,
        required: case.required,
        status: if passed {
            ScaleStatus::Passed
        } else {
            ScaleStatus::Failed
        },
        input_bytes: Some(file_size(&input)?),
        output_bytes: Some(file_size(&output)?),
        runtime_ms: Some(inspected.elapsed_ms + roundtripped.elapsed_ms),
        peak_rss_bytes: Some(inspected.peak_rss_bytes.max(roundtripped.peak_rss_bytes)),
        viewer_runtime: BTreeMap::from([
            ("inspect".to_owned(), inspected.report["runtime"].clone()),
            (
                "roundtrip".to_owned(),
                roundtripped.report["runtime"].clone(),
            ),
        ]),
        peak_tracked_heap_bytes: Some(tracked_heap(&inspected).max(tracked_heap(&roundtripped))),
        viewer_implementation: inspected.report["implementation"].clone(),
        probe_commands: BTreeMap::from([
            ("inspect".to_owned(), inspected.command),
            ("roundtrip".to_owned(), roundtripped.command),
        ]),
        coordinate_error_max_px: Some(comparison.coordinate_error.max),
        message: comparison
            .findings
            .into_iter()
            .map(|finding| finding.message)
            .collect::<Vec<_>>()
            .join("; "),
    })
}

fn tracked_heap(observation: &ProbeObservation) -> u64 {
    observation
        .report
        .pointer("/runtime/peak_tracked_heap_bytes")
        .and_then(Value::as_u64)
        .unwrap_or(0)
}

fn file_size(path: &Path) -> Result<u64, String> {
    fs::metadata(path)
        .map(|metadata| metadata.len())
        .map_err(|error| format!("could not stat {}: {error}", path.display()))
}

fn failed(directory: &Path, case: ScaleCase, message: String) -> ScaleObservation {
    let case_id = case.id();
    let input = directory.join(format!("{case_id}-input.dcm"));
    let output = directory.join(format!("{case_id}-roundtrip.dcm"));
    ScaleObservation {
        case_id,
        coordinate_values: case.coordinate_values,
        required: case.required,
        status: ScaleStatus::Failed,
        input_bytes: fs::metadata(input).ok().map(|metadata| metadata.len()),
        output_bytes: fs::metadata(output).ok().map(|metadata| metadata.len()),
        runtime_ms: None,
        peak_rss_bytes: None,
        viewer_runtime: BTreeMap::new(),
        peak_tracked_heap_bytes: None,
        viewer_implementation: Value::Object(serde_json::Map::new()),
        probe_commands: BTreeMap::new(),
        coordinate_error_max_px: None,
        message,
    }
}
