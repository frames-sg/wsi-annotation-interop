use serde_json::{Value, json};
use sha2::{Digest, Sha256};

const STUDY_UID: &str = "2.25.100000000000000000000000000000001";
const SOURCE_SERIES_UID: &str = "2.25.100000000000000000000000000000002";
const SOURCE_SOP_UID: &str = "2.25.100000000000000000000000000000003";
const FRAME_OF_REFERENCE_UID: &str = "2.25.100000000000000000000000000000004";
const PYRAMID_SOURCE_SERIES_UID: &str = "2.25.100000000000000000000000000000009";
const PYRAMID_SOURCE_SOP_UID: &str = "2.25.100000000000000000000000000000010";
const WSI_SOP_CLASS_UID: &str = "1.2.840.10008.5.1.4.1.1.77.1.6";

type Point = (f64, f64);

#[derive(Clone, Copy)]
enum MaskKind {
    Binary,
    Labelmap,
    Fractional,
}

impl MaskKind {
    const fn label(self) -> &'static str {
        match self {
            Self::Binary => "binary",
            Self::Labelmap => "labelmap",
            Self::Fractional => "fractional",
        }
    }

    const fn digest_tag(self) -> u8 {
        match self {
            Self::Binary => 0,
            Self::Labelmap => 1,
            Self::Fractional => 2,
        }
    }
}

enum MaskRun {
    Binary {
        segment_number: u16,
        row: u32,
        column_start: u32,
        length: u32,
    },
    Fractional {
        segment_number: u16,
        row: u32,
        column_start: u32,
        maximum_fractional_value: u16,
        values: Vec<u16>,
    },
}

impl MaskRun {
    fn as_json(&self) -> Value {
        match self {
            Self::Binary {
                segment_number,
                row,
                column_start,
                length,
            } => json!({
                "segment_number": segment_number,
                "row": row,
                "column_start": column_start,
                "length": length,
            }),
            Self::Fractional {
                segment_number,
                row,
                column_start,
                maximum_fractional_value,
                values,
            } => json!({
                "segment_number": segment_number,
                "row": row,
                "column_start": column_start,
                "maximum_fractional_value": maximum_fractional_value,
                "values": values,
            }),
        }
    }
}

/// Build the deterministic, implementation-independent core study oracle.
#[must_use]
pub fn build_core_ground_truth() -> Value {
    json!({
        "schema_version": 1,
        "cases": {
            "ann-2d-volume": ann_truth("2D_VOLUME", 1, 0),
            "ann-2d-frame": ann_truth("2D_FRAME", 2, 0),
            "ann-3d-common-z": ann_truth("3D_COMMON_Z", 3, 0),
            "ann-3d-xyz": ann_truth("3D_XYZ", 4, 0),
            "ann-2d-volume-level1": ann_truth("2D_VOLUME", 5, 1),
            "seg-binary": seg_truth(MaskKind::Binary, 1),
            "seg-labelmap": seg_truth(MaskKind::Labelmap, 2),
            "seg-fractional": seg_truth(MaskKind::Fractional, 3),
            "seg-binary-reordered": seg_truth(MaskKind::Binary, 4),
        },
    })
}

fn ann_truth(form: &str, index: u8, source_level: u8) -> Value {
    let is_3d = form.starts_with("3D");
    let pixel_origin = if is_3d {
        Value::Null
    } else if form == "2D_FRAME" {
        json!("FRAME")
    } else {
        json!("VOLUME")
    };
    let referenced_frame = (form == "2D_FRAME").then_some(1);
    let groups: Vec<_> = graphics()
        .into_iter()
        .enumerate()
        .map(|(offset, (graphic_type, annotations))| {
            ann_group_truth(offset + 1, graphic_type, &annotations, form, source_level)
        })
        .collect();

    json!({
        "sop_instance_uid": format!("2.25.2100000000000000000000000000000{index}"),
        "series_instance_uid": format!("2.25.2000000000000000000000000000000{index}"),
        "coordinate_type": if is_3d { "3D" } else { "2D" },
        "pixel_origin_interpretation": pixel_origin,
        "referenced_frame_number": referenced_frame,
        "content": {
            "label": format!("ANN{index}"),
            "description": format!("Deterministic {form} annotation fixture"),
            "creator_name": null,
        },
        "source": source_truth(source_level),
        "groups": groups,
    })
}

fn ann_group_truth(
    number: usize,
    graphic_type: &str,
    annotations: &[Vec<Point>; 2],
    form: &str,
    source_level: u8,
) -> Value {
    let native_dimensions = usize::from(form == "3D_XYZ") + 2;
    let canonical_dimensions = if form.starts_with("3D") { 3 } else { 2 };
    let mut native_coordinates = Vec::new();
    let mut canonical_coordinates = Vec::new();
    let mut ordinal = 0_u32;
    for annotation in annotations {
        for &(x, y) in annotation {
            if form.starts_with("3D") {
                let z = if form == "3D_COMMON_Z" {
                    0.01
                } else {
                    0.01 + f64::from(ordinal) * 0.0001
                };
                native_coordinates.extend([x * 0.001, y * 0.001]);
                if form == "3D_XYZ" {
                    native_coordinates.push(z);
                }
                canonical_coordinates.extend([x, y, z]);
                ordinal += 1;
            } else {
                native_coordinates.extend([x, y]);
                let scale = if source_level == 1 { 2.0 } else { 1.0 };
                canonical_coordinates.extend([x * scale, y * scale]);
            }
        }
    }
    let primitive_point_indices = if matches!(graphic_type, "POLYLINE" | "POLYGON") {
        vec![1, 1 + annotations[0].len() * native_dimensions]
    } else {
        Vec::new()
    };

    json!({
        "uid": format!("2.25.3000000000000000000000000000000{number}"),
        "label": format!("{graphic_type} GROUP"),
        "description": format!("Two deterministic {graphic_type} annotations"),
        "generation_type": "AUTOMATIC",
        "algorithms": [algorithm("Synthetic annotation algorithm", Some("threshold=0.5"))],
        "category": code("MORPH", "99WSI", "Morphologically abnormal structure", Some("1"), "short"),
        "property_type": code("TUMOR", "99WSI", "Tumor", None, "short"),
        "property_type_modifiers": [code("urn:wsi-interop:viable", "99WSI", "Viable", None, "urn")],
        "anatomic_regions": [code("LUNG", "99WSI", "Lung", None, "short")],
        "primary_anatomic_structures": [code("BRONCHUS", "99WSI", "Bronchus", None, "short")],
        "applies_to_all_optical_paths": false,
        "referenced_optical_paths": ["1"],
        "applies_to_all_z_planes": !form.starts_with("3D"),
        "common_z_coordinates_mm": if form == "3D_COMMON_Z" { vec![0.01] } else { Vec::new() },
        "recommended_display_cielab": [39321, 38036, 35466],
        "graphic_type": graphic_type,
        "annotation_count": 2,
        "measurements": [{
            "concept": code("AREA", "99WSI", "Synthetic area", None, "short"),
            "units": code("mm2", "UCUM", "square millimeter", None, "short"),
            "values": [1.0],
            "annotation_indices": [1],
        }],
        "geometry": {
            "mode": "Full",
            "native_dimensions": native_dimensions,
            "canonical_dimensions": canonical_dimensions,
            "native_coordinates": native_coordinates,
            "canonical_level0_coordinates": canonical_coordinates,
            "primitive_point_indices": primitive_point_indices,
        },
    })
}

fn seg_truth(kind: MaskKind, index: u8) -> Value {
    let mut segments = vec![segment_truth(1)];
    if !matches!(kind, MaskKind::Fractional) {
        segments.push(segment_truth(2));
    }
    if matches!(kind, MaskKind::Labelmap) {
        segments.insert(0, background_segment_truth());
    }
    let runs = match kind {
        MaskKind::Binary => binary_runs(),
        MaskKind::Labelmap => labelmap_runs(),
        MaskKind::Fractional => fractional_runs(),
    };
    let mode = if matches!(kind, MaskKind::Fractional) {
        "FullFractional"
    } else {
        "FullBinary"
    };

    json!({
        "sop_instance_uid": format!("2.25.5100000000000000000000000000000{index}"),
        "series_instance_uid": format!("2.25.5000000000000000000000000000000{index}"),
        "segmentation_kind": kind.label(),
        "content": {
            "label": format!("SEG{index}"),
            "description": format!("Deterministic {} segmentation fixture", kind.label().to_uppercase()),
            "creator_name": null,
        },
        "source": source_truth(0),
        "segments": segments,
        "masks": {
            "mode": mode,
            "sha256": mask_digest(kind, &runs),
            "runs": runs.iter().map(MaskRun::as_json).collect::<Vec<_>>(),
        },
    })
}

fn segment_truth(number: u8) -> Value {
    json!({
        "number": number,
        "label": format!("Segment {number}"),
        "description": "",
        "generation_type": "AUTOMATIC",
        "algorithms": [algorithm("Synthetic segmentation algorithm", None)],
        "category": code("MORPH", "99WSI", "Morphology", None, "short"),
        "property_type": code(&format!("TUMOR{number}"), "99WSI", &format!("Tumor {number}"), None, "short"),
        "property_type_modifiers": [],
        "tracking_id": format!("lesion-{number}"),
        "tracking_uid": format!("2.25.4000000000000000000000000000000{number}"),
        "anatomic_regions": [code("LUNG", "99WSI", "Lung", None, "short")],
        "primary_anatomic_structures": [],
        "recommended_display_cielab": [if number == 1 { 33423 } else { 34078 }, 35466, 34181],
    })
}

fn background_segment_truth() -> Value {
    let background = code("125040", "DCM", "Background", None, "short");
    json!({
        "number": 0,
        "label": "Background",
        "description": "",
        "generation_type": "AUTOMATIC",
        "algorithms": [],
        "category": background,
        "property_type": background,
        "property_type_modifiers": [],
        "tracking_id": null,
        "tracking_uid": null,
        "anatomic_regions": [],
        "primary_anatomic_structures": [],
        "recommended_display_cielab": [0, 32896, 32896],
    })
}

fn source_truth(level: u8) -> Value {
    let canonical = level == 0;
    json!({
        "sop_class_uid": WSI_SOP_CLASS_UID,
        "sop_instance_uid": if canonical { SOURCE_SOP_UID } else { PYRAMID_SOURCE_SOP_UID },
        "series_instance_uid": if canonical { SOURCE_SERIES_UID } else { PYRAMID_SOURCE_SERIES_UID },
        "study_instance_uid": STUDY_UID,
        "frame_of_reference_uid": FRAME_OF_REFERENCE_UID,
        "total_pixel_matrix_columns": if canonical { 16 } else { 8 },
        "total_pixel_matrix_rows": if canonical { 16 } else { 8 },
        "tile_columns": 4,
        "tile_rows": 4,
        "pixel_spacing": if canonical { vec![0.001, 0.001] } else { vec![0.002, 0.002] },
        "canonical_total_pixel_matrix_columns": 16,
        "canonical_total_pixel_matrix_rows": 16,
        "canonical_pixel_spacing": [0.001, 0.001],
    })
}

fn code(
    value: &str,
    scheme: &str,
    meaning: &str,
    coding_scheme_version: Option<&str>,
    value_kind: &str,
) -> Value {
    json!({
        "value": value,
        "value_kind": value_kind,
        "scheme": scheme,
        "coding_scheme_version": coding_scheme_version,
        "meaning": meaning,
        "context_identifier": null,
        "context_uid": null,
        "mapping_resource": null,
        "mapping_resource_uid": null,
        "context_group_version": null,
        "context_group_local_version": null,
        "context_group_extension": null,
        "context_group_extension_creator_uid": null,
    })
}

fn algorithm(name: &str, parameters: Option<&str>) -> Value {
    json!({
        "family": code("AI", "99WSI", "Artificial intelligence", None, "short"),
        "name_code": null,
        "name": name,
        "version": "1.0.0",
        "parameters": parameters,
        "source": "wsi-annotation-interop",
    })
}

fn graphics() -> [(&'static str, [Vec<Point>; 2]); 5] {
    [
        ("POINT", [vec![(0.5, 0.5)], vec![(2.5, 2.5)]]),
        (
            "POLYLINE",
            [
                vec![(0.5, 0.5), (1.5, 0.75), (2.5, 1.0)],
                vec![(0.5, 2.5), (1.5, 2.0), (2.5, 2.5)],
            ],
        ),
        (
            "POLYGON",
            [
                vec![(0.5, 0.5), (1.5, 0.5), (1.5, 1.5), (0.5, 1.5)],
                vec![(2.0, 2.0), (3.0, 2.0), (3.0, 3.0), (2.0, 3.0)],
            ],
        ),
        (
            "ELLIPSE",
            [
                vec![(0.5, 1.0), (2.5, 1.0), (1.5, 0.5), (1.5, 1.5)],
                vec![(1.0, 2.5), (3.0, 2.5), (2.0, 2.0), (2.0, 3.0)],
            ],
        ),
        (
            "RECTANGLE",
            [
                vec![(0.5, 0.5), (1.5, 0.5), (1.5, 1.5), (0.5, 1.5)],
                vec![(2.0, 2.0), (3.0, 2.0), (3.0, 3.0), (2.0, 3.0)],
            ],
        ),
    ]
}

fn binary_runs() -> Vec<MaskRun> {
    let mut planes = [[[false; 16]; 16]; 2];
    fill(&mut planes[0], 1..9, 1..9, true);
    fill(&mut planes[0], 4..6, 4..6, false);
    fill(&mut planes[0], 12..14, 1..3, true);
    fill(&mut planes[1], 6..14, 6..14, true);
    fill(&mut planes[1], 9..11, 9..11, false);
    planes
        .iter()
        .enumerate()
        .flat_map(|(index, plane)| plane_runs(plane, u16::try_from(index + 1).unwrap()))
        .collect()
}

fn labelmap_runs() -> Vec<MaskRun> {
    let mut plane = [[0_u8; 16]; 16];
    fill(&mut plane, 1..7, 1..7, 1);
    fill(&mut plane, 9..15, 9..15, 2);
    (1..=2)
        .flat_map(|segment_number| {
            let mask = plane.map(|row| row.map(|value| value == segment_number));
            plane_runs(&mask, u16::from(segment_number))
        })
        .collect()
}

fn fractional_runs() -> Vec<MaskRun> {
    let mut plane = [[0_u16; 16]; 16];
    fill(&mut plane, 1..7, 1..7, 64);
    fill(&mut plane, 9..15, 9..15, 191);
    let mut runs = Vec::new();
    for (row, values) in plane.iter().enumerate() {
        let mut column = 0;
        while column < values.len() {
            if values[column] == 0 {
                column += 1;
                continue;
            }
            let start = column;
            while column < values.len() && values[column] != 0 {
                column += 1;
            }
            runs.push(MaskRun::Fractional {
                segment_number: 1,
                row: u32::try_from(row).unwrap(),
                column_start: u32::try_from(start).unwrap(),
                maximum_fractional_value: 255,
                values: values[start..column].to_vec(),
            });
        }
    }
    runs
}

fn plane_runs(plane: &[[bool; 16]; 16], segment_number: u16) -> Vec<MaskRun> {
    let mut runs = Vec::new();
    for (row, values) in plane.iter().enumerate() {
        let mut column = 0;
        while column < values.len() {
            if !values[column] {
                column += 1;
                continue;
            }
            let start = column;
            while column < values.len() && values[column] {
                column += 1;
            }
            runs.push(MaskRun::Binary {
                segment_number,
                row: u32::try_from(row).unwrap(),
                column_start: u32::try_from(start).unwrap(),
                length: u32::try_from(column - start).unwrap(),
            });
        }
    }
    runs
}

fn mask_digest(kind: MaskKind, runs: &[MaskRun]) -> String {
    let mut digest = Sha256::new();
    digest.update(b"dicom-viewer-seg-runs-v1\0");
    digest.update(16_u32.to_le_bytes());
    digest.update(16_u32.to_le_bytes());
    digest.update([kind.digest_tag()]);
    for run in runs {
        match run {
            MaskRun::Binary {
                segment_number,
                row,
                column_start,
                length,
            } => {
                digest.update(segment_number.to_le_bytes());
                digest.update(row.to_le_bytes());
                digest.update(column_start.to_le_bytes());
                digest.update(length.to_le_bytes());
            }
            MaskRun::Fractional {
                segment_number,
                row,
                column_start,
                maximum_fractional_value,
                values,
            } => {
                digest.update(segment_number.to_le_bytes());
                digest.update(row.to_le_bytes());
                digest.update(column_start.to_le_bytes());
                digest.update(maximum_fractional_value.to_le_bytes());
                digest.update(u32::try_from(values.len()).unwrap().to_le_bytes());
                for value in values {
                    digest.update(value.to_le_bytes());
                }
            }
        }
    }
    format!("{:x}", digest.finalize())
}

fn fill<T: Copy>(
    plane: &mut [[T; 16]; 16],
    rows: std::ops::Range<usize>,
    columns: std::ops::Range<usize>,
    value: T,
) {
    for row in rows {
        for column in columns.clone() {
            plane[row][column] = value;
        }
    }
}
