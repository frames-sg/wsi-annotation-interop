use std::fmt;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Serialize;

use crate::process::{ProcessError, run};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValidatorInvocation {
    Each,
    Set,
}

#[derive(Debug, Clone)]
pub struct ValidatorSpec {
    pub name: String,
    pub command: Vec<String>,
    pub version_command: Vec<String>,
    pub validation_args: Vec<String>,
    pub invocation: ValidatorInvocation,
    pub edition: Option<String>,
    pub unsupported_markers: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct ValidatorObservation {
    pub name: String,
    pub edition: Option<String>,
    pub files: Vec<String>,
    pub command: Vec<String>,
    pub status: String,
    pub returncode: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub version_command: Vec<String>,
    pub version_returncode: Option<i32>,
    pub version_stdout: String,
    pub version_stderr: String,
    pub elapsed_ms: f64,
    pub peak_rss_bytes: u64,
}

#[derive(Debug)]
pub struct ValidatorError(String);

impl fmt::Display for ValidatorError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for ValidatorError {}

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
    let paths: Vec<_> = files
        .iter()
        .map(|path| path.to_string_lossy().into_owned())
        .collect();
    if !executable_available(&spec.command[0]) {
        return Ok(vec![unavailable(spec, &paths)]);
    }
    let version = run_version(&spec.version_command, timeout);
    let groups: Vec<Vec<String>> = match spec.invocation {
        ValidatorInvocation::Each => paths.iter().cloned().map(|path| vec![path]).collect(),
        ValidatorInvocation::Set => vec![paths],
    };
    Ok(groups
        .into_iter()
        .map(|group| run_group(spec, group, &version, timeout))
        .collect())
}

/// Return the four validators required by the full study profile.
#[must_use]
pub fn standard_validator_specs(edition: &str) -> Vec<ValidatorSpec> {
    vec![
        ValidatorSpec {
            name: "validate_iods".to_owned(),
            command: vec!["validate_iods".to_owned()],
            version_command: validate_iods_version_command(),
            validation_args: vec!["--edition".to_owned(), edition.to_owned()],
            invocation: ValidatorInvocation::Each,
            edition: Some(edition.to_owned()),
            unsupported_markers: vec!["Unknown or retired SOP Class UID".to_owned()],
        },
        ValidatorSpec {
            name: "dciodvfy".to_owned(),
            command: vec!["dciodvfy".to_owned()],
            version_command: vec!["dciodvfy".to_owned(), "-version".to_owned()],
            validation_args: Vec::new(),
            invocation: ValidatorInvocation::Each,
            edition: Some("dicom3tools embedded dictionary".to_owned()),
            unsupported_markers: Vec::new(),
        },
        ValidatorSpec {
            name: "dcentvfy".to_owned(),
            command: vec!["dcentvfy".to_owned()],
            version_command: vec!["dcentvfy".to_owned(), "-version".to_owned()],
            validation_args: Vec::new(),
            invocation: ValidatorInvocation::Set,
            edition: Some("dicom3tools embedded dictionary".to_owned()),
            unsupported_markers: Vec::new(),
        },
        ValidatorSpec {
            name: "dcm2json".to_owned(),
            command: vec!["dcm2json".to_owned()],
            version_command: vec!["dcm2json".to_owned(), "--version".to_owned()],
            validation_args: Vec::new(),
            invocation: ValidatorInvocation::Each,
            edition: Some("DCMTK embedded dictionary".to_owned()),
            unsupported_markers: Vec::new(),
        },
    ]
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

/// Reclassify the documented `validate_iods` Parametric Map functional-group
/// table defect after the independent highdicom PM control exhibits it too.
///
/// Returns `true` only when the control confirms the exact defect signature.
pub fn qualify_validate_iods_pm_defect(
    observations: &mut [ValidatorObservation],
    highdicom_pm: &Path,
) -> bool {
    let oracle_path = highdicom_pm.to_string_lossy();
    let oracle_confirmed = observations.iter().any(|observation| {
        observation.files.as_slice() == [oracle_path.as_ref()]
            && is_known_parametric_map_table_defect(observation)
    });
    if !oracle_confirmed {
        return false;
    }
    for observation in observations {
        if is_known_parametric_map_table_defect(observation) {
            "known_validator_defect".clone_into(&mut observation.status);
        }
    }
    true
}

/// Reclassify one documented `validate_iods` SEG functional-group table
/// signature after an independent highdicom SEG exhibits the same signature.
pub fn qualify_validate_iods_seg_defect(
    observations: &mut [ValidatorObservation],
    highdicom_seg: &Path,
) -> bool {
    let oracle_path = highdicom_seg.to_string_lossy();
    let Some(oracle_signature) = observations
        .iter()
        .find(|observation| observation.files.as_slice() == [oracle_path.as_ref()])
        .and_then(segmentation_table_signature)
    else {
        return false;
    };
    for observation in observations {
        if segmentation_table_signature(observation) == Some(oracle_signature) {
            "known_validator_defect".clone_into(&mut observation.status);
        }
    }
    true
}

/// Reclassify stale TID 1410 checks for tiled SEG references only after the
/// same validator reports the exact defect for an independent highdicom SR.
///
/// Current TID 1410 requires both frame and segment selectors for a tiled SEG.
pub fn qualify_tiled_segmentation_sr_validator_defect(
    observations: &mut [ValidatorObservation],
    highdicom_sr: &Path,
) -> bool {
    let oracle_path = highdicom_sr.to_string_lossy();
    let confirmed_validators = observations
        .iter()
        .filter(|observation| {
            observation.files.as_slice() == [oracle_path.as_ref()]
                && is_known_tiled_segmentation_sr_defect(observation)
        })
        .map(|observation| observation.name.clone())
        .collect::<Vec<_>>();
    if confirmed_validators.is_empty() {
        return false;
    }
    for observation in observations {
        if confirmed_validators.contains(&observation.name)
            && is_known_tiled_segmentation_sr_defect(observation)
        {
            "known_validator_defect".clone_into(&mut observation.status);
        }
    }
    true
}

fn is_known_parametric_map_table_defect(observation: &ValidatorObservation) -> bool {
    const REQUIRED: [&str; 5] = [
        "Pixel Measures Sequence",
        "Frame VOI LUT Sequence",
        "Pixel Value Transformation Sequence",
        "Parametric Map Frame Type Sequence",
        "Real World Value Mapping Sequence",
    ];
    const ALLOWED: [&str; 7] = [
        "Pixel Measures Sequence",
        "Frame VOI LUT Sequence",
        "Pixel Value Transformation Sequence",
        "Parametric Map Frame Type Sequence",
        "Real World Value Mapping Sequence",
        "Derivation Image Sequence",
        "Frame Content Sequence",
    ];
    if observation.name != "validate_iods"
        || observation.status != "failed"
        || observation.files.len() != 1
    {
        return false;
    }
    let output = format!("{}{}", observation.stdout, observation.stderr);
    if !output.contains("(Parametric Map IOD)")
        || REQUIRED
            .iter()
            .any(|name| !output.contains(&format!("({name}) is unexpected")))
    {
        return false;
    }
    output.lines().all(|line| {
        let line = line.trim();
        line.is_empty()
            || line.starts_with("Using DICOM edition ")
            || line.starts_with("Validating DICOM file ")
            || line.starts_with("SOP class is ")
            || matches!(
                line,
                "Errors" | "======" | "Module \"Multi-frame Functional Groups\":"
            )
            || matches!(
                line,
                "(5200,9229) (Shared Functional Groups Sequence):"
                    | "(5200,9230) (Per-Frame Functional Groups Sequence):"
            )
            || (line.starts_with("Tag ")
                && ALLOWED
                    .iter()
                    .any(|name| line.ends_with(&format!("({name}) is unexpected"))))
    })
}

fn segmentation_table_signature(observation: &ValidatorObservation) -> Option<u8> {
    const FUNCTIONAL_GROUPS: [&str; 5] = [
        "Derivation Image Sequence",
        "Pixel Measures Sequence",
        "Frame Content Sequence",
        "Plane Position (Slide) Sequence",
        "Segment Identification Sequence",
    ];
    if observation.name != "validate_iods"
        || observation.status != "failed"
        || observation.files.len() != 1
    {
        return None;
    }
    let output = format!("{}{}", observation.stdout, observation.stderr);
    if !output.contains("(Segmentation IOD)") {
        return None;
    }
    let signature = FUNCTIONAL_GROUPS
        .iter()
        .enumerate()
        .fold(0_u8, |signature, (index, name)| {
            if output.contains(&format!("({name}) is unexpected")) {
                signature | (1 << index)
            } else {
                signature
            }
        });
    if signature == 0 {
        return None;
    }
    output
        .lines()
        .all(|line| {
            let line = line.trim();
            line.is_empty()
                || line.starts_with("Using DICOM edition ")
                || line.starts_with("Validating DICOM file ")
                || line.starts_with("SOP class is ")
                || matches!(
                    line,
                    "Errors" | "======" | "Module \"Multi-frame Functional Groups\":"
                )
                || matches!(
                    line,
                    "(5200,9229) (Shared Functional Groups Sequence):"
                        | "(5200,9230) (Per-Frame Functional Groups Sequence):"
                )
                || (line.starts_with("Tag ")
                    && FUNCTIONAL_GROUPS
                        .iter()
                        .any(|name| line.ends_with(&format!("({name}) is unexpected"))))
        })
        .then_some(signature)
}

fn is_known_tiled_segmentation_sr_defect(observation: &ValidatorObservation) -> bool {
    if observation.status != "failed" || observation.files.len() != 1 {
        return false;
    }
    let output = format!("{}{}", observation.stdout, observation.stderr);
    match observation.name.as_str() {
        "dciodvfy" => {
            const DEFECT: &str = "Error - Shall not be present when ReferencedFrameNumber is present - attribute <ReferencedSegmentNumber>";
            output.contains("Comprehensive3DSR")
                && output
                    .lines()
                    .filter(|line| line.trim_start().starts_with("Error - "))
                    .eq([DEFECT])
        }
        "validate_iods" => is_known_validate_iods_tiled_segmentation_sr_defect(&output),
        _ => false,
    }
}

fn is_known_validate_iods_tiled_segmentation_sr_defect(output: &str) -> bool {
    const FRAME_ERROR: &str = "Tag (0008,1160) (Referenced Frame Number) is unexpected";
    const SEGMENT_ERROR: &str = "Tag (0062,000B) (Referenced Segment Number) is unexpected";
    if !output.contains("(Comprehensive 3D SR IOD)")
        || !output.contains(FRAME_ERROR)
        || !output.contains(SEGMENT_ERROR)
    {
        return false;
    }
    output.lines().all(|line| {
        let line = line.trim();
        line.is_empty()
            || line.starts_with("Using DICOM edition ")
            || line.starts_with("Validating DICOM file ")
            || line.starts_with("SOP class is ")
            || matches!(
                line,
                "Errors" | "======" | "Module \"SR Document Content\":"
            )
            || (line.starts_with("(0040,A730) ")
                && line.ends_with("(0008,1199) (Referenced SOP Sequence):"))
            || matches!(line, FRAME_ERROR | SEGMENT_ERROR)
    })
}

struct VersionOutput {
    returncode: Option<i32>,
    stdout: String,
    stderr: String,
}

struct ValidationOutput {
    status: &'static str,
    returncode: Option<i32>,
    stdout: String,
    stderr: String,
    elapsed_ms: f64,
    peak_rss_bytes: u64,
}

fn run_version(command: &[String], timeout: Duration) -> VersionOutput {
    if command.is_empty() {
        return VersionOutput {
            returncode: None,
            stdout: String::new(),
            stderr: "version command was not configured".to_owned(),
        };
    }
    match run(command, timeout) {
        Ok(output) => VersionOutput {
            returncode: output.status.code(),
            stdout: text(&output.stdout),
            stderr: text(&output.stderr),
        },
        Err(ProcessError::TimedOut { stdout, stderr, .. }) => VersionOutput {
            returncode: None,
            stdout: text(&stdout),
            stderr: format!("{}version command timed out", text(&stderr)),
        },
        Err(error) => VersionOutput {
            returncode: None,
            stdout: String::new(),
            stderr: error.to_string(),
        },
    }
}

fn run_group(
    spec: &ValidatorSpec,
    files: Vec<String>,
    version: &VersionOutput,
    timeout: Duration,
) -> ValidatorObservation {
    let mut command = spec.command.clone();
    command.extend_from_slice(&spec.validation_args);
    command.extend(files.iter().cloned());
    let output = match run(&command, timeout) {
        Ok(output) => {
            let stdout = text(&output.stdout);
            let stderr = text(&output.stderr);
            let combined = format!("{stdout}{stderr}");
            let status = if output.status.success()
                && !(spec.name == "dciodvfy"
                    && combined
                        .lines()
                        .any(|line| line.trim_start().starts_with("Error - ")))
            {
                "passed"
            } else if spec
                .unsupported_markers
                .iter()
                .any(|marker| combined.contains(marker))
            {
                "unsupported"
            } else {
                "failed"
            };
            ValidationOutput {
                status,
                returncode: output.status.code(),
                stdout,
                stderr,
                elapsed_ms: output.elapsed.as_secs_f64() * 1000.0,
                peak_rss_bytes: output.peak_rss_bytes,
            }
        }
        Err(ProcessError::TimedOut {
            timeout,
            stdout,
            stderr,
            peak_rss_bytes,
        }) => ValidationOutput {
            status: "timed_out",
            returncode: None,
            stdout: text(&stdout),
            stderr: format!(
                "{}validator timed out after {} seconds",
                text(&stderr),
                timeout.as_secs_f64()
            ),
            elapsed_ms: timeout.as_secs_f64() * 1000.0,
            peak_rss_bytes,
        },
        Err(error) => ValidationOutput {
            status: "unavailable",
            returncode: None,
            stdout: String::new(),
            stderr: error.to_string(),
            elapsed_ms: 0.0,
            peak_rss_bytes: 0,
        },
    };
    observation(spec, files, command, version, output)
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
        status: output.status.to_owned(),
        returncode: output.returncode,
        stdout: output.stdout,
        stderr: output.stderr,
        version_command: spec.version_command.clone(),
        version_returncode: version.returncode,
        version_stdout: version.stdout.clone(),
        version_stderr: version.stderr.clone(),
        elapsed_ms: output.elapsed_ms,
        peak_rss_bytes: output.peak_rss_bytes,
    }
}

fn unavailable(spec: &ValidatorSpec, files: &[String]) -> ValidatorObservation {
    let mut command = spec.command.clone();
    command.extend_from_slice(&spec.validation_args);
    command.extend_from_slice(files);
    let version = VersionOutput {
        returncode: None,
        stdout: String::new(),
        stderr: String::new(),
    };
    let output = ValidationOutput {
        status: "unavailable",
        returncode: None,
        stdout: String::new(),
        stderr: "validator executable was not found".to_owned(),
        elapsed_ms: 0.0,
        peak_rss_bytes: 0,
    };
    observation(spec, files.to_vec(), command, &version, output)
}

fn executable_available(executable: &str) -> bool {
    executable_path(executable).is_some()
}

fn executable_path(executable: &str) -> Option<PathBuf> {
    let path = Path::new(executable);
    if path.components().count() > 1 {
        return path.is_file().then(|| path.to_path_buf());
    }
    std::env::var_os("PATH").and_then(|value| {
        std::env::split_paths(&value)
            .map(|directory| directory.join(executable))
            .find(|candidate| candidate.is_file())
    })
}

fn validate_iods_version_command() -> Vec<String> {
    executable_path("validate_iods")
        .and_then(|entrypoint| python_distribution_version_command(&entrypoint, "dicom-validator"))
        .unwrap_or_else(|| vec!["validate_iods".to_owned(), "--version".to_owned()])
}

fn python_distribution_version_command(
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

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::python_distribution_version_command;

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
