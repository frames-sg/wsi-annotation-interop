use std::fs;
use std::path::Path;

use serde_json::Value;

use crate::compare::ComparisonResult;

use super::MatrixObservation;
use super::case::{Case, CaseDomain};
use super::observation::CaseRecorder;
use super::phases::{InputPhases, RewritePhases, VerificationPhases};

pub(super) fn finalize_roundtrip(
    case: &Case<'_>,
    domain: CaseDomain<'_>,
    recorder: CaseRecorder,
    input: &InputPhases,
    rewrite: &RewritePhases,
    verification: VerificationPhases,
) -> MatrixObservation {
    let roundtrip_equal = rewrite.viewer.is_ok()
        && rewrite.reference.is_ok()
        && verification.identity_message.is_none();
    let message = messages(
        [
            &input.reference,
            &input.comparison,
            &rewrite.viewer,
            &rewrite.reference,
        ],
        verification.identity_message,
    );
    let mut observation = recorder.finish();
    observation.inspect_equal = input.comparison.is_ok();
    observation.roundtrip_equal = Some(roundtrip_equal);
    observation.highdicom_readable = true;
    match domain {
        CaseDomain::Ann { .. } => {
            let errors = [&input.comparison, &rewrite.viewer, &rewrite.reference];
            observation.coordinate_error_max_px =
                Some(max_error(errors, |result| result.coordinate_error.max));
            observation.coordinate_error_median_px =
                Some(max_error(errors, |result| result.coordinate_error.median));
            observation.coordinate_error_rms_px =
                Some(max_error(errors, |result| result.coordinate_error.rms));
        }
        CaseDomain::Seg { .. } => {
            observation.dice = metric_number(verification.metrics.as_ref(), "dice");
            observation.mask_metrics = verification.metrics;
        }
    }
    observation.input_bytes = file_size(case.path).unwrap_or(observation.input_bytes);
    observation.output_bytes = Some(verification.output_bytes);
    observation.input_sop_instance_uid = verification.input_metadata.sop_instance_uid;
    observation.output_sop_instance_uid = Some(verification.output_metadata.sop_instance_uid);
    observation.message = message;
    observation
}
pub(super) fn messages<const N: usize>(
    results: [&ComparisonResult; N],
    extra: Option<String>,
) -> String {
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

pub(super) fn metric_number(metrics: Option<&Value>, key: &str) -> Option<f64> {
    metrics
        .and_then(|value| value.get(key))
        .and_then(Value::as_f64)
}

pub(super) fn file_size(path: &Path) -> Result<u64, String> {
    fs::metadata(path)
        .map(|metadata| metadata.len())
        .map_err(|error| format!("could not stat {}: {error}", path.display()))
}
