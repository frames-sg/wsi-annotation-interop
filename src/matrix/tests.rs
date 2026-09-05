use std::fs;
use std::path::Path;
use std::time::Duration;

use serde_json::json;
use tempfile::tempdir;

use super::case::{Case, MatrixServices, run_ann};
use super::observation::CaseRecorder;
use super::phases::rewrite_rejected;
use super::{MatrixPhase, PhaseObservation, PhaseStatus};
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
        serde_json::from_str(include_str!("../../tests/data/ann-report.json"))
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
