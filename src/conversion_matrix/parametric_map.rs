use std::path::Path;

use serde_json::Value;

use super::inputs::ConversionInputs;
use super::{ConversionObservation, passed, verify_report_outputs};
use crate::probe::{RasterChannels, ViewerProbe};
use crate::shim::{FixtureSet, ReferenceShim};

const FORCED_CONCATENATION_BYTES: u64 = 5_200;

pub(super) fn run_single(
    fixtures: &FixtureSet,
    reference: &ReferenceShim,
    probe: &ViewerProbe,
    output_directory: &Path,
    inputs: &ConversionInputs,
) -> Result<Vec<ConversionObservation>, String> {
    let bundle = output_directory.join("pm-float32");
    let observation = probe
        .convert_raster_bundle(
            &fixtures.source,
            None,
            &inputs.raster_profile,
            RasterChannels::Auto,
            &bundle,
            None,
            &inputs.raster,
        )
        .map_err(|error| error.to_string())?;
    let path = bundle.join("pm-0001.dcm");
    verify_report_outputs(&observation, "pm", std::slice::from_ref(&path))?;
    let normalized = reference
        .normalize_pm(&path)
        .map_err(|error| error.to_string())?;
    if normalized["pixel"]["precision"].as_str() != Some("float32")
        || normalized["pixel"]["finite_count"].as_u64() != Some(255)
        || normalized["pixel"]["missing_count"].as_u64() != Some(1)
        || normalized["mappings"][0]["quantity"]["value"].as_str() != Some("TUMOR")
        || normalized["source_sop_instance_uids"][0].as_str()
            != Some("2.25.100000000000000000000000000000003")
    {
        return Err("independent PM normalization differs from the f32 profile".to_owned());
    }
    Ok(vec![passed(
        "pm",
        "pm-float32",
        "pm",
        &observation,
        vec![path],
        normalized,
    )])
}

pub(super) fn run_concatenation(
    fixtures: &FixtureSet,
    reference: &ReferenceShim,
    probe: &ViewerProbe,
    output_directory: &Path,
    inputs: &ConversionInputs,
) -> Result<Vec<ConversionObservation>, String> {
    let bundle = output_directory.join("pm-concatenation");
    let observation = probe
        .convert_raster_bundle(
            &fixtures.source,
            None,
            &inputs.concatenation_profile,
            RasterChannels::Auto,
            &bundle,
            Some(FORCED_CONCATENATION_BYTES),
            &inputs.wide_raster,
        )
        .map_err(|error| error.to_string())?;
    let output_count = observation.report["outputs"]
        .as_array()
        .map(Vec::len)
        .ok_or_else(|| "PM concatenation report outputs must be an array".to_owned())?;
    let paths = (1..=output_count)
        .map(|number| bundle.join(format!("pm-{number:04}.dcm")))
        .collect::<Vec<_>>();
    verify_report_outputs(&observation, "pm", &paths)?;
    let normalized = paths
        .iter()
        .map(|path| {
            reference
                .normalize_pm(path)
                .map_err(|error| error.to_string())
        })
        .collect::<Result<Vec<_>, _>>()?;
    validate_concatenation(&normalized)?;
    Ok(vec![passed(
        "pm",
        "pm-concatenation",
        "pm",
        &observation,
        paths,
        Value::Array(normalized),
    )])
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
