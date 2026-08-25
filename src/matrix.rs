use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs;
use std::path::Path;

use serde_json::Value;

use crate::compare::{ComparisonResult, compare_ann, compare_seg};
use crate::ground_truth::build_core_ground_truth;
use crate::metrics::{BinaryRun, segmentation_metrics};
use crate::probe::{PayloadMode, ProbeError, ProbeObservation, ViewerProbe};
use crate::shim::{DicomMetadata, FixtureSet, ReferenceShim};

const PIXEL_TOLERANCE: f64 = 1e-6;
const MILLIMETER_TOLERANCE: f64 = 1e-9;

mod observation;

use observation::{CaseRecorder, pass_status};
pub use observation::{
    CoreMatrixResult, MatrixObservation, MatrixPhase, PhaseObservation, PhaseStatus,
};

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
        observations.push(run_ann(
            &Case {
                id: case_id,
                expected: required_case(cases, case_id)?,
                path,
                source: &fixtures.source,
            },
            services,
            None,
            0,
        ));
    }

    let pyramid_case = "ann-2d-volume-level1";
    observations.push(run_ann(
        &Case {
            id: pyramid_case,
            expected: required_case(cases, pyramid_case)?,
            path: &fixtures.pyramid_ann,
            source: &fixtures.pyramid_source,
        },
        services,
        Some(&fixtures.source),
        1,
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
        observations.push(run_seg(
            &Case {
                id: case_id,
                expected: required_case(cases, case_id)?,
                path,
                source: &fixtures.source,
            },
            services,
            kind,
        ));
    }

    let reordered_case = "seg-binary-reordered";
    observations.push(run_seg(
        &Case {
            id: reordered_case,
            expected: required_case(cases, reordered_case)?,
            path: &fixtures.reordered_seg,
            source: &fixtures.source,
        },
        services,
        "BINARY",
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

fn run_ann(
    case: &Case<'_>,
    services: &MatrixServices<'_>,
    canonical_source: Option<&Path>,
    pyramid_level: u8,
) -> MatrixObservation {
    run_case(
        case,
        services,
        CaseDomain::Ann {
            canonical_source,
            pyramid_level,
        },
    )
}

fn run_seg(case: &Case<'_>, services: &MatrixServices<'_>, kind: &str) -> MatrixObservation {
    run_case(
        case,
        services,
        CaseDomain::Seg {
            rewrite: if kind == "BINARY" {
                RewritePolicy::Required
            } else {
                RewritePolicy::RejectUnsupported
            },
        },
    )
}

#[derive(Clone, Copy)]
enum RewritePolicy {
    Required,
    RejectUnsupported,
}

#[derive(Clone, Copy)]
enum CaseDomain<'a> {
    Ann {
        canonical_source: Option<&'a Path>,
        pyramid_level: u8,
    },
    Seg {
        rewrite: RewritePolicy,
    },
}

impl<'a> CaseDomain<'a> {
    const fn canonical_source(self) -> Option<&'a Path> {
        match self {
            Self::Ann {
                canonical_source, ..
            } => canonical_source,
            Self::Seg { .. } => None,
        }
    }

    const fn pyramid_level(self) -> u8 {
        match self {
            Self::Ann { pyramid_level, .. } => pyramid_level,
            Self::Seg { .. } => 0,
        }
    }

    fn normalize(
        self,
        reference: &ReferenceShim,
        annotation: &Path,
        source: &Path,
    ) -> Result<Value, String> {
        match self {
            Self::Ann {
                canonical_source, ..
            } => reference
                .normalize_ann(annotation, source, canonical_source)
                .map_err(|error| error.to_string()),
            Self::Seg { .. } => reference
                .normalize_seg(annotation, source)
                .map_err(|error| error.to_string()),
        }
    }

    fn compare(self, expected: &Value, actual: &Value) -> Result<ComparisonResult, String> {
        match self {
            Self::Ann { .. } => {
                compare_ann(expected, actual, PIXEL_TOLERANCE, MILLIMETER_TOLERANCE)
            }
            Self::Seg { .. } => compare_seg(expected, actual),
        }
    }

    const fn rewrite_policy(self) -> RewritePolicy {
        match self {
            Self::Ann { .. } => RewritePolicy::Required,
            Self::Seg { rewrite } => rewrite,
        }
    }
}

#[allow(
    clippy::too_many_lines,
    reason = "the shared phase lifecycle is intentionally linear"
)]
fn run_case(
    case: &Case<'_>,
    services: &MatrixServices<'_>,
    domain: CaseDomain<'_>,
) -> MatrixObservation {
    let Case {
        id: case_id,
        expected,
        path,
        source,
    } = *case;
    let mut recorder = CaseRecorder::new(case_id, path, domain.pyramid_level(), Some(expected));
    let normalized_input = match domain.normalize(services.reference, path, source) {
        Ok(value) => {
            recorder.pass(MatrixPhase::ReferenceInputNormalization);
            recorder.observation.highdicom_readable = true;
            value
        }
        Err(error) => return recorder.fail(MatrixPhase::ReferenceInputNormalization, error),
    };
    let reference_input = match domain.compare(expected, &normalized_input) {
        Ok(result) => result,
        Err(error) => return recorder.fail(MatrixPhase::ReferenceInputComparison, error),
    };
    recorder.comparison(MatrixPhase::ReferenceInputComparison, &reference_input);
    let inspected =
        match services
            .probe
            .inspect(source, path, domain.canonical_source(), PayloadMode::Full)
        {
            Ok(observation) => observation,
            Err(error) => return recorder.fail_probe(MatrixPhase::ViewerInspection, error),
        };
    recorder.probe(MatrixPhase::ViewerInspection, &inspected, None);
    let inspect = match domain.compare(expected, &inspected.report) {
        Ok(result) => result,
        Err(error) => return recorder.fail(MatrixPhase::InspectComparison, error),
    };
    recorder.comparison(MatrixPhase::InspectComparison, &inspect);
    recorder.observation.inspect_equal = inspect.is_ok();
    if matches!(domain.rewrite_policy(), RewritePolicy::RejectUnsupported) {
        return read_only_seg(
            case,
            services,
            recorder,
            &reference_input,
            &inspected,
            &inspect,
        );
    }

    let output_path = services
        .output_directory
        .join(format!("{case_id}-roundtrip.dcm"));
    recorder.pass(MatrixPhase::RewriteRequest);
    let roundtripped = match services.probe.roundtrip(
        source,
        path,
        &output_path,
        domain.canonical_source(),
        PayloadMode::Full,
        false,
    ) {
        Ok(observation) => observation,
        Err(error) => return recorder.fail_probe(MatrixPhase::RewriteResponse, error),
    };
    recorder.probe(
        MatrixPhase::RewriteResponse,
        &roundtripped,
        Some(&output_path),
    );
    let viewer = match domain.compare(expected, &roundtripped.report) {
        Ok(result) => result,
        Err(error) => return recorder.fail(MatrixPhase::OutputComparison, error),
    };
    recorder.comparison(MatrixPhase::OutputComparison, &viewer);
    let normalized_output = match domain.normalize(services.reference, &output_path, source) {
        Ok(value) => {
            recorder.pass(MatrixPhase::ReferenceOutputNormalization);
            value
        }
        Err(error) => return recorder.fail(MatrixPhase::ReferenceOutputNormalization, error),
    };
    let reference_output = match domain.compare(expected, &normalized_output) {
        Ok(result) => result,
        Err(error) => return recorder.fail(MatrixPhase::ReferenceOutputComparison, error),
    };
    recorder.comparison(MatrixPhase::ReferenceOutputComparison, &reference_output);
    let input_metadata = match services.reference.metadata(path) {
        Ok(metadata) => metadata,
        Err(error) => return recorder.fail(MatrixPhase::IdentityVerification, error.to_string()),
    };
    let output_metadata = match services.reference.metadata(&output_path) {
        Ok(metadata) => metadata,
        Err(error) => return recorder.fail(MatrixPhase::IdentityVerification, error.to_string()),
    };
    let identity_message = rewrite_identity_message(&input_metadata, &output_metadata);
    recorder.record(PhaseObservation {
        phase: MatrixPhase::IdentityVerification,
        status: pass_status(identity_message.is_none()),
        command: None,
        elapsed_ms: None,
        peak_rss_bytes: None,
        output_truncated: None,
        error_code: identity_message
            .as_ref()
            .map(|_| "IDENTITY_MISMATCH".to_owned()),
        message: identity_message.clone().unwrap_or_default(),
        artifacts: vec![output_path.to_string_lossy().into_owned()],
        details: None,
    });
    let metrics = if matches!(domain, CaseDomain::Seg { .. }) {
        let metrics = match mask_metrics(expected, &roundtripped.report) {
            Ok(metrics) => metrics,
            Err(error) => return recorder.fail(MatrixPhase::MaskMetrics, error),
        };
        recorder.record(metric_phase(metrics.as_ref()));
        metrics
    } else {
        None
    };
    let output_bytes = match file_size(&output_path) {
        Ok(bytes) => {
            recorder.pass(MatrixPhase::ArtifactVerification);
            bytes
        }
        Err(error) => return recorder.fail(MatrixPhase::ArtifactVerification, error),
    };
    let roundtrip_equal = viewer.is_ok() && reference_output.is_ok() && identity_message.is_none();
    let message = messages(
        [&reference_input, &inspect, &viewer, &reference_output],
        identity_message,
    );

    let mut observation = recorder.finish();
    observation.inspect_equal = inspect.is_ok();
    observation.roundtrip_equal = Some(roundtrip_equal);
    observation.highdicom_readable = true;
    match domain {
        CaseDomain::Ann { .. } => {
            let errors = [&inspect, &viewer, &reference_output];
            observation.coordinate_error_max_px =
                Some(max_error(errors, |result| result.coordinate_error.max));
            observation.coordinate_error_median_px =
                Some(max_error(errors, |result| result.coordinate_error.median));
            observation.coordinate_error_rms_px =
                Some(max_error(errors, |result| result.coordinate_error.rms));
        }
        CaseDomain::Seg { .. } => {
            observation.dice = metric_number(metrics.as_ref(), "dice");
            observation.mask_metrics = metrics;
        }
    }
    observation.input_bytes = file_size(path).unwrap_or(observation.input_bytes);
    observation.output_bytes = Some(output_bytes);
    observation.input_sop_instance_uid = input_metadata.sop_instance_uid;
    observation.output_sop_instance_uid = Some(output_metadata.sop_instance_uid);
    observation.message = message;
    observation
}

fn read_only_seg(
    case: &Case<'_>,
    services: &MatrixServices<'_>,
    mut recorder: CaseRecorder,
    reference_input: &ComparisonResult,
    inspected: &ProbeObservation,
    inspect: &ComparisonResult,
) -> MatrixObservation {
    let output_path = services
        .output_directory
        .join(format!("{}-roundtrip.dcm", case.id));
    recorder.pass(MatrixPhase::RewriteRequest);
    let (rejected, rejection_code, rejection_message) = match services.probe.roundtrip(
        case.source,
        case.path,
        &output_path,
        None,
        PayloadMode::Full,
        false,
    ) {
        Err(error) => {
            let rejected = rewrite_rejected(&error, &output_path);
            let code = error
                .report
                .as_ref()
                .and_then(|report| report.pointer("/error/code"))
                .and_then(Value::as_str)
                .map(str::to_owned);
            let message = error.to_string();
            recorder.record(PhaseObservation {
                phase: MatrixPhase::RewriteResponse,
                status: if rejected {
                    PhaseStatus::RejectedAsExpected
                } else {
                    PhaseStatus::Failed
                },
                command: error.command.map(|command| *command),
                elapsed_ms: None,
                peak_rss_bytes: error.peak_rss_bytes,
                output_truncated: error
                    .process
                    .as_ref()
                    .map(|process| process.stdout_truncated || process.stderr_truncated),
                error_code: code.clone(),
                message: message.clone(),
                artifacts: Vec::new(),
                details: error.report,
            });
            (rejected, code, message)
        }
        Ok(observation) => {
            recorder.record(PhaseObservation {
                phase: MatrixPhase::RewriteResponse,
                status: PhaseStatus::Failed,
                command: Some(observation.command),
                elapsed_ms: Some(observation.elapsed_ms),
                peak_rss_bytes: observation
                    .rss_sampled
                    .then_some(observation.peak_rss_bytes),
                output_truncated: Some(observation.stderr_truncated),
                error_code: Some("REWRITE_NOT_REJECTED".to_owned()),
                message: "read-only SEG rewrite unexpectedly succeeded".to_owned(),
                artifacts: if output_path.exists() {
                    vec![output_path.to_string_lossy().into_owned()]
                } else {
                    Vec::new()
                },
                details: Some(observation.report),
            });
            (false, None, "rewrite unexpectedly succeeded".to_owned())
        }
    };
    let mut message = messages([reference_input, inspect], None);
    if !rejected {
        if !message.is_empty() {
            message.push_str("; ");
        }
        message.push_str("read-only SEG rewrite was not safely rejected");
        if let Some(code) = rejection_code {
            let _ = write!(message, " (code {code})");
        }
        if !rejection_message.is_empty() {
            let _ = write!(message, ": {rejection_message}");
        }
    }
    let metrics = match mask_metrics(case.expected, &inspected.report) {
        Ok(metrics) => metrics,
        Err(error) => return recorder.fail(MatrixPhase::MaskMetrics, error),
    };
    recorder.record(metric_phase(metrics.as_ref()));
    let mut observation = recorder.finish();
    observation.inspect_equal = inspect.is_ok();
    observation.rewrite_rejected = rejected;
    observation.highdicom_readable = true;
    observation.dice = metric_number(metrics.as_ref(), "dice");
    observation.mask_metrics = metrics;
    case.expected["sop_instance_uid"]
        .as_str()
        .unwrap_or_default()
        .clone_into(&mut observation.input_sop_instance_uid);
    observation.message = message;
    observation
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

fn metric_phase(metrics: Option<&Value>) -> PhaseObservation {
    let status = match metrics
        .and_then(|value| value.pointer("/advanced_metrics/status"))
        .and_then(Value::as_str)
    {
        Some("resource_limited") => PhaseStatus::ResourceLimited,
        None if metrics.is_none() => PhaseStatus::NotRun,
        Some(_) | None => PhaseStatus::Passed,
    };
    PhaseObservation {
        phase: MatrixPhase::MaskMetrics,
        status,
        command: None,
        elapsed_ms: None,
        peak_rss_bytes: None,
        output_truncated: None,
        error_code: (status == PhaseStatus::ResourceLimited)
            .then(|| "METRIC_RESOURCE_LIMIT".to_owned()),
        message: if status == PhaseStatus::ResourceLimited {
            "advanced mask metrics exceeded the configured crop budget".to_owned()
        } else {
            String::new()
        },
        artifacts: Vec::new(),
        details: metrics.cloned(),
    }
}

fn rewrite_rejected(error: &ProbeError, output: &Path) -> bool {
    error
        .report
        .as_ref()
        .and_then(|report| report.pointer("/error/code"))
        .and_then(Value::as_str)
        == Some("REWRITE_UNSUPPORTED")
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

fn file_size(path: &Path) -> Result<u64, String> {
    fs::metadata(path)
        .map(|metadata| metadata.len())
        .map_err(|error| format!("could not stat {}: {error}", path.display()))
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;
    use std::time::Duration;

    use serde_json::json;
    use tempfile::tempdir;

    use super::{
        Case, CaseRecorder, MatrixPhase, MatrixServices, PhaseObservation, PhaseStatus,
        rewrite_rejected, run_ann,
    };
    use crate::probe::{PayloadMode, ViewerProbe};
    use crate::schema::validate_matrix_observation;
    use crate::shim::ReferenceShim;

    #[test]
    fn late_failure_preserves_completed_phase_evidence() {
        let directory = tempdir().expect("temporary directory");
        let input = directory.path().join("input.dcm");
        fs::write(&input, b"dicom").expect("fixture write");
        let mut recorder = CaseRecorder::new("case", &input, 0, None);
        recorder.record(PhaseObservation {
            phase: MatrixPhase::ViewerInspection,
            status: PhaseStatus::Passed,
            command: Some(vec!["annotation_probe".to_owned(), "inspect".to_owned()]),
            elapsed_ms: Some(12.5),
            peak_rss_bytes: Some(4096),
            output_truncated: Some(false),
            error_code: None,
            message: String::new(),
            artifacts: Vec::new(),
            details: None,
        });

        let observation = recorder.fail(
            MatrixPhase::ReferenceOutputNormalization,
            "independent reader rejected output".to_owned(),
        );

        assert_eq!(observation.status, PhaseStatus::Failed);
        assert!((observation.runtime_ms - 12.5).abs() < f64::EPSILON);
        assert_eq!(observation.peak_rss_bytes, 4096);
        assert_eq!(
            observation.probe_commands["viewer_inspection"][1],
            "inspect"
        );
        assert_eq!(observation.phases.len(), 2);
        assert_eq!(observation.phases[0].status, PhaseStatus::Passed);
        assert_eq!(
            observation.phases[1].phase,
            MatrixPhase::ReferenceOutputNormalization
        );
        assert_eq!(observation.phases[1].status, PhaseStatus::Failed);
        validate_matrix_observation(
            &serde_json::to_value(&observation).expect("observation serialization"),
        )
        .expect("phase-preserving observation schema");
    }

    #[test]
    fn reference_output_failure_keeps_real_inspection_and_rewrite_evidence() {
        let directory = tempdir().expect("temporary directory");
        let input = directory.path().join("input.dcm");
        fs::write(&input, b"dicom").expect("fixture write");
        let mut inspect_report: serde_json::Value =
            serde_json::from_str(include_str!("../tests/data/ann-report.json"))
                .expect("inspect report");
        inspect_report["input"]["path"] = json!(input.to_string_lossy());
        let mut roundtrip_report = inspect_report.clone();
        roundtrip_report["operation"] = json!("roundtrip");
        roundtrip_report["output"] = roundtrip_report["input"].clone();
        let output = directory.path().join("case-roundtrip.dcm");
        roundtrip_report["output"]["path"] = json!(output.to_string_lossy());
        let inspect_json = serde_json::to_string(&inspect_report).expect("inspect JSON");
        let roundtrip_json = serde_json::to_string(&roundtrip_report).expect("roundtrip JSON");
        let probe_script = format!(
            r#"if [ "$1" = roundtrip ]; then
shift
while [ "$#" -gt 0 ]; do
  if [ "$1" = --output ]; then printf dicom > "$2"; fi
  shift
done
printf '%s' '{roundtrip_json}'
else
printf '%s' '{inspect_json}'
fi"#
        );
        let reference_script = format!(
            r#"case "$*" in
*roundtrip.dcm*) printf 'late independent normalization failure' >&2; exit 7 ;;
*) printf '%s' '{inspect_json}' ;;
esac"#
        );
        let reference = ReferenceShim::new(
            vec![
                "/bin/sh".to_owned(),
                "-c".to_owned(),
                reference_script,
                "reference".to_owned(),
            ],
            Duration::from_secs(2),
        )
        .expect("reference configuration");
        let probe = ViewerProbe::new(
            vec![
                "/bin/sh".to_owned(),
                "-c".to_owned(),
                probe_script,
                "probe".to_owned(),
            ],
            Some(Duration::from_secs(2)),
        )
        .expect("probe configuration");
        let services = MatrixServices {
            reference: &reference,
            probe: &probe,
            output_directory: directory.path(),
        };

        let observation = run_ann(
            &Case {
                id: "case",
                expected: &inspect_report,
                path: &input,
                source: Path::new("source.dcm"),
            },
            &services,
            None,
            0,
        );

        assert_eq!(observation.status, PhaseStatus::Failed);
        assert!(observation.inspect_equal);
        assert!(observation.highdicom_readable);
        assert!(observation.runtime_ms > 0.0);
        assert!(observation.output_bytes.is_some());
        assert!(observation.probe_commands.contains_key("viewer_inspection"));
        assert!(observation.probe_commands.contains_key("rewrite_response"));
        assert_eq!(
            observation.phases.last().map(|phase| phase.phase),
            Some(MatrixPhase::ReferenceOutputNormalization)
        );
        assert_eq!(
            observation.phases.last().map(|phase| phase.status),
            Some(PhaseStatus::Failed)
        );
        validate_matrix_observation(
            &serde_json::to_value(&observation).expect("observation serialization"),
        )
        .expect("real phase-preserving observation schema");
    }

    #[test]
    fn expected_rewrite_rejection_requires_a_precise_code_and_no_artifact() {
        let directory = tempdir().expect("temporary directory");
        let output = directory.path().join("roundtrip.dcm");
        let generic = probe_error("OPERATION_FAILED", &output);
        assert!(!rewrite_rejected(&generic, &output));

        let precise = probe_error("REWRITE_UNSUPPORTED", &output);
        assert!(rewrite_rejected(&precise, &output));

        fs::write(&output, b"partial").expect("partial output");
        assert!(!rewrite_rejected(&precise, &output));
    }

    #[test]
    fn case_status_derivation_is_deterministic() {
        let phase = |status| PhaseObservation {
            phase: MatrixPhase::ViewerInspection,
            status,
            command: None,
            elapsed_ms: None,
            peak_rss_bytes: None,
            output_truncated: None,
            error_code: None,
            message: String::new(),
            artifacts: Vec::new(),
            details: None,
        };
        assert_eq!(
            super::observation::derive_case_status(&[
                phase(PhaseStatus::RejectedAsExpected),
                phase(PhaseStatus::Passed),
            ]),
            PhaseStatus::Passed
        );
        assert_eq!(
            super::observation::derive_case_status(&[
                phase(PhaseStatus::ResourceLimited),
                phase(PhaseStatus::Failed),
            ]),
            PhaseStatus::Failed
        );
        assert_eq!(
            super::observation::derive_case_status(&[
                phase(PhaseStatus::Passed),
                phase(PhaseStatus::TimedOut),
            ]),
            PhaseStatus::TimedOut
        );
    }

    fn probe_error(code: &str, output: &Path) -> crate::probe::ProbeError {
        let report = json!({
            "schema_version": 1,
            "status": "error",
            "operation": "roundtrip",
            "implementation": {"name": "synthetic", "version": "1"},
            "error": {"code": code, "message": "rejected"}
        });
        let script = format!(
            "printf '%s' '{}'; exit 1",
            serde_json::to_string(&report).expect("report serialization")
        );
        ViewerProbe::new(
            vec!["/bin/sh".to_owned(), "-c".to_owned(), script],
            Some(Duration::from_secs(2)),
        )
        .expect("probe configuration")
        .roundtrip(
            Path::new("source.dcm"),
            Path::new("input.dcm"),
            output,
            None,
            PayloadMode::Digest,
            false,
        )
        .expect_err("synthetic probe rejection")
    }
}
