use std::fs::File;
use std::io::Read;
use std::path::Path;
use std::time::Duration;

use crate::process::{CommandSpec, ProcessError, run as process_run};

use super::discovery::executable_path;
use super::{StreamCapture, empty_capture, stream_capture, text};

pub(super) struct VersionOutput {
    pub(super) returncode: Option<i32>,
    pub(super) stdout: String,
    pub(super) stderr: String,
    pub(super) stdout_capture: StreamCapture,
    pub(super) stderr_capture: StreamCapture,
}

pub(super) fn run(command: &[String], timeout: Duration) -> VersionOutput {
    if command.is_empty() {
        return VersionOutput {
            returncode: None,
            stdout: String::new(),
            stderr: "version command was not configured".to_owned(),
            stdout_capture: empty_capture(),
            stderr_capture: empty_capture(),
        };
    }
    let Ok(command) = CommandSpec::from_strings(command.to_vec(), "validator version") else {
        return VersionOutput {
            returncode: None,
            stdout: String::new(),
            stderr: "version command has an empty program".to_owned(),
            stdout_capture: empty_capture(),
            stderr_capture: empty_capture(),
        };
    };
    match process_run(&command.process_spec(timeout)) {
        Ok(output) => VersionOutput {
            returncode: output.status.code(),
            stdout: text(&output.stdout.bytes),
            stderr: text(&output.stderr.bytes),
            stdout_capture: stream_capture(&output.stdout),
            stderr_capture: stream_capture(&output.stderr),
        },
        Err(ProcessError::TimedOut { output, .. }) => VersionOutput {
            returncode: None,
            stdout: text(&output.stdout.bytes),
            stderr: format!("{}version command timed out", text(&output.stderr.bytes)),
            stdout_capture: stream_capture(&output.stdout),
            stderr_capture: stream_capture(&output.stderr),
        },
        Err(error) => VersionOutput {
            returncode: None,
            stdout: String::new(),
            stderr: error.to_string(),
            stdout_capture: empty_capture(),
            stderr_capture: empty_capture(),
        },
    }
}

pub(super) fn validate_iods_command() -> Vec<String> {
    executable_path("validate_iods")
        .and_then(|entrypoint| python_distribution_version_command(&entrypoint, "dicom-validator"))
        .unwrap_or_else(|| vec!["validate_iods".to_owned(), "--version".to_owned()])
}

pub(super) fn python_distribution_version_command(
    entrypoint: &Path,
    distribution: &str,
) -> Option<Vec<String>> {
    if !distribution
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return None;
    }
    let mut bytes = Vec::new();
    File::open(entrypoint)
        .ok()?
        .take(4_096)
        .read_to_end(&mut bytes)
        .ok()?;
    let first_line = std::str::from_utf8(&bytes).ok()?.lines().next()?;
    let mut command = first_line
        .strip_prefix("#!")?
        .split_whitespace()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let invokes_python = command.iter().any(|part| {
        Path::new(part)
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.contains("python"))
    });
    if command.is_empty() || !invokes_python {
        return None;
    }
    command.push("-c".to_owned());
    command.push(format!(
        "import importlib.metadata; print('{distribution} ' + importlib.metadata.version('{distribution}'))"
    ));
    Some(command)
}
