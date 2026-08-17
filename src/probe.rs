use std::fmt;
use std::path::Path;
use std::time::Duration;

use serde_json::Value;

use crate::process::{ProcessError, run};
use crate::schema::{validate_conversion_report, validate_probe_report};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PayloadMode {
    Full,
    Digest,
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
}

#[derive(Debug, Clone)]
pub struct ProbeError {
    message: String,
    pub report: Option<Value>,
    pub stderr: String,
    pub returncode: Option<i32>,
    pub peak_rss_bytes: Option<u64>,
}

impl fmt::Display for ProbeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for ProbeError {}

#[derive(Debug, Clone)]
pub struct ViewerProbe {
    executable: Vec<String>,
    timeout: Duration,
}

impl ViewerProbe {
    /// Construct a process-only viewer adapter.
    ///
    /// # Errors
    ///
    /// Returns an error for an empty command or a zero timeout.
    pub fn new(executable: Vec<String>, timeout: Option<Duration>) -> Result<Self, String> {
        if executable.is_empty() {
            return Err("annotation_probe command must not be empty".to_owned());
        }
        let timeout = timeout.unwrap_or(Duration::from_mins(10));
        if timeout.is_zero() {
            return Err("timeout must be positive".to_owned());
        }
        Ok(Self {
            executable,
            timeout,
        })
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
        command.extend(["inspect".to_owned(), "--source".to_owned()]);
        command.push(source.to_string_lossy().into_owned());
        if let Some(canonical_source) = canonical_source {
            command.push("--canonical-source".to_owned());
            command.push(canonical_source.to_string_lossy().into_owned());
        }
        command.extend([
            "--payload".to_owned(),
            payload.label().to_owned(),
            annotation.to_string_lossy().into_owned(),
        ]);
        self.execute(command, "inspect", validate_probe_report)
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
        command.extend(["roundtrip".to_owned(), "--source".to_owned()]);
        command.push(source.to_string_lossy().into_owned());
        command.push("--output".to_owned());
        command.push(output.to_string_lossy().into_owned());
        if let Some(canonical_source) = canonical_source {
            command.push("--canonical-source".to_owned());
            command.push(canonical_source.to_string_lossy().into_owned());
        }
        if allow_lossy {
            command.push("--allow-lossy".to_owned());
        }
        command.extend([
            "--payload".to_owned(),
            payload.label().to_owned(),
            annotation.to_string_lossy().into_owned(),
        ]);
        self.execute(command, "roundtrip", validate_probe_report)
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
            "convert-geojson".to_owned(),
            "--source".to_owned(),
            path_text(source),
        ]);
        if let Some(canonical_source) = canonical_source {
            command.extend(["--canonical-source".to_owned(), path_text(canonical_source)]);
        }
        command.extend([
            "--mapping".to_owned(),
            path_text(mapping),
            "--coordinate-space".to_owned(),
            coordinate_space.label().to_owned(),
        ]);
        for target in targets {
            command.extend(["--target".to_owned(), target.label().to_owned()]);
        }
        if allow_lossy {
            command.push("--allow-lossy".to_owned());
        }
        command.extend([
            "--output-dir".to_owned(),
            path_text(output_directory),
            path_text(geojson),
        ]);
        self.execute(command, "convert-geojson", validate_conversion_report)
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
            "convert-raster".to_owned(),
            "--source".to_owned(),
            path_text(source),
        ]);
        if let Some(canonical_source) = canonical_source {
            command.extend(["--canonical-source".to_owned(), path_text(canonical_source)]);
        }
        command.extend(["--profile".to_owned(), path_text(profile)]);
        match channels {
            RasterChannels::Auto => {}
            RasterChannels::One(value) => command.extend(["--channel".to_owned(), value]),
            RasterChannels::All => command.push("--all-channels".to_owned()),
        }
        if let Some(maximum) = maximum_instance_bytes {
            command.extend(["--max-instance-bytes".to_owned(), maximum.to_string()]);
        }
        command.extend([
            "--output-dir".to_owned(),
            path_text(output_directory),
            path_text(raster),
        ]);
        self.execute(command, "convert-raster", validate_conversion_report)
    }

    fn execute(
        &self,
        command: Vec<String>,
        expected_operation: &str,
        validate: fn(&Value) -> Result<(), String>,
    ) -> Result<ProbeObservation, ProbeError> {
        let output = run(&command, self.timeout).map_err(process_error)?;
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        let report: Value = serde_json::from_slice(&output.stdout).map_err(|error| ProbeError {
            message: format!("annotation_probe emitted invalid JSON: {error}"),
            report: None,
            stderr: stderr.clone(),
            returncode: output.status.code(),
            peak_rss_bytes: Some(output.peak_rss_bytes),
        })?;
        validate(&report).map_err(|message| ProbeError {
            message,
            report: Some(report.clone()),
            stderr: stderr.clone(),
            returncode: output.status.code(),
            peak_rss_bytes: Some(output.peak_rss_bytes),
        })?;
        let operation = report.get("operation").and_then(Value::as_str);
        if operation != Some(expected_operation) {
            return Err(ProbeError {
                message: format!(
                    "annotation_probe reported operation {} for requested {expected_operation}",
                    operation.unwrap_or("<missing>")
                ),
                report: Some(report),
                stderr,
                returncode: output.status.code(),
                peak_rss_bytes: Some(output.peak_rss_bytes),
            });
        }
        if !output.status.success() || report.get("status").and_then(Value::as_str) != Some("ok") {
            let message = report
                .pointer("/error/message")
                .and_then(Value::as_str)
                .map_or_else(
                    || format!("annotation_probe exited with status {}", output.status),
                    str::to_owned,
                );
            return Err(ProbeError {
                message,
                report: Some(report),
                stderr,
                returncode: output.status.code(),
                peak_rss_bytes: Some(output.peak_rss_bytes),
            });
        }
        Ok(ProbeObservation {
            command,
            report,
            stderr,
            elapsed_ms: output.elapsed.as_secs_f64() * 1000.0,
            peak_rss_bytes: output.peak_rss_bytes,
        })
    }
}

fn path_text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn configuration_error(message: &str) -> ProbeError {
    ProbeError {
        message: message.to_owned(),
        report: None,
        stderr: String::new(),
        returncode: None,
        peak_rss_bytes: None,
    }
}

fn process_error(error: ProcessError) -> ProbeError {
    match error {
        ProcessError::TimedOut {
            timeout,
            stdout,
            stderr,
            peak_rss_bytes,
        } => ProbeError {
            message: format!(
                "annotation_probe timed out after {} seconds",
                timeout.as_secs_f64()
            ),
            report: serde_json::from_slice(&stdout).ok(),
            stderr: String::from_utf8_lossy(&stderr).into_owned(),
            returncode: None,
            peak_rss_bytes: Some(peak_rss_bytes),
        },
        error => ProbeError {
            message: format!("annotation_probe execution failed: {error}"),
            report: None,
            stderr: String::new(),
            returncode: None,
            peak_rss_bytes: None,
        },
    }
}
