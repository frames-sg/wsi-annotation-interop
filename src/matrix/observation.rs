use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use serde::Serialize;
use serde_json::{Value, json};

use crate::compare::ComparisonResult;
use crate::probe::{ProbeError, ProbeObservation};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PhaseStatus {
    Passed,
    Failed,
    RejectedAsExpected,
    Unavailable,
    TimedOut,
    ResourceLimited,
    NotRun,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MatrixPhase {
    ReferenceInputNormalization,
    ReferenceInputComparison,
    ViewerInspection,
    InspectComparison,
    RewriteRequest,
    RewriteResponse,
    ReferenceOutputNormalization,
    ReferenceOutputComparison,
    OutputComparison,
    IdentityVerification,
    MaskMetrics,
    ArtifactVerification,
}

impl MatrixPhase {
    pub(super) const fn label(self) -> &'static str {
        match self {
            Self::ReferenceInputNormalization => "reference_input_normalization",
            Self::ReferenceInputComparison => "reference_input_comparison",
            Self::ViewerInspection => "viewer_inspection",
            Self::InspectComparison => "inspect_comparison",
            Self::RewriteRequest => "rewrite_request",
            Self::RewriteResponse => "rewrite_response",
            Self::ReferenceOutputNormalization => "reference_output_normalization",
            Self::ReferenceOutputComparison => "reference_output_comparison",
            Self::OutputComparison => "output_comparison",
            Self::IdentityVerification => "identity_verification",
            Self::MaskMetrics => "mask_metrics",
            Self::ArtifactVerification => "artifact_verification",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PhaseObservation {
    pub phase: MatrixPhase,
    pub status: PhaseStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub elapsed_ms: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub peak_rss_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_truncated: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
    pub message: String,
    pub artifacts: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
}

#[derive(Debug, Serialize)]
pub struct MatrixObservation {
    pub schema_version: u8,
    pub case_id: String,
    pub pyramid_level: u8,
    pub status: PhaseStatus,
    pub phases: Vec<PhaseObservation>,
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
                .all(|observation| observation.status == PhaseStatus::Passed)
    }
}

pub(super) struct CaseRecorder {
    pub(super) observation: MatrixObservation,
}

impl CaseRecorder {
    pub(super) fn new(
        case_id: &str,
        path: &Path,
        pyramid_level: u8,
        expected: Option<&Value>,
    ) -> Self {
        let mut observation = failed_observation(case_id, path, pyramid_level, expected);
        observation.status = PhaseStatus::NotRun;
        Self { observation }
    }

    pub(super) fn record(&mut self, phase: PhaseObservation) {
        if let Some(elapsed_ms) = phase.elapsed_ms {
            self.observation.runtime_ms += elapsed_ms;
        }
        if let Some(peak_rss_bytes) = phase.peak_rss_bytes {
            self.observation.peak_rss_bytes = self.observation.peak_rss_bytes.max(peak_rss_bytes);
        }
        if let Some(command) = &phase.command {
            self.observation
                .probe_commands
                .insert(phase.phase.label().to_owned(), command.clone());
        }
        self.observation.phases.push(phase);
    }

    pub(super) fn pass(&mut self, phase: MatrixPhase) {
        self.record(PhaseObservation {
            phase,
            status: PhaseStatus::Passed,
            command: None,
            elapsed_ms: None,
            peak_rss_bytes: None,
            output_truncated: None,
            error_code: None,
            message: String::new(),
            artifacts: Vec::new(),
            details: None,
        });
    }

    pub(super) fn comparison(&mut self, phase: MatrixPhase, result: &ComparisonResult) {
        self.record(PhaseObservation {
            phase,
            status: pass_status(result.is_ok()),
            command: None,
            elapsed_ms: None,
            peak_rss_bytes: None,
            output_truncated: None,
            error_code: result.findings.first().map(|finding| finding.code.clone()),
            message: result
                .findings
                .iter()
                .map(|finding| finding.message.as_str())
                .collect::<Vec<_>>()
                .join("; "),
            artifacts: Vec::new(),
            details: serde_json::to_value(result).ok(),
        });
    }

    pub(super) fn probe(
        &mut self,
        phase: MatrixPhase,
        observation: &ProbeObservation,
        artifact: Option<&Path>,
    ) {
        self.observation.viewer_implementation = observation.report["implementation"].clone();
        self.observation.viewer_runtime.insert(
            phase.label().to_owned(),
            observation.report["runtime"].clone(),
        );
        self.observation.peak_tracked_heap_bytes = self.observation.peak_tracked_heap_bytes.max(
            observation
                .report
                .pointer("/runtime/peak_tracked_heap_bytes")
                .and_then(Value::as_u64)
                .unwrap_or(0),
        );
        if let Some(path) = artifact {
            self.observation.output_bytes = fs::metadata(path).ok().map(|metadata| metadata.len());
        }
        self.record(PhaseObservation {
            phase,
            status: PhaseStatus::Passed,
            command: Some(observation.command.clone()),
            elapsed_ms: Some(observation.elapsed_ms),
            peak_rss_bytes: observation
                .rss_sampled
                .then_some(observation.peak_rss_bytes),
            output_truncated: Some(observation.stderr_truncated),
            error_code: None,
            message: String::new(),
            artifacts: artifact
                .map(|path| vec![path.to_string_lossy().into_owned()])
                .unwrap_or_default(),
            details: Some(observation.report.clone()),
        });
    }

    pub(super) fn fail_probe(mut self, phase: MatrixPhase, error: ProbeError) -> MatrixObservation {
        let error_code = error
            .report
            .as_ref()
            .and_then(|report| report.pointer("/error/code"))
            .and_then(Value::as_str)
            .map(str::to_owned);
        let output_truncated = error
            .process
            .as_ref()
            .map(|process| process.stdout_truncated || process.stderr_truncated);
        let message = error.to_string();
        self.record(PhaseObservation {
            phase,
            status: if message.contains("timed out") {
                PhaseStatus::TimedOut
            } else {
                PhaseStatus::Failed
            },
            command: error.command.map(|command| *command),
            elapsed_ms: None,
            peak_rss_bytes: error.peak_rss_bytes,
            output_truncated,
            error_code,
            message: message.clone(),
            artifacts: Vec::new(),
            details: error.report,
        });
        self.observation.message = message;
        self.finish()
    }

    pub(super) fn fail(mut self, phase: MatrixPhase, message: String) -> MatrixObservation {
        self.record(PhaseObservation {
            phase,
            status: PhaseStatus::Failed,
            command: None,
            elapsed_ms: None,
            peak_rss_bytes: None,
            output_truncated: None,
            error_code: None,
            message: message.clone(),
            artifacts: Vec::new(),
            details: None,
        });
        self.observation.message = message;
        self.finish()
    }

    pub(super) fn finish(mut self) -> MatrixObservation {
        self.observation.status = derive_case_status(&self.observation.phases);
        self.observation
    }
}

pub(super) fn derive_case_status(phases: &[PhaseObservation]) -> PhaseStatus {
    if phases
        .iter()
        .any(|phase| phase.status == PhaseStatus::Failed)
    {
        PhaseStatus::Failed
    } else if phases
        .iter()
        .any(|phase| phase.status == PhaseStatus::TimedOut)
    {
        PhaseStatus::TimedOut
    } else if phases
        .iter()
        .any(|phase| phase.status == PhaseStatus::ResourceLimited)
    {
        PhaseStatus::ResourceLimited
    } else if phases
        .iter()
        .any(|phase| phase.status == PhaseStatus::Unavailable)
    {
        PhaseStatus::Unavailable
    } else if phases.is_empty()
        || phases
            .iter()
            .all(|phase| phase.status == PhaseStatus::NotRun)
    {
        PhaseStatus::NotRun
    } else {
        PhaseStatus::Passed
    }
}

pub(super) const fn pass_status(passed: bool) -> PhaseStatus {
    if passed {
        PhaseStatus::Passed
    } else {
        PhaseStatus::Failed
    }
}

fn failed_observation(
    case_id: &str,
    path: &Path,
    pyramid_level: u8,
    expected: Option<&Value>,
) -> MatrixObservation {
    MatrixObservation {
        schema_version: 2,
        case_id: case_id.to_owned(),
        pyramid_level,
        status: PhaseStatus::Failed,
        phases: Vec::new(),
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
        message: String::new(),
    }
}
