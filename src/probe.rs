use std::ffi::OsString;
use std::fmt;
use std::path::Path;
use std::time::Duration;

use serde_json::Value;

use crate::process::{CommandSpec, ProcessError, ProcessOutput, run};
use crate::schema::{validate_conversion_report, validate_probe_report};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PayloadMode {
    Full,
    Digest,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProbeOperation {
    Inspect,
    Roundtrip,
    ConvertGeoJson,
    ConvertRaster,
}

impl ProbeOperation {
    const fn label(self) -> &'static str {
        match self {
            Self::Inspect => "inspect",
            Self::Roundtrip => "roundtrip",
            Self::ConvertGeoJson => "convert-geojson",
            Self::ConvertRaster => "convert-raster",
        }
    }
}

impl PayloadMode {
    const fn label(self) -> &'static str {
        match self {
            Self::Full => "full",
            Self::Digest => "digest",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GeoJsonCoordinateSpace {
    Level0Pixels,
    SourcePixels,
    SlideMillimeters,
}

impl GeoJsonCoordinateSpace {
    const fn label(self) -> &'static str {
        match self {
            Self::Level0Pixels => "level0-pixels",
            Self::SourcePixels => "source-pixels",
            Self::SlideMillimeters => "slide-mm",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GeoJsonTarget {
    Ann,
    Seg,
    Sr,
}

impl GeoJsonTarget {
    const fn label(self) -> &'static str {
        match self {
            Self::Ann => "ann",
            Self::Seg => "seg",
            Self::Sr => "sr",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RasterChannels {
    Auto,
    One(String),
    All,
}

#[derive(Debug, Clone)]
pub struct ProbeObservation {
    pub command: Vec<String>,
    pub report: Value,
    pub stderr: String,
    pub elapsed_ms: f64,
    pub peak_rss_bytes: u64,
    pub rss_sampled: bool,
    pub sample_interval_ms: Option<u64>,
    pub stdout_total_bytes: u64,
    pub stderr_total_bytes: u64,
    pub stderr_truncated: bool,
}

#[derive(Debug, Clone)]
pub struct ProbeError {
    message: String,
    pub report: Option<Value>,
    pub stderr: String,
    pub returncode: Option<i32>,
    pub peak_rss_bytes: Option<u64>,
    pub process: Option<Box<ProbeProcessEvidence>>,
    pub command: Option<Box<Vec<String>>>,
}

#[derive(Debug, Clone)]
pub struct ProbeProcessEvidence {
    pub stdout_truncated: bool,
    pub stderr_truncated: bool,
    pub stdout_total_bytes: u64,
    pub stderr_total_bytes: u64,
    pub sample_interval_ms: Option<u64>,
}

impl fmt::Display for ProbeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for ProbeError {}

impl ProbeError {
    fn with_command(mut self, command: &[String]) -> Self {
        self.command = Some(Box::new(command.to_vec()));
        self
    }
}

#[derive(Debug, Clone)]
pub struct ViewerProbe {
    executable: CommandSpec,
    timeout: Duration,
}

impl ViewerProbe {
    /// Construct a process-only viewer adapter.
    ///
    /// # Errors
    ///
    /// Returns an error for an empty command or a zero timeout.
    pub fn new(executable: Vec<String>, timeout: Option<Duration>) -> Result<Self, String> {
        let executable = CommandSpec::from_strings(executable, "annotation_probe")?;
        Self::from_spec(executable, timeout)
    }

    /// Construct a viewer adapter without converting the program or fixed arguments to UTF-8.
    ///
    /// # Errors
    ///
    /// Returns an error for an empty program or a zero timeout.
    pub fn from_program(
        program: impl Into<OsString>,
        arguments: Vec<OsString>,
        timeout: Option<Duration>,
    ) -> Result<Self, String> {
        let executable = CommandSpec::new(program.into(), arguments)?;
        Self::from_spec(executable, timeout)
    }

    fn from_spec(executable: CommandSpec, timeout: Option<Duration>) -> Result<Self, String> {
        let timeout = timeout.unwrap_or(Duration::from_mins(10));
        if timeout.is_zero() {
            return Err("timeout must be positive".to_owned());
        }
        Ok(Self {
            executable,
            timeout,
        })
    }

    #[must_use]
    pub fn program(&self) -> &Path {
        Path::new(self.executable.program())
    }

    /// Inspect one ANN or SEG object through the public probe contract.
    ///
    /// # Errors
    ///
    /// Returns a structured error for process, timeout, JSON, schema, or probe failures.
    pub fn inspect(
        &self,
        source: &Path,
        annotation: &Path,
        canonical_source: Option<&Path>,
        payload: PayloadMode,
    ) -> Result<ProbeObservation, ProbeError> {
        let mut command = self.executable.clone();
        command.extend(["inspect", "--source"]);
        command.push(source.as_os_str());
        if let Some(canonical_source) = canonical_source {
            command.push("--canonical-source");
            command.push(canonical_source.as_os_str());
        }
        command.extend([
            OsString::from("--payload"),
            OsString::from(payload.label()),
            annotation.as_os_str().to_owned(),
        ]);
        self.execute(&command, ProbeOperation::Inspect, validate_probe_report)
    }

    /// Rewrite one ANN or SEG object through the public probe contract.
    ///
    /// # Errors
    ///
    /// Returns a structured error for process, timeout, JSON, schema, or probe failures.
    pub fn roundtrip(
        &self,
        source: &Path,
        annotation: &Path,
        output: &Path,
        canonical_source: Option<&Path>,
        payload: PayloadMode,
        allow_lossy: bool,
    ) -> Result<ProbeObservation, ProbeError> {
        let mut command = self.executable.clone();
        command.extend(["roundtrip", "--source"]);
        command.push(source.as_os_str());
        command.push("--output");
        command.push(output.as_os_str());
        if let Some(canonical_source) = canonical_source {
            command.push("--canonical-source");
            command.push(canonical_source.as_os_str());
        }
        if allow_lossy {
            command.push("--allow-lossy");
        }
        command.extend([
            OsString::from("--payload"),
            OsString::from(payload.label()),
            annotation.as_os_str().to_owned(),
        ]);
        self.execute(&command, ProbeOperation::Roundtrip, validate_probe_report)
    }

    /// Convert profiled `GeoJSON` into a deterministic multi-object bundle.
    ///
    /// # Errors
    ///
    /// Returns a structured error for process, schema, or conversion failures.
    #[allow(clippy::too_many_arguments)]
    pub fn convert_geojson_bundle(
        &self,
        source: &Path,
        canonical_source: Option<&Path>,
        mapping: &Path,
        coordinate_space: GeoJsonCoordinateSpace,
        targets: &[GeoJsonTarget],
        output_directory: &Path,
        geojson: &Path,
        allow_lossy: bool,
    ) -> Result<ProbeObservation, ProbeError> {
        if targets.is_empty() {
            return Err(configuration_error(
                "GeoJSON conversion requires at least one target",
            ));
        }
        let mut command = self.executable.clone();
        command.extend([
            OsString::from("convert-geojson"),
            OsString::from("--source"),
            source.as_os_str().to_owned(),
        ]);
        if let Some(canonical_source) = canonical_source {
            command.extend([
                OsString::from("--canonical-source"),
                canonical_source.as_os_str().to_owned(),
            ]);
        }
        command.extend([
            OsString::from("--mapping"),
            mapping.as_os_str().to_owned(),
            OsString::from("--coordinate-space"),
            OsString::from(coordinate_space.label()),
        ]);
        for target in targets {
            command.extend(["--target", target.label()]);
        }
        if allow_lossy {
            command.push("--allow-lossy");
        }
        command.extend([
            OsString::from("--output-dir"),
            output_directory.as_os_str().to_owned(),
            geojson.as_os_str().to_owned(),
        ]);
        self.execute(
            &command,
            ProbeOperation::ConvertGeoJson,
            validate_conversion_report,
        )
    }

    /// Convert a profiled raster into a Parametric Map bundle.
    ///
    /// # Errors
    ///
    /// Returns a structured error for process, schema, or conversion failures.
    #[allow(clippy::too_many_arguments)]
    pub fn convert_raster_bundle(
        &self,
        source: &Path,
        canonical_source: Option<&Path>,
        profile: &Path,
        channels: RasterChannels,
        output_directory: &Path,
        maximum_instance_bytes: Option<u64>,
        raster: &Path,
    ) -> Result<ProbeObservation, ProbeError> {
        let mut command = self.executable.clone();
        command.extend([
            OsString::from("convert-raster"),
            OsString::from("--source"),
            source.as_os_str().to_owned(),
        ]);
        if let Some(canonical_source) = canonical_source {
            command.extend([
                OsString::from("--canonical-source"),
                canonical_source.as_os_str().to_owned(),
            ]);
        }
        command.extend([OsString::from("--profile"), profile.as_os_str().to_owned()]);
        match channels {
            RasterChannels::Auto => {}
            RasterChannels::One(value) => {
                command.extend([OsString::from("--channel"), OsString::from(value)]);
            }
            RasterChannels::All => command.push("--all-channels"),
        }
        if let Some(maximum) = maximum_instance_bytes {
            command.extend([
                OsString::from("--max-instance-bytes"),
                OsString::from(maximum.to_string()),
            ]);
        }
        command.extend([
            OsString::from("--output-dir"),
            output_directory.as_os_str().to_owned(),
            raster.as_os_str().to_owned(),
        ]);
        self.execute(
            &command,
            ProbeOperation::ConvertRaster,
            validate_conversion_report,
        )
    }

    fn execute(
        &self,
        command: &CommandSpec,
        expected_operation: ProbeOperation,
        validate: fn(&Value) -> Result<(), String>,
    ) -> Result<ProbeObservation, ProbeError> {
        let display_command = command.display();
        let output = run(&command.process_spec(self.timeout))
            .map_err(|error| process_error(error).with_command(&display_command))?;
        let stderr = String::from_utf8_lossy(&output.stderr.bytes).into_owned();
        if output.stdout.truncated {
            return Err(observed_error(
                format!(
                    "annotation_probe report exceeded the {} byte stdout limit ({} bytes observed)",
                    output.stdout.bytes.len(),
                    output.stdout.total_bytes
                ),
                None,
                stderr,
                &output,
                &display_command,
            ));
        }
        let report: Value = serde_json::from_slice(&output.stdout.bytes).map_err(|error| {
            observed_error(
                format!("annotation_probe emitted invalid JSON: {error}"),
                None,
                stderr.clone(),
                &output,
                &display_command,
            )
        })?;
        validate(&report).map_err(|message| {
            observed_error(
                message,
                Some(report.clone()),
                stderr.clone(),
                &output,
                &display_command,
            )
        })?;
        let operation = report.get("operation").and_then(Value::as_str);
        if operation != Some(expected_operation.label()) {
            return Err(observed_error(
                format!(
                    "annotation_probe reported operation {} for requested {}",
                    operation.unwrap_or("<missing>"),
                    expected_operation.label()
                ),
                Some(report),
                stderr,
                &output,
                &display_command,
            ));
        }
        if !output.status.success() || report.get("status").and_then(Value::as_str) != Some("ok") {
            let message = report
                .pointer("/error/message")
                .and_then(Value::as_str)
                .map_or_else(
                    || format!("annotation_probe exited with status {}", output.status),
                    str::to_owned,
                );
            return Err(observed_error(
                message,
                Some(report),
                stderr,
                &output,
                &display_command,
            ));
        }
        Ok(ProbeObservation {
            command: display_command,
            report,
            stderr,
            elapsed_ms: output.elapsed.as_secs_f64() * 1000.0,
            peak_rss_bytes: output.peak_rss_bytes,
            rss_sampled: output.rss_sampled,
            sample_interval_ms: output
                .sample_interval
                .and_then(|interval| u64::try_from(interval.as_millis()).ok()),
            stdout_total_bytes: output.stdout.total_bytes,
            stderr_total_bytes: output.stderr.total_bytes,
            stderr_truncated: output.stderr.truncated,
        })
    }
}

fn configuration_error(message: &str) -> ProbeError {
    ProbeError {
        message: message.to_owned(),
        report: None,
        stderr: String::new(),
        returncode: None,
        peak_rss_bytes: None,
        process: None,
        command: None,
    }
}

fn observed_error(
    message: String,
    report: Option<Value>,
    stderr: String,
    output: &ProcessOutput,
    command: &[String],
) -> ProbeError {
    ProbeError {
        message,
        report,
        stderr,
        returncode: output.status.code(),
        peak_rss_bytes: output.rss_sampled.then_some(output.peak_rss_bytes),
        process: Some(Box::new(ProbeProcessEvidence {
            stdout_truncated: output.stdout.truncated,
            stderr_truncated: output.stderr.truncated,
            stdout_total_bytes: output.stdout.total_bytes,
            stderr_total_bytes: output.stderr.total_bytes,
            sample_interval_ms: output
                .sample_interval
                .and_then(|interval| u64::try_from(interval.as_millis()).ok()),
        })),
        command: Some(Box::new(command.to_vec())),
    }
}

fn process_error(error: ProcessError) -> ProbeError {
    match error {
        ProcessError::TimedOut { timeout, output } => ProbeError {
            message: format!(
                "annotation_probe timed out after {} seconds",
                timeout.as_secs_f64()
            ),
            report: (!output.stdout.truncated)
                .then(|| serde_json::from_slice(&output.stdout.bytes).ok())
                .flatten(),
            stderr: String::from_utf8_lossy(&output.stderr.bytes).into_owned(),
            returncode: None,
            peak_rss_bytes: output.rss_sampled.then_some(output.peak_rss_bytes),
            process: Some(Box::new(ProbeProcessEvidence {
                stdout_truncated: output.stdout.truncated,
                stderr_truncated: output.stderr.truncated,
                stdout_total_bytes: output.stdout.total_bytes,
                stderr_total_bytes: output.stderr.total_bytes,
                sample_interval_ms: output
                    .sample_interval
                    .and_then(|interval| u64::try_from(interval.as_millis()).ok()),
            })),
            command: None,
        },
        error => ProbeError {
            message: format!("annotation_probe execution failed: {error}"),
            report: None,
            stderr: String::new(),
            returncode: None,
            peak_rss_bytes: None,
            process: None,
            command: None,
        },
    }
}
