use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use serde::Serialize;
use serde_json::{Value, json};

use crate::compare::{ComparisonResult, compare_ann, compare_seg};
use crate::ground_truth::build_core_ground_truth;
use crate::metrics::{BinaryRun, segmentation_metrics};
use crate::probe::{PayloadMode, ProbeError, ProbeObservation, ViewerProbe};
use crate::shim::{DicomMetadata, FixtureSet, ReferenceShim};

const PIXEL_TOLERANCE: f64 = 1e-6;
const MILLIMETER_TOLERANCE: f64 = 1e-9;

struct Case<'a> {
    id: &'a str,
    expected: &'a Value,
    path: &'a Path,
    source: &'a Path,
}

struct MatrixServices<'a> {
    reference: &'a ReferenceShim,
    probe: &'a ViewerProbe,
    output_directory: &'a Path,
}

#[derive(Debug, Serialize)]
pub struct MatrixObservation {
    pub case_id: String,
    pub pyramid_level: u8,
    pub status: String,
    pub inspect_equal: bool,
    pub roundtrip_equal: Option<bool>,
    pub rewrite_rejected: bool,
    pub highdicom_readable: bool,
    pub coordinate_error_max_px: Option<f64>,
    pub coordinate_error_median_px: Option<f64>,
    pub coordinate_error_rms_px: Option<f64>,
    pub dice: Option<f64>,
    pub mask_metrics: Option<Value>,
    pub runtime_ms: f64,
    pub peak_rss_bytes: u64,
    pub viewer_runtime: BTreeMap<String, Value>,
    pub peak_tracked_heap_bytes: u64,
    pub viewer_implementation: Value,
    pub probe_commands: BTreeMap<String, Vec<String>>,
    pub input_bytes: u64,
    pub output_bytes: Option<u64>,
    pub input_sop_instance_uid: String,
    pub output_sop_instance_uid: Option<String>,
    pub message: String,
}

#[derive(Debug, Serialize)]
pub struct CoreMatrixResult {
    pub observations: Vec<MatrixObservation>,
}

impl CoreMatrixResult {
    #[must_use]
    pub fn is_ok(&self) -> bool {
        !self.observations.is_empty()
            && self
                .observations
                .iter()
                .all(|observation| observation.status == "passed")
    }
}

/// Run the independent-reference → viewer → independent-reference core matrix.
///
/// # Errors
///
/// Returns an error when the declarative oracle or output directory is invalid.
/// Individual case failures are retained as observations.
pub fn run_core_matrix(
    fixtures: &FixtureSet,
    reference: &ReferenceShim,
    probe: &ViewerProbe,
    output_directory: &Path,
) -> Result<CoreMatrixResult, String> {
    fs::create_dir_all(output_directory)
        .map_err(|error| format!("could not create matrix output directory: {error}"))?;
    let truth = load_ground_truth(&fixtures.ground_truth)?;
    let cases = truth
        .get("cases")
        .and_then(Value::as_object)
        .ok_or_else(|| "ground truth cases must be an object".to_owned())?;
    let services = MatrixServices {
        reference,
        probe,
        output_directory,
    };
    let mut observations = run_ann_matrix(fixtures, cases, &services)?;
    observations.extend(run_seg_matrix(fixtures, cases, &services)?);
    Ok(CoreMatrixResult { observations })
}

fn run_ann_matrix(
    fixtures: &FixtureSet,
    cases: &serde_json::Map<String, Value>,
    services: &MatrixServices<'_>,
) -> Result<Vec<MatrixObservation>, String> {
    let mut observations = Vec::with_capacity(5);
    for (form, case_id) in [
        ("2D_VOLUME", "ann-2d-volume"),
        ("2D_FRAME", "ann-2d-frame"),
        ("3D_COMMON_Z", "ann-3d-common-z"),
        ("3D_XYZ", "ann-3d-xyz"),
    ] {
        let path = fixture(&fixtures.ann, form)?;
        observations.push(case_observation(
            case_id,
            path,
            0,
            cases.get(case_id),
            || {
                run_ann(
                    &Case {
                        id: case_id,
                        expected: required_case(cases, case_id)?,
                        path,
                        source: &fixtures.source,
                    },
                    services,
                    None,
                    0,
                )
            },
        ));
    }

    let pyramid_case = "ann-2d-volume-level1";
    observations.push(case_observation(
        pyramid_case,
        &fixtures.pyramid_ann,
        1,
        cases.get(pyramid_case),
        || {
            run_ann(
                &Case {
                    id: pyramid_case,
                    expected: required_case(cases, pyramid_case)?,
                    path: &fixtures.pyramid_ann,
                    source: &fixtures.pyramid_source,
                },
                services,
                Some(&fixtures.source),
                1,
            )
        },
    ));

    Ok(observations)
}

fn run_seg_matrix(
    fixtures: &FixtureSet,
    cases: &serde_json::Map<String, Value>,
    services: &MatrixServices<'_>,
) -> Result<Vec<MatrixObservation>, String> {
    let mut observations = Vec::with_capacity(4);
    for (kind, case_id) in [
        ("BINARY", "seg-binary"),
        ("LABELMAP", "seg-labelmap"),
        ("FRACTIONAL", "seg-fractional"),
    ] {
        let path = fixture(&fixtures.seg, kind)?;
        observations.push(case_observation(
            case_id,
            path,
            0,
            cases.get(case_id),
            || {
                run_seg(
                    &Case {
                        id: case_id,
                        expected: required_case(cases, case_id)?,
                        path,
                        source: &fixtures.source,
                    },
                    services,
                    kind,
                )
            },
        ));
    }

    let reordered_case = "seg-binary-reordered";
    observations.push(case_observation(
        reordered_case,
        &fixtures.reordered_seg,
        0,
        cases.get(reordered_case),
        || {
            run_seg(
                &Case {
                    id: reordered_case,
                    expected: required_case(cases, reordered_case)?,
                    path: &fixtures.reordered_seg,
                    source: &fixtures.source,
                },
                services,
                "BINARY",
            )
        },
    ));

    Ok(observations)
}

fn load_ground_truth(path: &Path) -> Result<Value, String> {
    let data = fs::read(path)
        .map_err(|error| format!("could not read ground truth {}: {error}", path.display()))?;
    let truth: Value = serde_json::from_slice(&data)
        .map_err(|error| format!("ground truth is invalid JSON: {error}"))?;
    if truth != build_core_ground_truth() {
        return Err("generated ground truth differs from the Rust declarative oracle".to_owned());
    }
    Ok(truth)
}

fn fixture<'a>(
    fixtures: &'a BTreeMap<String, std::path::PathBuf>,
    key: &str,
) -> Result<&'a Path, String> {
    fixtures
        .get(key)
        .map(std::path::PathBuf::as_path)
        .ok_or_else(|| format!("core fixture {key} is missing"))
}

fn required_case<'a>(
    cases: &'a serde_json::Map<String, Value>,
    case_id: &str,
) -> Result<&'a Value, String> {
    cases
        .get(case_id)
        .ok_or_else(|| format!("ground truth case {case_id} is missing"))
}

fn case_observation(
    case_id: &str,
    path: &Path,
    pyramid_level: u8,
    expected: Option<&Value>,
    run: impl FnOnce() -> Result<MatrixObservation, String>,
) -> MatrixObservation {
    run().unwrap_or_else(|error| failed(case_id, path, pyramid_level, expected, error))
}

fn run_ann(
    case: &Case<'_>,
    services: &MatrixServices<'_>,
    canonical_source: Option<&Path>,
    pyramid_level: u8,
) -> Result<MatrixObservation, String> {
    let Case {
        id: case_id,
        expected,
        path,
        source,
    } = *case;
    let reference_input = compare_ann(
        expected,
        &services
            .reference
            .normalize_ann(path, source, canonical_source)
            .map_err(|error| error.to_string())?,
        PIXEL_TOLERANCE,
        MILLIMETER_TOLERANCE,
    )?;
    let inspected = services
        .probe
        .inspect(source, path, canonical_source, PayloadMode::Full)
        .map_err(|error| error.to_string())?;
    let inspect = compare_ann(
        expected,
        &inspected.report,
        PIXEL_TOLERANCE,
        MILLIMETER_TOLERANCE,
    )?;
    let output_path = services
        .output_directory
        .join(format!("{case_id}-roundtrip.dcm"));
    let roundtripped = services
        .probe
        .roundtrip(
            source,
            path,
            &output_path,
            canonical_source,
            PayloadMode::Full,
            false,
        )
        .map_err(|error| error.to_string())?;
    let viewer = compare_ann(
        expected,
        &roundtripped.report,
        PIXEL_TOLERANCE,
        MILLIMETER_TOLERANCE,
    )?;
    let reference_output = compare_ann(
        expected,
        &services
            .reference
            .normalize_ann(&output_path, source, canonical_source)
            .map_err(|error| error.to_string())?,
        PIXEL_TOLERANCE,
        MILLIMETER_TOLERANCE,
    )?;
    let input_metadata = services
        .reference
        .metadata(path)
        .map_err(|error| error.to_string())?;
    let output_metadata = services
        .reference
        .metadata(&output_path)
        .map_err(|error| error.to_string())?;
    let identity_message = rewrite_identity_message(&input_metadata, &output_metadata);
    let roundtrip_equal = viewer.is_ok() && reference_output.is_ok() && identity_message.is_none();
    let message = messages(
        [&reference_input, &inspect, &viewer, &reference_output],
        identity_message,
    );
    let errors = [&inspect, &viewer, &reference_output];

    Ok(MatrixObservation {
        case_id: case_id.to_owned(),
        pyramid_level,
        status: pass_status(reference_input.is_ok() && inspect.is_ok() && roundtrip_equal),
        inspect_equal: inspect.is_ok(),
        roundtrip_equal: Some(roundtrip_equal),
        rewrite_rejected: false,
        highdicom_readable: true,
        coordinate_error_max_px: Some(max_error(errors, |result| result.coordinate_error.max)),
        coordinate_error_median_px: Some(max_error(errors, |result| {
            result.coordinate_error.median
        })),
        coordinate_error_rms_px: Some(max_error(errors, |result| result.coordinate_error.rms)),
        dice: None,
        mask_metrics: None,
        runtime_ms: inspected.elapsed_ms + roundtripped.elapsed_ms,
        peak_rss_bytes: inspected.peak_rss_bytes.max(roundtripped.peak_rss_bytes),
        viewer_runtime: viewer_runtime(&[("inspect", &inspected), ("roundtrip", &roundtripped)]),
        peak_tracked_heap_bytes: peak_tracked_heap(&[&inspected, &roundtripped]),
        viewer_implementation: inspected.report["implementation"].clone(),
        probe_commands: probe_commands(&[("inspect", &inspected), ("roundtrip", &roundtripped)]),
        input_bytes: file_size(path)?,
        output_bytes: Some(file_size(&output_path)?),
        input_sop_instance_uid: input_metadata.sop_instance_uid,
        output_sop_instance_uid: Some(output_metadata.sop_instance_uid),
        message,
    })
}

fn run_seg(
    case: &Case<'_>,
    services: &MatrixServices<'_>,
    kind: &str,
) -> Result<MatrixObservation, String> {
    let Case {
        id: case_id,
        expected,
        path,
        source,
    } = *case;
    let reference_input = compare_seg(
        expected,
        &services
            .reference
            .normalize_seg(path, source)
            .map_err(|error| error.to_string())?,
    )?;
    let inspected = services
        .probe
        .inspect(source, path, None, PayloadMode::Full)
        .map_err(|error| error.to_string())?;
    let inspect = compare_seg(expected, &inspected.report)?;
    if kind != "BINARY" {
        return read_only_seg(case, services, &reference_input, &inspected, &inspect);
    }

    let output_path = services
        .output_directory
        .join(format!("{case_id}-roundtrip.dcm"));
    let roundtripped = services
        .probe
        .roundtrip(source, path, &output_path, None, PayloadMode::Full, false)
        .map_err(|error| error.to_string())?;
    let viewer = compare_seg(expected, &roundtripped.report)?;
    let reference_output = compare_seg(
        expected,
        &services
            .reference
            .normalize_seg(&output_path, source)
            .map_err(|error| error.to_string())?,
    )?;
    let input_metadata = services
        .reference
        .metadata(path)
        .map_err(|error| error.to_string())?;
    let output_metadata = services
        .reference
        .metadata(&output_path)
        .map_err(|error| error.to_string())?;
    let identity_message = rewrite_identity_message(&input_metadata, &output_metadata);
    let roundtrip_equal = viewer.is_ok() && reference_output.is_ok() && identity_message.is_none();
    let message = messages(
        [&reference_input, &inspect, &viewer, &reference_output],
        identity_message,
    );
    let metrics = mask_metrics(expected, &roundtripped.report)?;

    Ok(MatrixObservation {
        case_id: case_id.to_owned(),
        pyramid_level: 0,
        status: pass_status(reference_input.is_ok() && inspect.is_ok() && roundtrip_equal),
        inspect_equal: inspect.is_ok(),
        roundtrip_equal: Some(roundtrip_equal),
        rewrite_rejected: false,
        highdicom_readable: true,
        coordinate_error_max_px: None,
        coordinate_error_median_px: None,
        coordinate_error_rms_px: None,
        dice: metric_number(metrics.as_ref(), "dice"),
        mask_metrics: metrics,
        runtime_ms: inspected.elapsed_ms + roundtripped.elapsed_ms,
        peak_rss_bytes: inspected.peak_rss_bytes.max(roundtripped.peak_rss_bytes),
        viewer_runtime: viewer_runtime(&[("inspect", &inspected), ("roundtrip", &roundtripped)]),
        peak_tracked_heap_bytes: peak_tracked_heap(&[&inspected, &roundtripped]),
        viewer_implementation: inspected.report["implementation"].clone(),
        probe_commands: probe_commands(&[("inspect", &inspected), ("roundtrip", &roundtripped)]),
        input_bytes: file_size(path)?,
        output_bytes: Some(file_size(&output_path)?),
        input_sop_instance_uid: input_metadata.sop_instance_uid,
        output_sop_instance_uid: Some(output_metadata.sop_instance_uid),
        message,
    })
}

fn read_only_seg(
    case: &Case<'_>,
    services: &MatrixServices<'_>,
    reference_input: &ComparisonResult,
    inspected: &ProbeObservation,
    inspect: &ComparisonResult,
) -> Result<MatrixObservation, String> {
    let output_path = services
        .output_directory
        .join(format!("{}-roundtrip.dcm", case.id));
    let rejected = match services.probe.roundtrip(
        case.source,
        case.path,
        &output_path,
        None,
        PayloadMode::Full,
        false,
    ) {
        Err(error) => rewrite_rejected(&error, &output_path),
        Ok(_) => false,
    };
    let mut message = messages([reference_input, inspect], None);
    if !rejected {
        if !message.is_empty() {
            message.push_str("; ");
        }
        message.push_str("read-only SEG rewrite was not safely rejected");
    }
    let metrics = mask_metrics(case.expected, &inspected.report)?;
    Ok(MatrixObservation {
        case_id: case.id.to_owned(),
        pyramid_level: 0,
        status: pass_status(reference_input.is_ok() && inspect.is_ok() && rejected),
        inspect_equal: inspect.is_ok(),
        roundtrip_equal: None,
        rewrite_rejected: rejected,
        highdicom_readable: true,
        coordinate_error_max_px: None,
        coordinate_error_median_px: None,
        coordinate_error_rms_px: None,
        dice: metric_number(metrics.as_ref(), "dice"),
        mask_metrics: metrics,
        runtime_ms: inspected.elapsed_ms,
        peak_rss_bytes: inspected.peak_rss_bytes,
        viewer_runtime: viewer_runtime(&[("inspect", inspected)]),
        peak_tracked_heap_bytes: peak_tracked_heap(&[inspected]),
        viewer_implementation: inspected.report["implementation"].clone(),
        probe_commands: probe_commands(&[("inspect", inspected)]),
        input_bytes: file_size(case.path)?,
        output_bytes: None,
        input_sop_instance_uid: case.expected["sop_instance_uid"]
            .as_str()
            .unwrap_or_default()
            .to_owned(),
        output_sop_instance_uid: None,
        message,
    })
}

fn mask_metrics(expected: &Value, actual: &Value) -> Result<Option<Value>, String> {
    let actual = actual.pointer("/semantic/data").unwrap_or(actual);
    let expected_masks = &expected["masks"];
    let actual_masks = &actual["masks"];
    if expected_masks["mode"] != "FullBinary" || actual_masks["mode"] != "FullBinary" {
        return Ok(None);
    }
    let expected_runs: Vec<BinaryRun> = serde_json::from_value(expected_masks["runs"].clone())
        .map_err(|error| format!("ground-truth mask runs are invalid: {error}"))?;
    let actual_runs: Vec<BinaryRun> = serde_json::from_value(actual_masks["runs"].clone())
        .map_err(|error| format!("probe mask runs are invalid: {error}"))?;
    let source = &expected["source"];
    let rows = json_usize(source, "total_pixel_matrix_rows")?;
    let columns = json_usize(source, "total_pixel_matrix_columns")?;
    let spacing = source["pixel_spacing"]
        .as_array()
        .filter(|values| values.len() == 2)
        .ok_or_else(|| "ground-truth pixel spacing must contain two values".to_owned())?;
    let metrics = segmentation_metrics(
        &expected_runs,
        &actual_runs,
        (rows, columns),
        (
            spacing[0]
                .as_f64()
                .ok_or_else(|| "row spacing must be numeric".to_owned())?,
            spacing[1]
                .as_f64()
                .ok_or_else(|| "column spacing must be numeric".to_owned())?,
        ),
    )?;
    serde_json::to_value(metrics)
        .map(Some)
        .map_err(|error| format!("could not serialize mask metrics: {error}"))
}

fn rewrite_rejected(error: &ProbeError, output: &Path) -> bool {
    error
        .report
        .as_ref()
        .and_then(|report| report.pointer("/error/code"))
        .and_then(Value::as_str)
        == Some("OPERATION_FAILED")
        && !output.exists()
}

fn rewrite_identity_message(input: &DicomMetadata, output: &DicomMetadata) -> Option<String> {
    let mut failures = Vec::new();
    if input.sop_instance_uid == output.sop_instance_uid {
        failures.push("new SOP Instance UID".to_owned());
    }
    if input.study_instance_uid != output.study_instance_uid {
        failures.push("Study Instance UID".to_owned());
    }
    if input.series_instance_uid != output.series_instance_uid {
        failures.push("Series Instance UID".to_owned());
    }
    if input.frame_of_reference_uid != output.frame_of_reference_uid {
        failures.push("Frame of Reference UID".to_owned());
    }
    for key in input
        .preserved
        .keys()
        .chain(output.preserved.keys())
        .collect::<std::collections::BTreeSet<_>>()
    {
        if input.preserved.get(key) != output.preserved.get(key) {
            failures.push((*key).clone());
        }
    }
    (!failures.is_empty()).then(|| format!("rewrite identity failure: {}", failures.join(", ")))
}

fn messages<const N: usize>(results: [&ComparisonResult; N], extra: Option<String>) -> String {
    let mut messages = results
        .into_iter()
        .flat_map(|result| &result.findings)
        .map(|finding| finding.message.clone())
        .collect::<Vec<_>>();
    messages.extend(extra);
    messages.join("; ")
}

fn viewer_runtime(entries: &[(&str, &ProbeObservation)]) -> BTreeMap<String, Value> {
    entries
        .iter()
        .map(|(name, observation)| ((*name).to_owned(), observation.report["runtime"].clone()))
        .collect()
}

fn probe_commands(entries: &[(&str, &ProbeObservation)]) -> BTreeMap<String, Vec<String>> {
    entries
        .iter()
        .map(|(name, observation)| ((*name).to_owned(), observation.command.clone()))
        .collect()
}

fn peak_tracked_heap(observations: &[&ProbeObservation]) -> u64 {
    observations
        .iter()
        .filter_map(|observation| {
            observation
                .report
                .pointer("/runtime/peak_tracked_heap_bytes")
                .and_then(Value::as_u64)
        })
        .max()
        .unwrap_or(0)
}

fn max_error<const N: usize>(
    results: [&ComparisonResult; N],
    value: impl Fn(&ComparisonResult) -> f64,
) -> f64 {
    results.into_iter().map(value).fold(0.0, f64::max)
}

fn metric_number(metrics: Option<&Value>, key: &str) -> Option<f64> {
    metrics
        .and_then(|value| value.get(key))
        .and_then(Value::as_f64)
}

fn json_usize(object: &Value, key: &str) -> Result<usize, String> {
    object[key]
        .as_u64()
        .and_then(|value| usize::try_from(value).ok())
        .ok_or_else(|| format!("{key} must be an addressable positive integer"))
}

fn pass_status(passed: bool) -> String {
    if passed { "passed" } else { "failed" }.to_owned()
}

fn file_size(path: &Path) -> Result<u64, String> {
    fs::metadata(path)
        .map(|metadata| metadata.len())
        .map_err(|error| format!("could not stat {}: {error}", path.display()))
}

fn failed(
    case_id: &str,
    path: &Path,
    pyramid_level: u8,
    expected: Option<&Value>,
    error: String,
) -> MatrixObservation {
    MatrixObservation {
        case_id: case_id.to_owned(),
        pyramid_level,
        status: "failed".to_owned(),
        inspect_equal: false,
        roundtrip_equal: None,
        rewrite_rejected: false,
        highdicom_readable: false,
        coordinate_error_max_px: None,
        coordinate_error_median_px: None,
        coordinate_error_rms_px: None,
        dice: None,
        mask_metrics: None,
        runtime_ms: 0.0,
        peak_rss_bytes: 0,
        viewer_runtime: BTreeMap::new(),
        peak_tracked_heap_bytes: 0,
        viewer_implementation: json!({}),
        probe_commands: BTreeMap::new(),
        input_bytes: fs::metadata(path).map_or(0, |metadata| metadata.len()),
        output_bytes: None,
        input_sop_instance_uid: expected
            .and_then(|value| value.get("sop_instance_uid"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        output_sop_instance_uid: None,
        message: error,
    }
}
