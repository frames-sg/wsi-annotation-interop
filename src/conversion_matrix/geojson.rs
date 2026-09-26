use std::path::Path;

use super::expected;
use crate::compare::{compare_ann, compare_seg};
use serde_json::Value;

use super::inputs::ConversionInputs;
use super::observation::{failed_probe, observe_output};
use super::{ConversionObservation, ConversionTarget};
use crate::probe::{GeoJsonCoordinateSpace, GeoJsonTarget, ViewerProbe};
use crate::shim::{FixtureSet, ReferenceShim};

pub(super) fn run_direct(
    fixtures: &FixtureSet,
    reference: &ReferenceShim,
    probe: &ViewerProbe,
    output_directory: &Path,
    inputs: &ConversionInputs,
) -> Vec<ConversionObservation> {
    let bundle = output_directory.join("geojson-direct");
    let result = probe.convert_geojson_bundle(
        &fixtures.source,
        None,
        &inputs.mapping,
        GeoJsonCoordinateSpace::Level0Pixels,
        &[GeoJsonTarget::Ann, GeoJsonTarget::Sr],
        &bundle,
        &inputs.direct_geojson,
        false,
    );
    let ann_path = bundle.join("ann.dcm");
    let sr_path = bundle.join("sr.dcm");
    let sr_case = "sr-direct";
    let observation = match result {
        Ok(observation) => observation,
        Err(error) => {
            return vec![
                failed_probe("geojson-ann", ConversionTarget::Ann, &error, vec![ann_path]),
                failed_probe(sr_case, ConversionTarget::Sr, &error, vec![sr_path]),
            ];
        }
    };
    let ann = observe_output(
        "geojson-ann",
        ConversionTarget::Ann,
        &observation,
        std::slice::from_ref(&ann_path),
        || {
            reference
                .normalize_ann(&ann_path, &fixtures.source, None)
                .map_err(|error| error.to_string())
        },
        validate_ann,
    );
    let sr = observe_output(
        sr_case,
        ConversionTarget::Sr,
        &observation,
        std::slice::from_ref(&sr_path),
        || {
            reference
                .normalize_sr(&sr_path)
                .map_err(|error| error.to_string())
        },
        validate_direct_sr,
    );
    vec![ann, sr]
}

pub(super) fn run_seg_reference(
    fixtures: &FixtureSet,
    reference: &ReferenceShim,
    probe: &ViewerProbe,
    output_directory: &Path,
    inputs: &ConversionInputs,
) -> Vec<ConversionObservation> {
    let bundle = output_directory.join("geojson-seg-reference");
    let result = probe.convert_geojson_bundle(
        &fixtures.source,
        None,
        &inputs.mapping,
        GeoJsonCoordinateSpace::Level0Pixels,
        &[GeoJsonTarget::Seg, GeoJsonTarget::Sr],
        &bundle,
        &inputs.seg_geojson,
        false,
    );
    let seg_path = bundle.join("seg.dcm");
    let sr_path = bundle.join("sr.dcm");
    let sr_case = "sr-seg-reference";
    let observation = match result {
        Ok(observation) => observation,
        Err(error) => {
            return vec![
                failed_probe("geojson-seg", ConversionTarget::Seg, &error, vec![seg_path]),
                failed_probe(sr_case, ConversionTarget::Sr, &error, vec![sr_path]),
            ];
        }
    };
    let seg = observe_output(
        "geojson-seg",
        ConversionTarget::Seg,
        &observation,
        std::slice::from_ref(&seg_path),
        || {
            reference
                .normalize_seg(&seg_path, &fixtures.source)
                .map_err(|error| error.to_string())
        },
        validate_seg,
    );
    let sr = observe_output(
        sr_case,
        ConversionTarget::Sr,
        &observation,
        std::slice::from_ref(&sr_path),
        || {
            reference
                .normalize_sr(&sr_path)
                .map_err(|error| error.to_string())
        },
        |sr| validate_seg_sr(sr, &seg.normalized),
    );
    vec![seg, sr]
}

fn validate_ann(ann: &Value) -> Result<(), String> {
    let mut semantic = ann.clone();
    // Producer-assigned series identity is intentionally variable; source identity is compared.
    semantic
        .as_object_mut()
        .ok_or("ANN normalization must be an object")?
        .remove("series_instance_uid");
    let comparison = compare_ann(&expected::ann(), &semantic, 1e-6, 1e-9)?;
    if comparison.is_ok() {
        Ok(())
    } else {
        Err(format!(
            "independent ANN semantic mismatch: {:?}",
            comparison.findings
        ))
    }
}

fn validate_direct_sr(sr: &Value) -> Result<(), String> {
    validate_sr_status(sr)?;
    let groups = array_at(sr, "/groups")?;
    let group = groups
        .first()
        .ok_or_else(|| "direct SR contains no measurement group".to_owned())?;
    let graphic_data = array_at(group, "/reference/graphic_data")?;
    let expected_points = [
        [0.001, 0.001, 0.0],
        [0.006, 0.001, 0.0],
        [0.006, 0.006, 0.0],
        [0.001, 0.006, 0.0],
        [0.001, 0.001, 0.0],
    ];
    let geometry_equal = graphic_data.len() == expected_points.len()
        && graphic_data
            .iter()
            .zip(expected_points)
            .all(|(point, expected)| {
                point.as_array().is_some_and(|point| {
                    point.len() == 3
                        && point.iter().zip(expected).all(|(value, expected)| {
                            value.as_f64().is_some_and(|value| {
                                value.is_finite() && (value - expected).abs() <= 1e-9
                            })
                        })
                })
            });
    if groups.len() != 1
        || group["tracking"]["id"].as_str() != Some("2.25.7101")
        || group["reference"]["kind"].as_str() != Some("coordinates")
        || group["reference"]["graphic_type"].as_str() != Some("POLYGON")
        || !geometry_equal
        || group["reference"]["frame_of_reference_uid"].as_str()
            != Some("2.25.100000000000000000000000000000004")
        || group["tracking"]["uid"] != group["tracking"]["id"]
        || group["measurements"] != expected::sr_measurements(25.0)
        || group["qualitative_evaluations"] != expected::qualitative()
        || group["finding_category"] != expected::category()
        || group["finding_type"] != expected::property_type()
        || group["measurements"][0]["value"].as_f64() != Some(25.0)
        || group["qualitative_evaluations"][0]["value"]["value"].as_str() != Some("75540009")
    {
        return Err("independent SR normalization differs from the direct ROI mapping".to_owned());
    }
    Ok(())
}

fn validate_seg(seg: &Value) -> Result<(), String> {
    let mut semantic = seg.clone();
    semantic
        .as_object_mut()
        .ok_or("SEG normalization must be an object")?
        .remove("series_instance_uid");
    // Compare the full mask itself; preserve the reader's digest in the retained observation.
    if let Some(masks) = semantic.get_mut("masks").and_then(Value::as_object_mut) {
        masks.remove("sha256");
    }
    let comparison = compare_seg(&expected::seg(), &semantic)?;
    if comparison.is_ok() {
        Ok(())
    } else {
        Err(format!(
            "independent SEG semantic mismatch: {:?}",
            comparison.findings
        ))
    }
}

fn validate_seg_sr(sr: &Value, seg: &Value) -> Result<(), String> {
    validate_sr_status(sr)?;
    let groups = array_at(sr, "/groups")?;
    let group = groups
        .first()
        .ok_or_else(|| "SEG-referenced SR contains no measurement group".to_owned())?;
    if groups.len() != 1
        || group["tracking"]["id"].as_str() != Some("2.25.7102")
        || group["tracking"]["uid"] != group["tracking"]["id"]
        || group["measurements"] != expected::sr_measurements(104.0)
        || group["qualitative_evaluations"] != expected::qualitative()
        || group["finding_category"] != expected::category()
        || group["finding_type"] != expected::property_type()
        || group["reference"]["kind"].as_str() != Some("segmentation")
        || group["reference"]["sop_instance_uid"] != seg["sop_instance_uid"]
        || group["reference"]["segment_numbers"][0].as_u64() != Some(1)
        || group["reference"]["frame_numbers"]
            .as_array()
            .is_none_or(Vec::is_empty)
    {
        return Err("independent SR normalization differs from the SEG reference".to_owned());
    }
    Ok(())
}

fn validate_sr_status(sr: &Value) -> Result<(), String> {
    if sr["template_id"].as_str() == Some("1500")
        && sr["status"]["completion"].as_str() == Some("COMPLETE")
        && sr["status"]["verification"].as_str() == Some("UNVERIFIED")
        && sr["status"]["preliminary"].as_str() == Some("PRELIMINARY")
    {
        Ok(())
    } else {
        Err("SR status or TID 1500 root is incorrect".to_owned())
    }
}

fn array_at<'a>(value: &'a Value, pointer: &str) -> Result<&'a Vec<Value>, String> {
    value
        .pointer(pointer)
        .and_then(Value::as_array)
        .ok_or_else(|| format!("independent normalization has no array at {pointer}"))
}
