use std::path::Path;

use serde_json::Value;

use super::inputs::ConversionInputs;
use super::{ConversionObservation, ConversionTarget, passed, verify_report_outputs};
use crate::probe::{GeoJsonCoordinateSpace, GeoJsonTarget, ViewerProbe};
use crate::shim::{FixtureSet, ReferenceShim};

pub(super) fn run_direct(
    fixtures: &FixtureSet,
    reference: &ReferenceShim,
    probe: &ViewerProbe,
    output_directory: &Path,
    inputs: &ConversionInputs,
) -> Result<Vec<ConversionObservation>, String> {
    let bundle = output_directory.join("geojson-direct");
    let observation = probe
        .convert_geojson_bundle(
            &fixtures.source,
            None,
            &inputs.mapping,
            GeoJsonCoordinateSpace::Level0Pixels,
            &[GeoJsonTarget::Ann, GeoJsonTarget::Sr],
            &bundle,
            &inputs.direct_geojson,
            false,
        )
        .map_err(|error| error.to_string())?;
    let ann_path = bundle.join("ann.dcm");
    let sr_path = bundle.join("sr.dcm");
    verify_report_outputs(&observation, "ann", std::slice::from_ref(&ann_path))?;
    verify_report_outputs(&observation, "sr", std::slice::from_ref(&sr_path))?;
    let ann = reference
        .normalize_ann(&ann_path, &fixtures.source, None)
        .map_err(|error| error.to_string())?;
    validate_ann(&ann)?;
    let sr = reference
        .normalize_sr(&sr_path)
        .map_err(|error| error.to_string())?;
    validate_direct_sr(&sr)?;
    Ok(vec![
        passed(
            "geojson-ann",
            ConversionTarget::Ann,
            &observation,
            vec![ann_path],
            ann,
        ),
        passed(
            "sr-direct",
            ConversionTarget::Sr,
            &observation,
            vec![sr_path],
            sr,
        ),
    ])
}

pub(super) fn run_seg_reference(
    fixtures: &FixtureSet,
    reference: &ReferenceShim,
    probe: &ViewerProbe,
    output_directory: &Path,
    inputs: &ConversionInputs,
) -> Result<Vec<ConversionObservation>, String> {
    let bundle = output_directory.join("geojson-seg-reference");
    let observation = probe
        .convert_geojson_bundle(
            &fixtures.source,
            None,
            &inputs.mapping,
            GeoJsonCoordinateSpace::Level0Pixels,
            &[GeoJsonTarget::Seg, GeoJsonTarget::Sr],
            &bundle,
            &inputs.seg_geojson,
            false,
        )
        .map_err(|error| error.to_string())?;
    let seg_path = bundle.join("seg.dcm");
    let sr_path = bundle.join("sr.dcm");
    verify_report_outputs(&observation, "seg", std::slice::from_ref(&seg_path))?;
    verify_report_outputs(&observation, "sr", std::slice::from_ref(&sr_path))?;
    let seg = reference
        .normalize_seg(&seg_path, &fixtures.source)
        .map_err(|error| error.to_string())?;
    validate_seg(&seg)?;
    let sr = reference
        .normalize_sr(&sr_path)
        .map_err(|error| error.to_string())?;
    validate_seg_sr(&sr, &seg)?;
    Ok(vec![
        passed(
            "geojson-seg",
            ConversionTarget::Seg,
            &observation,
            vec![seg_path],
            seg,
        ),
        passed(
            "sr-seg-reference",
            ConversionTarget::Sr,
            &observation,
            vec![sr_path],
            sr,
        ),
    ])
}

fn validate_ann(ann: &Value) -> Result<(), String> {
    let groups = array_at(ann, "/groups")?;
    if groups.len() != 1
        || groups[0]["uid"].as_str() != Some("2.25.7101")
        || groups[0]["graphic_type"].as_str() != Some("POLYGON")
        || groups[0]["measurements"].as_array().map(Vec::len) != Some(1)
    {
        return Err("independent ANN normalization differs from the profiled feature".to_owned());
    }
    Ok(())
}

fn validate_direct_sr(sr: &Value) -> Result<(), String> {
    validate_sr_status(sr)?;
    let groups = array_at(sr, "/groups")?;
    let group = groups
        .first()
        .ok_or_else(|| "direct SR contains no measurement group".to_owned())?;
    let graphic_data = array_at(group, "/reference/graphic_data")?;
    if groups.len() != 1
        || group["tracking"]["id"].as_str() != Some("2.25.7101")
        || group["reference"]["kind"].as_str() != Some("coordinates")
        || group["reference"]["graphic_type"].as_str() != Some("POLYGON")
        || graphic_data.first() != graphic_data.last()
        || group["measurements"][0]["value"].as_f64() != Some(25.0)
        || group["qualitative_evaluations"][0]["value"]["value"].as_str() != Some("75540009")
    {
        return Err("independent SR normalization differs from the direct ROI mapping".to_owned());
    }
    Ok(())
}

fn validate_seg(seg: &Value) -> Result<(), String> {
    let segments = array_at(seg, "/segments")?;
    if segments.len() != 1
        || segments[0]["tracking_id"].as_str() != Some("2.25.7102")
        || segments[0]["tracking_uid"].as_str() != Some("2.25.7102")
        || !mask_contains(seg, 1, 2, 2)
        || mask_contains(seg, 1, 5, 5)
    {
        return Err("independent SEG normalization did not preserve the polygon hole".to_owned());
    }
    Ok(())
}

fn validate_seg_sr(sr: &Value, seg: &Value) -> Result<(), String> {
    validate_sr_status(sr)?;
    let groups = array_at(sr, "/groups")?;
    let group = groups
        .first()
        .ok_or_else(|| "SEG-referenced SR contains no measurement group".to_owned())?;
    if groups.len() != 1
        || group["tracking"]["id"].as_str() != Some("2.25.7102")
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

fn mask_contains(seg: &Value, segment: u64, row: u64, column: u64) -> bool {
    seg.pointer("/masks/runs")
        .and_then(Value::as_array)
        .is_some_and(|runs| {
            runs.iter().any(|run| {
                let start = run["column_start"].as_u64().unwrap_or(u64::MAX);
                let length = run["length"].as_u64().unwrap_or(0);
                run["segment_number"].as_u64() == Some(segment)
                    && run["row"].as_u64() == Some(row)
                    && column >= start
                    && column < start.saturating_add(length)
            })
        })
}

fn array_at<'a>(value: &'a Value, pointer: &str) -> Result<&'a Vec<Value>, String> {
    value
        .pointer(pointer)
        .and_then(Value::as_array)
        .ok_or_else(|| format!("independent normalization has no array at {pointer}"))
}
