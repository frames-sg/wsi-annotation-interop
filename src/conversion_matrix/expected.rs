//! Expectations for the checked-in conversion examples, independent of produced DICOM.

use serde_json::{Value, json};

use crate::ground_truth::{code, source_truth};

pub(super) fn ann() -> Value {
    json!({
        "source": source_truth(0),
        "coordinate_type": "2D", "pixel_origin_interpretation": "VOLUME",
        "referenced_frame_number": null,
        "content": {"label": "WSI_ANNOTATION", "description": "WSI vector annotations", "creator_name": null},
        "groups": [{
            "uid": "2.25.7101", "label": "Viable tumor", "description": "",
            "generation_type": "AUTOMATIC", "algorithms": [algorithm()],
            "category": category(), "property_type": property_type(),
            "property_type_modifiers": [], "anatomic_regions": [], "primary_anatomic_structures": [],
            "applies_to_all_optical_paths": true, "referenced_optical_paths": [],
            "applies_to_all_z_planes": true, "common_z_coordinates_mm": [],
            "recommended_display_cielab": [40000, 30000, 20000],
            "graphic_type": "POLYGON", "annotation_count": 1,
            "measurements": [{"concept": area(), "units": area_unit(), "values": [25.0], "annotation_indices": null}],
            "geometry": {
                "mode": "Full", "native_dimensions": 2, "canonical_dimensions": 2,
                "native_coordinates": [1.0,1.0,6.0,1.0,6.0,6.0,1.0,6.0],
                "canonical_level0_coordinates": [1.0,1.0,6.0,1.0,6.0,6.0,1.0,6.0],
                "primitive_point_indices": [1]
            }
        }]
    })
}

pub(super) fn seg() -> Value {
    // The example is the half-open rectangle [1,13) × [1,11), less [4,8) × [4,8).
    let mut runs = Vec::new();
    for row in 1..11 {
        let intervals: &[(u32, u32)] = if (4..8).contains(&row) {
            &[(1, 3), (8, 5)]
        } else {
            &[(1, 12)]
        };
        for &(column_start, length) in intervals {
            runs.push(json!({"segment_number": 1, "row": row, "column_start": column_start, "length": length}));
        }
    }
    json!({
        "source": source_truth(0), "segmentation_kind": "binary",
        "content": {"label": "WSI_SEGMENTATION", "description": "WSI binary segmentation", "creator_name": null},
        "segments": [{
            "number": 1, "label": "Viable tumor", "description": "Viable tumor",
            "generation_type": "AUTOMATIC", "algorithms": [algorithm()],
            "category": category(), "property_type": property_type(), "property_type_modifiers": [],
            "tracking_id": "2.25.7102", "tracking_uid": "2.25.7102",
            "anatomic_regions": [], "primary_anatomic_structures": [],
            "recommended_display_cielab": [40000, 30000, 20000]
        }],
        "masks": {"mode": "FullBinary", "runs": runs}
    })
}

pub(super) fn sr_measurements(area_value: f64) -> Value {
    json!([{"concept": area(), "unit": area_unit(), "value": area_value, "coordinates": []}])
}

pub(super) fn qualitative() -> Value {
    json!([{
        "concept": code("258244004", "SCT", "Tumor grade", None, "short"),
        "value": code("75540009", "SCT", "High grade", None, "short")
    }])
}

pub(super) fn category() -> Value {
    code(
        "M-01000",
        "SRT",
        "Morphologically Altered Structure",
        None,
        "short",
    )
}

pub(super) fn property_type() -> Value {
    code("108369006", "SCT", "Neoplasm", None, "short")
}

fn area() -> Value {
    code("42798000", "SCT", "Area", None, "short")
}
fn area_unit() -> Value {
    code("mm2", "UCUM", "square millimeter", None, "short")
}

fn algorithm() -> Value {
    json!({
        "family": code("123110", "DCM", "Artificial Intelligence", None, "short"),
        "name": "Example pathology model", "version": "1.0",
        "name_code": null, "parameters": null, "source": null
    })
}

pub(super) fn pm_samples(part: Option<usize>) -> Vec<Option<f64>> {
    (0_u16..256)
        .map(|index| match part {
            None if index == 85 => None,
            None => Some(f64::from(f32::from(index) / 255.0)),
            Some(part) => {
                let ordinal = part * 256 + usize::from(index);
                // Only the three declared small fixture parts call this function.
                u16::try_from(ordinal)
                    .ok()
                    .filter(|&value| value <= 512)
                    .map(|value| f64::from(f32::from(value) / 512.0))
            }
        })
        .collect()
}
