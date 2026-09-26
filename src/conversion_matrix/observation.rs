use std::path::PathBuf;

use serde_json::{Value, json};

use super::{ConversionObservation, ConversionStatus, ConversionTarget, verify_report_outputs};
use crate::probe::{ProbeError, ProbeObservation};

pub(super) fn failed_probe(
    case_id: &str,
    target: ConversionTarget,
    error: &ProbeError,
    paths: Vec<PathBuf>,
) -> ConversionObservation {
    let heap = error
        .report
        .as_ref()
        .and_then(|report| report["peak_tracked_heap_bytes"].as_u64());
    ConversionObservation {
        matrix: target.matrix(),
        case_id: case_id.to_owned(),
        target,
        status: ConversionStatus::Failed,
        highdicom_readable: false,
        output_paths: paths.into_iter().filter(|path| path.is_file()).collect(),
        report: json!({
            "execution_error": error,
            "measurements": {
                "runtime_ms": error.elapsed_ms(),
                "peak_rss_bytes": error.peak_rss_bytes,
                "peak_tracked_heap_bytes": heap,
            }
        }),
        normalized: Value::Null,
        command: error.command.as_deref().cloned().unwrap_or_default(),
        runtime_ms: error.elapsed_ms().unwrap_or(0.0),
        peak_rss_bytes: error.peak_rss_bytes.unwrap_or(0),
        peak_tracked_heap_bytes: heap.unwrap_or(0),
        message: error.to_string(),
    }
}

pub(super) fn observe_output(
    case_id: &str,
    target: ConversionTarget,
    probe: &ProbeObservation,
    paths: &[PathBuf],
    normalize: impl FnOnce() -> Result<Value, String>,
    validate: impl FnOnce(&Value) -> Result<(), String>,
) -> ConversionObservation {
    let mut observation = ConversionObservation {
        matrix: target.matrix(),
        case_id: case_id.to_owned(),
        target,
        status: ConversionStatus::Failed,
        highdicom_readable: false,
        output_paths: paths
            .iter()
            .filter(|path| path.is_file())
            .cloned()
            .collect(),
        report: probe.report.clone(),
        normalized: Value::Null,
        command: probe.command.clone(),
        runtime_ms: probe.elapsed_ms,
        peak_rss_bytes: probe.peak_rss_bytes,
        peak_tracked_heap_bytes: probe.report["peak_tracked_heap_bytes"]
            .as_u64()
            .unwrap_or(0),
        message: String::new(),
    };
    let target_name = match target {
        ConversionTarget::Ann => "ann",
        ConversionTarget::Seg => "seg",
        ConversionTarget::Sr => "sr",
        ConversionTarget::Pm => "pm",
    };
    let outcome = verify_report_outputs(probe, target_name, paths)
        .and_then(|()| normalize())
        .and_then(|normalized| {
            observation.highdicom_readable = true;
            observation.normalized = normalized;
            validate(&observation.normalized)
        });
    match outcome {
        Ok(()) => observation.status = ConversionStatus::Passed,
        Err(error) => observation.message = error,
    }
    observation
}
