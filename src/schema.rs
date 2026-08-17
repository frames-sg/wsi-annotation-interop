use std::sync::OnceLock;

use jsonschema::Validator;
use serde_json::Value;

const PROBE_REPORT_SCHEMA: &str = include_str!("../schema/probe-report-v1.schema.json");
const CONVERSION_REPORT_SCHEMA: &str = include_str!("../schema/conversion-report-v1.schema.json");
const PATHOLOGY_MAPPING_SCHEMA: &str = include_str!("../schema/pathology-mapping-v1.schema.json");
const RASTER_PROFILE_SCHEMA: &str = include_str!("../schema/raster-profile-v1.schema.json");
const TILED_MANIFEST_SCHEMA: &str = include_str!("../schema/tiled-manifest-v1.schema.json");

static PROBE_VALIDATOR: OnceLock<Result<Validator, String>> = OnceLock::new();
static CONVERSION_VALIDATOR: OnceLock<Result<Validator, String>> = OnceLock::new();
static PATHOLOGY_MAPPING_VALIDATOR: OnceLock<Result<Validator, String>> = OnceLock::new();
static RASTER_PROFILE_VALIDATOR: OnceLock<Result<Validator, String>> = OnceLock::new();
static TILED_MANIFEST_VALIDATOR: OnceLock<Result<Validator, String>> = OnceLock::new();

/// Validate an `annotation_probe` report against the versioned public schema.
///
/// # Errors
///
/// Returns an error if the bundled schema cannot be compiled or the report does
/// not conform. At most five deterministic validation errors are included.
pub fn validate_probe_report(report: &Value) -> Result<(), String> {
    validate_document(
        report,
        PROBE_REPORT_SCHEMA,
        &PROBE_VALIDATOR,
        "probe report",
    )
}

/// Validate an `annotation_probe` conversion report against its public schema.
///
/// # Errors
///
/// Returns an error for an invalid bundled schema or a nonconforming report.
pub fn validate_conversion_report(report: &Value) -> Result<(), String> {
    validate_document(
        report,
        CONVERSION_REPORT_SCHEMA,
        &CONVERSION_VALIDATOR,
        "conversion report",
    )
}

/// Validate a pathology label and SR mapping profile.
///
/// # Errors
///
/// Returns an error for an invalid bundled schema or a nonconforming profile.
pub fn validate_pathology_mapping(profile: &Value) -> Result<(), String> {
    validate_document(
        profile,
        PATHOLOGY_MAPPING_SCHEMA,
        &PATHOLOGY_MAPPING_VALIDATOR,
        "pathology mapping",
    )
}

/// Validate a pathology raster profile.
///
/// # Errors
///
/// Returns an error for an invalid bundled schema or a nonconforming profile.
pub fn validate_raster_profile(profile: &Value) -> Result<(), String> {
    validate_document(
        profile,
        RASTER_PROFILE_SCHEMA,
        &RASTER_PROFILE_VALIDATOR,
        "raster profile",
    )
}

/// Validate a tiled raster manifest.
///
/// # Errors
///
/// Returns an error for an invalid bundled schema or a nonconforming manifest.
pub fn validate_tiled_manifest(manifest: &Value) -> Result<(), String> {
    validate_document(
        manifest,
        TILED_MANIFEST_SCHEMA,
        &TILED_MANIFEST_VALIDATOR,
        "tiled manifest",
    )
}

fn validate_document(
    value: &Value,
    schema_text: &str,
    cell: &OnceLock<Result<Validator, String>>,
    label: &str,
) -> Result<(), String> {
    let validator = cell
        .get_or_init(|| {
            let schema = serde_json::from_str(schema_text)
                .map_err(|error| format!("bundled {label} schema is invalid JSON: {error}"))?;
            jsonschema::validator_for(&schema)
                .map_err(|error| format!("bundled {label} schema is invalid: {error}"))
        })
        .as_ref()
        .map_err(Clone::clone)?;
    let mut errors: Vec<_> = validator
        .iter_errors(value)
        .map(|error| {
            let path = error.instance_path().to_string();
            (
                path.clone(),
                format!("{}: {error}", if path.is_empty() { "$" } else { &path }),
            )
        })
        .collect();
    if errors.is_empty() {
        return Ok(());
    }
    errors.sort_by(|left, right| left.0.cmp(&right.0).then_with(|| left.1.cmp(&right.1)));
    let extra = errors.len().saturating_sub(5);
    let details = errors
        .into_iter()
        .take(5)
        .map(|(_, message)| message)
        .collect::<Vec<_>>()
        .join("; ");
    let suffix = if extra == 0 {
        String::new()
    } else {
        format!("; and {extra} more")
    };
    Err(format!(
        "{label} schema validation failed: {details}{suffix}"
    ))
}
