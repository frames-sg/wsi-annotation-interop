use std::fmt;

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ValidatorStatus {
    Passed,
    KnownValidatorDefect,
    Unsupported,
    Failed,
    TimedOut,
    Unavailable,
}

#[derive(Debug, Serialize)]
pub struct ValidatorObservation {
    pub name: String,
    pub edition: Option<String>,
    pub files: Vec<String>,
    pub command: Vec<String>,
    pub status: ValidatorStatus,
    pub returncode: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub version_command: Vec<String>,
    pub version_returncode: Option<i32>,
    pub version_stdout: String,
    pub version_stderr: String,
    pub version_stdout_capture: StreamCapture,
    pub version_stderr_capture: StreamCapture,
    pub elapsed_ms: f64,
    pub peak_rss_bytes: u64,
    pub sampling: SamplingMetadata,
    pub stdout_capture: StreamCapture,
    pub stderr_capture: StreamCapture,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct StreamCapture {
    pub total_bytes: u64,
    pub truncated: bool,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct SamplingMetadata {
    pub sampled: bool,
    pub interval_ms: Option<u64>,
}

#[derive(Debug)]
pub struct ValidatorError(pub(super) String);

impl fmt::Display for ValidatorError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for ValidatorError {}
