use std::fmt::Write as _;
use std::path::Path;

use serde_json::Value;

use crate::compare::ComparisonResult;
use crate::metrics::{BinaryRun, segmentation_metrics};
use crate::probe::{PayloadMode, ProbeError, ProbeObservation};
use crate::shim::DicomMetadata;

use super::case::{Case, CaseDomain, MatrixServices, RewritePolicy};
use super::finalize::{file_size, finalize_roundtrip, messages, metric_number};
use super::observation::{CaseRecorder, pass_status};
use super::{MatrixObservation, MatrixPhase, PhaseObservation, PhaseStatus};

enum PhaseFailure {
    Message(MatrixPhase, String),
    Probe(MatrixPhase, ProbeError),
}

impl PhaseFailure {
    fn finish(self, recorder: CaseRecorder) -> MatrixObservation {
        match self {
            Self::Message(phase, message) => recorder.fail(phase, message),
            Self::Probe(phase, error) => recorder.fail_probe(phase, error),
        }
    }
}

pub(super) struct InputPhases {
    pub(super) reference: ComparisonResult,
    inspected: ProbeObservation,
    pub(super) comparison: ComparisonResult,
}

pub(super) struct RewritePhases {
    output_path: std::path::PathBuf,
    probe: ProbeObservation,
    pub(super) viewer: ComparisonResult,
    pub(super) reference: ComparisonResult,
}

pub(super) struct VerificationPhases {
    pub(super) input_metadata: DicomMetadata,
    pub(super) output_metadata: DicomMetadata,
    pub(super) identity_message: Option<String>,
    pub(super) metrics: Option<Value>,
    pub(super) output_bytes: u64,
}

pub(super) fn run_case(
    case: &Case<'_>,
    services: &MatrixServices<'_>,
    domain: CaseDomain<'_>,
) -> MatrixObservation {
    let mut recorder = CaseRecorder::new(
        case.id,
        case.path,
        domain.pyramid_level(),
        Some(case.expected),
    );
    let input = match run_input_phases(case, services, domain, &mut recorder) {
        Ok(input) => input,
        Err(failure) => return failure.finish(recorder),
    };
    if matches!(domain.rewrite_policy(), RewritePolicy::RejectUnsupported) {
        return read_only_seg(
            case,
            services,
            recorder,
            &input.reference,
            &input.inspected,
            &input.comparison,
        );
    }
    let rewrite = match run_rewrite_phases(case, services, domain, &mut recorder) {
        Ok(rewrite) => rewrite,
        Err(failure) => return failure.finish(recorder),
    };
    let verification =
        match run_verification_phases(case, services, domain, &rewrite, &mut recorder) {
            Ok(verification) => verification,
            Err(failure) => return failure.finish(recorder),
        };
    finalize_roundtrip(case, domain, recorder, &input, &rewrite, verification)
}

fn run_input_phases(
    case: &Case<'_>,
    services: &MatrixServices<'_>,
    domain: CaseDomain<'_>,
    recorder: &mut CaseRecorder,
) -> Result<InputPhases, PhaseFailure> {
    let normalized = domain
        .normalize(services.reference, case.path, case.source)
        .map_err(|error| PhaseFailure::Message(MatrixPhase::ReferenceInputNormalization, error))?;
    recorder.pass(MatrixPhase::ReferenceInputNormalization);
    recorder.observation.highdicom_readable = true;

    let reference = domain
        .compare(case.expected, &normalized)
        .map_err(|error| PhaseFailure::Message(MatrixPhase::ReferenceInputComparison, error))?;
    recorder.comparison(MatrixPhase::ReferenceInputComparison, &reference);

    let inspected = services
        .probe
        .inspect(
            case.source,
            case.path,
            domain.canonical_source(),
            PayloadMode::Full,
        )
        .map_err(|error| PhaseFailure::Probe(MatrixPhase::ViewerInspection, error))?;
    recorder.probe(MatrixPhase::ViewerInspection, &inspected, None);

    let comparison = domain
        .compare(case.expected, &inspected.report)
        .map_err(|error| PhaseFailure::Message(MatrixPhase::InspectComparison, error))?;
    recorder.comparison(MatrixPhase::InspectComparison, &comparison);
    recorder.observation.inspect_equal = comparison.is_ok();

    Ok(InputPhases {
        reference,
        inspected,
        comparison,
    })
}

fn run_rewrite_phases(
    case: &Case<'_>,
    services: &MatrixServices<'_>,
    domain: CaseDomain<'_>,
    recorder: &mut CaseRecorder,
) -> Result<RewritePhases, PhaseFailure> {
    let output_path = services
        .output_directory
        .join(format!("{}-roundtrip.dcm", case.id));
    recorder.pass(MatrixPhase::RewriteRequest);
    let probe = services
        .probe
        .roundtrip(
            case.source,
            case.path,
            &output_path,
            domain.canonical_source(),
            PayloadMode::Full,
            false,
        )
        .map_err(|error| PhaseFailure::Probe(MatrixPhase::RewriteResponse, error))?;
    recorder.probe(MatrixPhase::RewriteResponse, &probe, Some(&output_path));

    let viewer = domain
        .compare(case.expected, &probe.report)
        .map_err(|error| PhaseFailure::Message(MatrixPhase::OutputComparison, error))?;
    recorder.comparison(MatrixPhase::OutputComparison, &viewer);

    let normalized = domain
        .normalize(services.reference, &output_path, case.source)
        .map_err(|error| PhaseFailure::Message(MatrixPhase::ReferenceOutputNormalization, error))?;
    recorder.pass(MatrixPhase::ReferenceOutputNormalization);

    let reference = domain
        .compare(case.expected, &normalized)
        .map_err(|error| PhaseFailure::Message(MatrixPhase::ReferenceOutputComparison, error))?;
    recorder.comparison(MatrixPhase::ReferenceOutputComparison, &reference);

    Ok(RewritePhases {
        output_path,
        probe,
        viewer,
        reference,
    })
}

fn run_verification_phases(
    case: &Case<'_>,
    services: &MatrixServices<'_>,
    domain: CaseDomain<'_>,
    rewrite: &RewritePhases,
    recorder: &mut CaseRecorder,
) -> Result<VerificationPhases, PhaseFailure> {
    let input_metadata = services.reference.metadata(case.path).map_err(|error| {
        PhaseFailure::Message(MatrixPhase::IdentityVerification, error.to_string())
    })?;
    let output_metadata = services
        .reference
        .metadata(&rewrite.output_path)
        .map_err(|error| {
            PhaseFailure::Message(MatrixPhase::IdentityVerification, error.to_string())
        })?;
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
        artifacts: vec![rewrite.output_path.to_string_lossy().into_owned()],
        details: None,
    });

    let metrics = if matches!(domain, CaseDomain::Seg { .. }) {
        let metrics = mask_metrics(case.expected, &rewrite.probe.report)
            .map_err(|error| PhaseFailure::Message(MatrixPhase::MaskMetrics, error))?;
        recorder.record(metric_phase(metrics.as_ref()));
        metrics
    } else {
        None
    };
    let output_bytes = file_size(&rewrite.output_path)
        .map_err(|error| PhaseFailure::Message(MatrixPhase::ArtifactVerification, error))?;
    recorder.pass(MatrixPhase::ArtifactVerification);

    Ok(VerificationPhases {
        input_metadata,
        output_metadata,
        identity_message,
        metrics,
        output_bytes,
    })
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

pub(super) fn rewrite_rejected(error: &ProbeError, output: &Path) -> bool {
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

fn json_usize(object: &Value, key: &str) -> Result<usize, String> {
    object[key]
        .as_u64()
        .and_then(|value| usize::try_from(value).ok())
        .ok_or_else(|| format!("{key} must be an addressable positive integer"))
}
