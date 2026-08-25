use std::path::PathBuf;
use std::time::Duration;

use crate::process::{CapturedStream, CommandSpec, ProcessError, run};

mod discovery;
mod known_defects;
mod observation;
mod spec;
mod version;

use discovery::executable_available;
use version::VersionOutput;

pub use known_defects::{
    qualify_tiled_segmentation_sr_validator_defect, qualify_validate_iods_pm_defect,
    qualify_validate_iods_seg_defect,
};
pub use observation::{
    SamplingMetadata, StreamCapture, ValidatorError, ValidatorObservation, ValidatorStatus,
};
pub use spec::{ValidatorInvocation, ValidatorSpec, standard_validator_specs};

/// Run one configured validator with complete command and version provenance.
///
/// # Errors
///
/// Returns an error for invalid configuration. Runtime failures are observations.
pub fn run_validator(
    spec: &ValidatorSpec,
    files: &[PathBuf],
    timeout: Duration,
) -> Result<Vec<ValidatorObservation>, ValidatorError> {
    if spec.command.is_empty() {
        return Err(ValidatorError(
            "validator command must not be empty".to_owned(),
        ));
    }
    if timeout.is_zero() {
        return Err(ValidatorError(
            "validator timeout must be positive".to_owned(),
        ));
    }
    if files.is_empty() {
        return Err(ValidatorError(
            "at least one DICOM file is required".to_owned(),
        ));
    }
    let base_command =
        CommandSpec::from_strings(spec.command.clone(), "validator").map_err(ValidatorError)?;
    if !executable_available(&spec.command[0]) {
        return Ok(vec![unavailable(spec, files)]);
    }
    let version = version::run(&spec.version_command, timeout);
    let groups: Vec<Vec<PathBuf>> = match spec.invocation {
        ValidatorInvocation::Each => files.iter().cloned().map(|path| vec![path]).collect(),
        ValidatorInvocation::Set => vec![files.to_vec()],
    };
    Ok(groups
        .into_iter()
        .map(|group| run_group(spec, &base_command, &group, &version, timeout))
        .collect())
}

/// Execute all standard validators.
///
/// # Errors
///
/// Returns an error only for invalid shared configuration.
pub fn run_standard_validators(
    files: &[PathBuf],
    edition: &str,
    timeout: Duration,
) -> Result<Vec<ValidatorObservation>, ValidatorError> {
    let mut observations = Vec::new();
    for spec in standard_validator_specs(edition) {
        observations.extend(run_validator(&spec, files, timeout)?);
    }
    Ok(observations)
}

struct ValidationOutput {
    status: ValidatorStatus,
    returncode: Option<i32>,
    stdout: String,
    stderr: String,
    elapsed_ms: f64,
    peak_rss_bytes: u64,
    sampling: SamplingMetadata,
    stdout_capture: StreamCapture,
    stderr_capture: StreamCapture,
}

fn run_group(
    spec: &ValidatorSpec,
    base_command: &CommandSpec,
    files: &[PathBuf],
    version: &VersionOutput,
    timeout: Duration,
) -> ValidatorObservation {
    let mut command = base_command.clone();
    command.extend(spec.validation_args.iter().cloned());
    command.extend(files.iter().map(|path| path.as_os_str().to_owned()));
    let display_command = command.display();
    let output = match run(&command.process_spec(timeout)) {
        Ok(output) => {
            let stdout = text(&output.stdout.bytes);
            let stderr = text(&output.stderr.bytes);
            let combined = format!("{stdout}{stderr}");
            let status = if output.stdout.truncated || output.stderr.truncated {
                ValidatorStatus::Failed
            } else if output.status.success()
                && !(spec.name == "dciodvfy"
                    && combined
                        .lines()
                        .any(|line| line.trim_start().starts_with("Error - ")))
            {
                ValidatorStatus::Passed
            } else if spec
                .unsupported_markers
                .iter()
                .any(|marker| combined.contains(marker))
            {
                ValidatorStatus::Unsupported
            } else {
                ValidatorStatus::Failed
            };
            ValidationOutput {
                status,
                returncode: output.status.code(),
                stdout,
                stderr,
                elapsed_ms: output.elapsed.as_secs_f64() * 1000.0,
                peak_rss_bytes: output.peak_rss_bytes,
                sampling: sampling_metadata(output.rss_sampled, output.sample_interval),
                stdout_capture: stream_capture(&output.stdout),
                stderr_capture: stream_capture(&output.stderr),
            }
        }
        Err(ProcessError::TimedOut { timeout, output }) => ValidationOutput {
            status: ValidatorStatus::TimedOut,
            returncode: None,
            stdout: text(&output.stdout.bytes),
            stderr: format!(
                "{}validator timed out after {} seconds",
                text(&output.stderr.bytes),
                timeout.as_secs_f64()
            ),
            elapsed_ms: output.elapsed.as_secs_f64() * 1000.0,
            peak_rss_bytes: output.peak_rss_bytes,
            sampling: sampling_metadata(output.rss_sampled, output.sample_interval),
            stdout_capture: stream_capture(&output.stdout),
            stderr_capture: stream_capture(&output.stderr),
        },
        Err(error) => ValidationOutput {
            status: ValidatorStatus::Unavailable,
            returncode: None,
            stdout: String::new(),
            stderr: error.to_string(),
            elapsed_ms: 0.0,
            peak_rss_bytes: 0,
            sampling: sampling_metadata(false, None),
            stdout_capture: empty_capture(),
            stderr_capture: empty_capture(),
        },
    };
    let display_files = files
        .iter()
        .map(|path| path.to_string_lossy().into_owned())
        .collect();
    observation(spec, display_files, display_command, version, output)
}

fn observation(
    spec: &ValidatorSpec,
    files: Vec<String>,
    command: Vec<String>,
    version: &VersionOutput,
    output: ValidationOutput,
) -> ValidatorObservation {
    ValidatorObservation {
        name: spec.name.clone(),
        edition: spec.edition.clone(),
        files,
        command,
        status: output.status,
        returncode: output.returncode,
        stdout: output.stdout,
        stderr: output.stderr,
        version_command: spec.version_command.clone(),
        version_returncode: version.returncode,
        version_stdout: version.stdout.clone(),
        version_stderr: version.stderr.clone(),
        version_stdout_capture: version.stdout_capture,
        version_stderr_capture: version.stderr_capture,
        elapsed_ms: output.elapsed_ms,
        peak_rss_bytes: output.peak_rss_bytes,
        sampling: output.sampling,
        stdout_capture: output.stdout_capture,
        stderr_capture: output.stderr_capture,
    }
}

fn unavailable(spec: &ValidatorSpec, files: &[PathBuf]) -> ValidatorObservation {
    let mut command = spec.command.clone();
    command.extend_from_slice(&spec.validation_args);
    command.extend(files.iter().map(|path| path.to_string_lossy().into_owned()));
    let version = VersionOutput {
        returncode: None,
        stdout: String::new(),
        stderr: String::new(),
        stdout_capture: empty_capture(),
        stderr_capture: empty_capture(),
    };
    let output = ValidationOutput {
        status: ValidatorStatus::Unavailable,
        returncode: None,
        stdout: String::new(),
        stderr: "validator executable was not found".to_owned(),
        elapsed_ms: 0.0,
        peak_rss_bytes: 0,
        sampling: sampling_metadata(false, None),
        stdout_capture: empty_capture(),
        stderr_capture: empty_capture(),
    };
    observation(
        spec,
        files
            .iter()
            .map(|path| path.to_string_lossy().into_owned())
            .collect(),
        command,
        &version,
        output,
    )
}

fn stream_capture(stream: &CapturedStream) -> StreamCapture {
    StreamCapture {
        total_bytes: stream.total_bytes,
        truncated: stream.truncated,
    }
}

const fn empty_capture() -> StreamCapture {
    StreamCapture {
        total_bytes: 0,
        truncated: false,
    }
}

fn sampling_metadata(sampled: bool, interval: Option<Duration>) -> SamplingMetadata {
    SamplingMetadata {
        sampled,
        interval_ms: interval.and_then(|value| u64::try_from(value.as_millis()).ok()),
    }
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::version::python_distribution_version_command;

    #[test]
    fn python_entrypoint_version_uses_the_entrypoints_own_interpreter() {
        let directory = tempdir().unwrap();
        let entrypoint = directory.path().join("validate_iods");
        fs::write(
            &entrypoint,
            "#!/opt/dicom-validator/bin/python\nfrom dicom_validator import main\n",
        )
        .unwrap();

        let command = python_distribution_version_command(&entrypoint, "dicom-validator").unwrap();

        assert_eq!(command[0], "/opt/dicom-validator/bin/python");
        assert_eq!(command[1], "-c");
        assert!(command[2].contains("dicom-validator"));
        assert!(command[2].contains("importlib.metadata"));
    }
}
