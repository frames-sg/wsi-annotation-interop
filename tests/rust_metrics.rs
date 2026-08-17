use wsi_annotation_interop::metrics::{BinaryRun, segmentation_metrics};

fn run(segment_number: u16, row: usize, column_start: usize, length: usize) -> BinaryRun {
    BinaryRun {
        segment_number,
        row,
        column_start,
        length,
    }
}

fn runs_with_hole() -> Vec<BinaryRun> {
    vec![
        run(1, 0, 0, 5),
        run(1, 1, 0, 5),
        run(1, 2, 0, 2),
        run(1, 2, 3, 2),
        run(1, 3, 0, 5),
        run(1, 4, 0, 5),
    ]
}

fn close(left: f64, right: f64) {
    assert!((left - right).abs() < 1e-12, "{left} != {right}");
}

#[test]
fn metrics_are_exact_for_identical_masks_with_a_hole() {
    let metrics =
        segmentation_metrics(&runs_with_hole(), &runs_with_hole(), (5, 5), (0.5, 0.25)).unwrap();

    close(metrics.dice, 1.0);
    assert_eq!(metrics.area_difference_pixels, 0);
    close(metrics.centroid_distance_pixels, 0.0);
    close(metrics.hd95_pixels, 0.0);
    close(metrics.assd_pixels, 0.0);
    assert_eq!(metrics.expected_holes, 1);
    assert_eq!(metrics.actual_holes, 1);
    assert_eq!(metrics.expected_components, 1);
}

#[test]
fn metrics_report_shift_in_pixels_and_physical_units() {
    let metrics =
        segmentation_metrics(&[run(1, 1, 1, 2)], &[run(1, 1, 2, 2)], (5, 4), (0.5, 0.25)).unwrap();

    close(metrics.dice, 0.5);
    close(metrics.centroid_distance_pixels, 1.0);
    close(metrics.centroid_distance_um, 250.0);
    close(metrics.hd95_pixels, 1.0);
    close(metrics.assd_pixels, 0.5);
}

#[test]
fn metrics_preserve_segment_identity_and_overlap() {
    let expected = [run(1, 0, 0, 2), run(2, 0, 1, 2)];
    let actual = [run(1, 0, 1, 2), run(2, 0, 0, 2)];

    let metrics = segmentation_metrics(&expected, &actual, (2, 3), (0.5, 0.25)).unwrap();

    close(metrics.dice, 0.5);
    assert_eq!(metrics.expected_overlap_pixels, 1);
    assert_eq!(metrics.actual_overlap_pixels, 1);
    assert_eq!(metrics.overlap_difference_pixels, 0);
}

#[test]
fn metrics_reject_out_of_bounds_runs() {
    let error = segmentation_metrics(&[run(1, 0, 2, 2)], &[], (2, 3), (0.5, 0.25)).unwrap_err();

    assert!(error.contains("outside mask bounds"));
}

#[test]
fn metrics_detect_lost_holes_and_overlaps() {
    let expected_hole = runs_with_hole();
    let mut filled_hole: Vec<_> = expected_hole
        .iter()
        .filter(|run| run.row != 2)
        .cloned()
        .collect();
    filled_hole.push(run(1, 2, 0, 5));
    let hole_metrics =
        segmentation_metrics(&expected_hole, &filled_hole, (5, 5), (0.5, 0.25)).unwrap();
    assert_eq!(hole_metrics.expected_holes, 1);
    assert_eq!(hole_metrics.actual_holes, 0);

    let expected_overlap = [run(1, 0, 0, 2), run(2, 0, 1, 2)];
    let lost_overlap = [run(1, 0, 0, 1), run(2, 0, 1, 2)];
    let overlap_metrics =
        segmentation_metrics(&expected_overlap, &lost_overlap, (1, 3), (0.5, 0.25)).unwrap();
    assert_eq!(overlap_metrics.expected_overlap_pixels, 1);
    assert_eq!(overlap_metrics.actual_overlap_pixels, 0);
}
