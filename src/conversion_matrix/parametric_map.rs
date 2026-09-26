use std::path::Path;

use super::expected;
use serde_json::{Value, json};

use super::inputs::ConversionInputs;
use super::observation::{failed_probe, observe_output};
use super::{ConversionObservation, ConversionTarget};
use crate::probe::{RasterChannels, ViewerProbe};
use crate::shim::{FixtureSet, ReferenceShim};

const FORCED_CONCATENATION_BYTES: u64 = 5_200;

pub(super) fn run_single(
    fixtures: &FixtureSet,
    reference: &ReferenceShim,
    probe: &ViewerProbe,
    output_directory: &Path,
    inputs: &ConversionInputs,
) -> Vec<ConversionObservation> {
    let bundle = output_directory.join("pm-float32");
    let result = probe.convert_raster_bundle(
        &fixtures.source,
        None,
        &inputs.raster_profile,
        RasterChannels::Auto,
        &bundle,
        None,
        &inputs.raster,
    );
    let path = bundle.join("pm-0001.dcm");
    let observation = match result {
        Ok(observation) => observation,
        Err(error) => {
            return vec![failed_probe(
                "pm-float32",
                ConversionTarget::Pm,
                &error,
                vec![path],
            )];
        }
    };
    vec![observe_output(
        "pm-float32",
        ConversionTarget::Pm,
        &observation,
        std::slice::from_ref(&path),
        || {
            reference
                .normalize_pm_samples(&path)
                .map_err(|error| error.to_string())
        },
        validate_single,
    )]
}

fn validate_single(normalized: &Value) -> Result<(), String> {
    validate_samples(normalized, None)?;
    if normalized["pixel"]["precision"].as_str() != Some("float32")
        || normalized["pixel"]["finite_count"].as_u64() != Some(255)
        || normalized["pixel"]["missing_count"].as_u64() != Some(1)
        || normalized["mappings"][0]["quantity"]["value"].as_str() != Some("TUMOR")
        || normalized["source_sop_instance_uids"][0].as_str()
            != Some("2.25.100000000000000000000000000000003")
    {
        return Err("independent PM normalization differs from the f32 profile".to_owned());
    }
    Ok(())
}

pub(super) fn run_concatenation(
    fixtures: &FixtureSet,
    reference: &ReferenceShim,
    probe: &ViewerProbe,
    output_directory: &Path,
    inputs: &ConversionInputs,
) -> Vec<ConversionObservation> {
    let bundle = output_directory.join("pm-concatenation");
    let result = probe.convert_raster_bundle(
        &fixtures.source,
        None,
        &inputs.concatenation_profile,
        RasterChannels::Auto,
        &bundle,
        Some(FORCED_CONCATENATION_BYTES),
        &inputs.wide_raster,
    );
    // This fixture requires three frames/instances, independent of the producer's report.
    let paths = (1..=3)
        .map(|number| bundle.join(format!("pm-{number:04}.dcm")))
        .collect::<Vec<_>>();
    let observation = match result {
        Ok(observation) => observation,
        Err(error) => {
            return vec![failed_probe(
                "pm-concatenation",
                ConversionTarget::Pm,
                &error,
                paths,
            )];
        }
    };
    vec![observe_output(
        "pm-concatenation",
        ConversionTarget::Pm,
        &observation,
        &paths,
        || {
            paths
                .iter()
                .map(|path| {
                    reference
                        .normalize_pm_samples(path)
                        .map_err(|error| error.to_string())
                })
                .collect::<Result<Vec<_>, _>>()
                .map(Value::Array)
        },
        |value| validate_concatenation(value.as_array().ok_or("PM parts must be an array")?),
    )]
}

fn validate_concatenation(parts: &[Value]) -> Result<(), String> {
    if parts.len() != 3 {
        return Err(format!(
            "forced PM concatenation produced {} parts instead of 3",
            parts.len()
        ));
    }
    let uid = parts[0]["concatenation"]["uid"]
        .as_str()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "PM concatenation has no Concatenation UID".to_owned())?;
    let source_uid = parts[0]["concatenation"]["source_sop_instance_uid"]
        .as_str()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "PM concatenation has no notional source SOP UID".to_owned())?;
    let series_uid = parts[0]["series_instance_uid"]
        .as_str()
        .ok_or_else(|| "PM concatenation has no Series Instance UID".to_owned())?;
    let mut finite = 0_u64;
    let mut missing = 0_u64;
    for (index, part) in parts.iter().enumerate() {
        validate_samples(part, Some(index))?;
        let expected_number = u64::try_from(index + 1)
            .map_err(|_| "PM concatenation index does not fit u64".to_owned())?;
        if part["concatenation"]["uid"].as_str() != Some(uid)
            || part["concatenation"]["source_sop_instance_uid"].as_str() != Some(source_uid)
            || part["series_instance_uid"].as_str() != Some(series_uid)
            || part["concatenation"]["number"].as_u64() != Some(expected_number)
            || part["concatenation"]["total"].as_u64() != Some(3)
            || part["concatenation"]["frame_offset"].as_u64()
                != Some(u64::try_from(index).unwrap_or(u64::MAX))
        {
            return Err("PM concatenation identity, numbering, or offsets disagree".to_owned());
        }
        finite = finite
            .checked_add(part["pixel"]["finite_count"].as_u64().unwrap_or(0))
            .ok_or_else(|| "PM finite sample count overflow".to_owned())?;
        missing = missing
            .checked_add(part["pixel"]["missing_count"].as_u64().unwrap_or(0))
            .ok_or_else(|| "PM missing sample count overflow".to_owned())?;
    }
    if finite != 513 || missing != 255 {
        return Err(format!(
            "PM concatenation contains {finite} finite and {missing} missing samples"
        ));
    }
    Ok(())
}

fn validate_samples(normalized: &Value, part: Option<usize>) -> Result<(), String> {
    let matrix = if part.is_some() {
        json!({"columns": 256, "rows": 1, "frames": 1, "total_columns": 513, "total_rows": 1})
    } else {
        json!({"columns": 16, "rows": 16, "frames": 1, "total_columns": 16, "total_rows": 16})
    };
    if normalized["pixel"]["samples"] != json!(expected::pm_samples(part))
        || normalized["pixel"]["precision"].as_str() != Some("float32")
        || normalized["matrix"] != matrix
        || normalized["dimension_organization_type"].as_str() != Some("TILED_FULL")
        || normalized["mappings"][0]["slope"].as_f64() != Some(1.0)
        || normalized["mappings"][0]["intercept"].as_f64() != Some(0.0)
    {
        return Err(
            "independent PM samples, padding, layout, or value mapping differ from input"
                .to_owned(),
        );
    }
    Ok(())
}
