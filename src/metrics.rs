use serde::{Deserialize, Serialize};

mod crop;
mod distance;
mod runs;

use crop::advanced_metrics;
use runs::{area, canonicalize, centroid, intersection_area, overlap_pixels};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BinaryRun {
    pub segment_number: u16,
    pub row: usize,
    pub column_start: usize,
    pub length: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MetricLimits {
    pub max_crop_pixels: usize,
}

impl Default for MetricLimits {
    fn default() -> Self {
        Self {
            max_crop_pixels: 4_000_000,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum MetricComputationStatus {
    Available,
    ResourceLimited {
        required_crop_pixels: usize,
        limit_pixels: usize,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SegmentationMetrics {
    pub dice: f64,
    pub expected_area_pixels: usize,
    pub actual_area_pixels: usize,
    pub area_difference_pixels: usize,
    pub relative_area_difference: f64,
    pub area_difference_mm2: f64,
    pub centroid_distance_pixels: f64,
    pub centroid_distance_um: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hd95_pixels: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hd95_um: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub assd_pixels: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub assd_um: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expected_components: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actual_components: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expected_holes: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actual_holes: Option<usize>,
    pub advanced_metrics: MetricComputationStatus,
    pub expected_overlap_pixels: usize,
    pub actual_overlap_pixels: usize,
    pub overlap_difference_pixels: usize,
    pub overlap_difference_mm2: f64,
}

/// Calculate identity-aware binary segmentation metrics from normalized row runs.
///
/// Core metrics allocate in proportion to canonical run count. Exact topology and surface metrics
/// use bounded occupied crops; their fields are absent and `advanced_metrics` is
/// `resource_limited` when the configured crop budget would be exceeded.
///
/// Same-segment runs are sorted and their overlapping or adjacent intervals are merged. This makes
/// duplicate input deterministic and prevents duplicate pixels from inflating area.
///
/// # Errors
///
/// Returns an error when dimensions or spacing are invalid, arithmetic overflows, or a run lies
/// outside the declared mask.
pub fn segmentation_metrics(
    expected_runs: &[BinaryRun],
    actual_runs: &[BinaryRun],
    shape: (usize, usize),
    pixel_spacing_mm: (f64, f64),
) -> Result<SegmentationMetrics, String> {
    segmentation_metrics_with_limits(
        expected_runs,
        actual_runs,
        shape,
        pixel_spacing_mm,
        MetricLimits::default(),
    )
}

/// Calculate segmentation metrics with an explicit maximum occupied-crop cell count.
///
/// # Errors
///
/// Returns an error for invalid input or checked-arithmetic failures. A crop over the limit is a
/// successful result with explicit resource-limited advanced metrics.
pub fn segmentation_metrics_with_limits(
    expected_runs: &[BinaryRun],
    actual_runs: &[BinaryRun],
    shape: (usize, usize),
    pixel_spacing_mm: (f64, f64),
    limits: MetricLimits,
) -> Result<SegmentationMetrics, String> {
    validate_configuration(shape, pixel_spacing_mm, limits)?;
    let expected = canonicalize(expected_runs, shape)?;
    let actual = canonicalize(actual_runs, shape)?;
    let expected_area = area(&expected)?;
    let actual_area = area(&actual)?;
    let intersection = intersection_area(&expected, &actual)?;
    let total_area = expected_area
        .checked_add(actual_area)
        .ok_or_else(|| "combined mask area overflows usize".to_owned())?;
    let dice = if total_area == 0 {
        1.0
    } else {
        2.0 * usize_as_f64(intersection) / usize_as_f64(total_area)
    };
    let (centroid_pixels, centroid_um) =
        centroid_distances(centroid(&expected), centroid(&actual), pixel_spacing_mm);
    let expected_overlap = overlap_pixels(&expected)?;
    let actual_overlap = overlap_pixels(&actual)?;
    let area_difference = expected_area.abs_diff(actual_area);
    let overlap_difference = expected_overlap.abs_diff(actual_overlap);
    let pixel_area_mm2 = pixel_spacing_mm.0 * pixel_spacing_mm.1;

    let advanced = advanced_metrics(&expected, &actual, pixel_spacing_mm, limits.max_crop_pixels);
    let (
        advanced_metrics,
        hd95_pixels,
        hd95_um,
        assd_pixels,
        assd_um,
        expected_components,
        actual_components,
        expected_holes,
        actual_holes,
    ) = match advanced {
        Ok(value) => (
            MetricComputationStatus::Available,
            Some(value.hd95_pixels),
            Some(value.hd95_mm * 1000.0),
            Some(value.assd_pixels),
            Some(value.assd_mm * 1000.0),
            Some(value.expected_components),
            Some(value.actual_components),
            Some(value.expected_holes),
            Some(value.actual_holes),
        ),
        Err(required_crop_pixels) => (
            MetricComputationStatus::ResourceLimited {
                required_crop_pixels,
                limit_pixels: limits.max_crop_pixels,
            },
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        ),
    };

    Ok(SegmentationMetrics {
        dice,
        expected_area_pixels: expected_area,
        actual_area_pixels: actual_area,
        area_difference_pixels: area_difference,
        relative_area_difference: if expected_area == 0 && actual_area == 0 {
            0.0
        } else if expected_area == 0 {
            f64::INFINITY
        } else {
            usize_as_f64(area_difference) / usize_as_f64(expected_area)
        },
        area_difference_mm2: usize_as_f64(area_difference) * pixel_area_mm2,
        centroid_distance_pixels: centroid_pixels,
        centroid_distance_um: centroid_um,
        hd95_pixels,
        hd95_um,
        assd_pixels,
        assd_um,
        expected_components,
        actual_components,
        expected_holes,
        actual_holes,
        advanced_metrics,
        expected_overlap_pixels: expected_overlap,
        actual_overlap_pixels: actual_overlap,
        overlap_difference_pixels: overlap_difference,
        overlap_difference_mm2: usize_as_f64(overlap_difference) * pixel_area_mm2,
    })
}

fn validate_configuration(
    shape: (usize, usize),
    spacing: (f64, f64),
    limits: MetricLimits,
) -> Result<(), String> {
    if shape.0 == 0 || shape.1 == 0 {
        return Err("mask shape must be positive".to_owned());
    }
    if !spacing.0.is_finite() || !spacing.1.is_finite() || spacing.0 <= 0.0 || spacing.1 <= 0.0 {
        return Err("pixel spacing must be positive and finite".to_owned());
    }
    if limits.max_crop_pixels == 0 {
        return Err("metric crop limit must be positive".to_owned());
    }
    Ok(())
}

fn centroid_distances(
    expected: Option<(f64, f64)>,
    actual: Option<(f64, f64)>,
    spacing: (f64, f64),
) -> (f64, f64) {
    match (expected, actual) {
        (None, None) => (0.0, 0.0),
        (Some(_), None) | (None, Some(_)) => (f64::INFINITY, f64::INFINITY),
        (Some(expected), Some(actual)) => {
            let row_delta = actual.0 - expected.0;
            let column_delta = actual.1 - expected.1;
            (
                row_delta.hypot(column_delta),
                (row_delta * spacing.0).hypot(column_delta * spacing.1) * 1000.0,
            )
        }
    }
}

#[allow(clippy::cast_precision_loss)]
pub(crate) fn usize_as_f64(value: usize) -> f64 {
    value as f64
}
