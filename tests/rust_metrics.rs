use wsi_annotation_interop::metrics::{
    BinaryRun, MetricComputationStatus, MetricLimits, segmentation_metrics,
    segmentation_metrics_with_limits,
};

#[derive(Debug)]
struct DenseReference {
    dice: f64,
    centroid_distance_pixels: f64,
    hd95_pixels: f64,
    assd_pixels: f64,
    expected_components: usize,
    actual_components: usize,
    expected_holes: usize,
    actual_holes: usize,
    expected_overlap: usize,
    actual_overlap: usize,
}

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
    close(metrics.hd95_pixels.unwrap(), 0.0);
    close(metrics.assd_pixels.unwrap(), 0.0);
    assert_eq!(metrics.expected_holes, Some(1));
    assert_eq!(metrics.actual_holes, Some(1));
    assert_eq!(metrics.expected_components, Some(1));
    assert_eq!(metrics.advanced_metrics, MetricComputationStatus::Available);
}

#[test]
fn metrics_report_shift_in_pixels_and_physical_units() {
    let metrics =
        segmentation_metrics(&[run(1, 1, 1, 2)], &[run(1, 1, 2, 2)], (5, 4), (0.5, 0.25)).unwrap();

    close(metrics.dice, 0.5);
    close(metrics.centroid_distance_pixels, 1.0);
    close(metrics.centroid_distance_um, 250.0);
    close(metrics.hd95_pixels.unwrap(), 1.0);
    close(metrics.assd_pixels.unwrap(), 0.5);
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
    assert_eq!(hole_metrics.expected_holes, Some(1));
    assert_eq!(hole_metrics.actual_holes, Some(0));

    let expected_overlap = [run(1, 0, 0, 2), run(2, 0, 1, 2)];
    let lost_overlap = [run(1, 0, 0, 1), run(2, 0, 1, 2)];
    let overlap_metrics =
        segmentation_metrics(&expected_overlap, &lost_overlap, (1, 3), (0.5, 0.25)).unwrap();
    assert_eq!(overlap_metrics.expected_overlap_pixels, 1);
    assert_eq!(overlap_metrics.actual_overlap_pixels, 0);
}

#[test]
fn duplicate_overlapping_and_adjacent_runs_normalize_to_their_union() {
    let redundant = [
        run(1, 0, 3, 2),
        run(1, 0, 0, 2),
        run(1, 0, 1, 3),
        run(1, 0, 0, 2),
    ];
    let canonical = [run(1, 0, 0, 5)];

    let metrics = segmentation_metrics(&redundant, &canonical, (1, 5), (0.5, 0.25)).unwrap();

    assert_eq!(metrics.expected_area_pixels, 5);
    assert_eq!(metrics.actual_area_pixels, 5);
    close(metrics.dice, 1.0);
}

#[test]
fn huge_declared_slide_with_tiny_occupancy_does_not_require_full_slide_memory() {
    let runs = [run(1, 99_999, 99_999, 1)];
    let metrics =
        segmentation_metrics(&runs, &runs, (100_000, 100_000), (0.000_25, 0.000_25)).unwrap();

    assert_eq!(metrics.expected_area_pixels, 1);
    assert_eq!(metrics.actual_area_pixels, 1);
    assert_eq!(metrics.advanced_metrics, MetricComputationStatus::Available);
    close(metrics.hd95_pixels.unwrap(), 0.0);
}

#[test]
fn an_adversarial_sparse_bounding_box_returns_an_explicit_resource_status() {
    let runs = [run(1, 0, 0, 1), run(1, 99_999, 99_999, 1)];
    let metrics = segmentation_metrics_with_limits(
        &runs,
        &runs,
        (100_000, 100_000),
        (0.000_25, 0.000_25),
        MetricLimits {
            max_crop_pixels: 1_000,
        },
    )
    .unwrap();

    assert_eq!(metrics.expected_area_pixels, 2);
    close(metrics.dice, 1.0);
    assert_eq!(metrics.hd95_pixels, None);
    assert_eq!(metrics.expected_components, None);
    assert!(matches!(
        metrics.advanced_metrics,
        MetricComputationStatus::ResourceLimited {
            limit_pixels: 1_000,
            ..
        }
    ));
}

#[test]
fn sparse_results_are_deterministic_independent_of_run_order() {
    let ordered = [run(1, 0, 0, 2), run(1, 2, 2, 1), run(2, 1, 1, 3)];
    let shuffled = [run(2, 1, 1, 3), run(1, 2, 2, 1), run(1, 0, 0, 2)];

    let left = segmentation_metrics(&ordered, &ordered, (4, 5), (0.5, 0.25)).unwrap();
    let right = segmentation_metrics(&shuffled, &ordered, (4, 5), (0.5, 0.25)).unwrap();

    assert_eq!(left, right);
}

#[test]
fn run_validation_rejects_zero_values_and_checked_end_overflow() {
    assert!(segmentation_metrics(&[run(0, 0, 0, 1)], &[], (1, 1), (1.0, 1.0)).is_err());
    assert!(segmentation_metrics(&[run(1, 0, 0, 0)], &[], (1, 1), (1.0, 1.0)).is_err());
    let error = segmentation_metrics(
        &[run(1, 0, usize::MAX - 1, 2)],
        &[],
        (1, usize::MAX),
        (1.0, 1.0),
    )
    .unwrap_err();
    assert!(error.contains("outside mask bounds"));
}

#[test]
fn empty_and_diagonal_connectivity_semantics_are_explicit() {
    let empty = segmentation_metrics(&[], &[], (3, 3), (0.5, 0.25)).unwrap();
    close(empty.dice, 1.0);
    close(empty.hd95_pixels.unwrap(), 0.0);
    assert_eq!(empty.expected_components, Some(0));

    let nonempty = segmentation_metrics(&[], &[run(1, 1, 1, 1)], (3, 3), (0.5, 0.25)).unwrap();
    assert!(nonempty.centroid_distance_pixels.is_infinite());
    assert!(nonempty.hd95_pixels.unwrap().is_infinite());

    let diagonal = [run(1, 0, 0, 1), run(1, 1, 1, 1), run(1, 2, 2, 1)];
    let separated = [run(1, 0, 0, 1), run(1, 2, 2, 1)];
    let diagonal_metrics = segmentation_metrics(&diagonal, &diagonal, (3, 3), (0.5, 0.25)).unwrap();
    let separated_metrics =
        segmentation_metrics(&separated, &separated, (3, 3), (0.5, 0.25)).unwrap();
    assert_eq!(diagonal_metrics.expected_components, Some(1));
    assert_eq!(separated_metrics.expected_components, Some(2));
}

#[test]
fn large_sparse_multisegment_overlap_stays_run_proportional() {
    let mut runs = Vec::new();
    for segment in 1..=200 {
        runs.push(run(segment, usize::from(segment) * 100, 50_000, 10));
    }
    runs.push(run(201, 20_000, 50_005, 10));
    let metrics = segmentation_metrics_with_limits(
        &runs,
        &runs,
        (100_000, 100_000),
        (0.000_25, 0.000_25),
        MetricLimits {
            max_crop_pixels: 100,
        },
    )
    .unwrap();
    assert_eq!(metrics.expected_area_pixels, 2_010);
    assert_eq!(metrics.expected_overlap_pixels, 5);
    close(metrics.dice, 1.0);
}

#[test]
fn randomized_small_masks_match_an_independent_dense_oracle() {
    let shape = (6, 7);
    let mut state = 0x4d59_5df4_d0f3_3173_u64;
    for _case in 0..48 {
        let expected = random_masks(&mut state, shape);
        let actual = random_masks(&mut state, shape);
        let expected_runs = masks_to_runs(&expected, shape);
        let actual_runs = masks_to_runs(&actual, shape);
        let sparse =
            segmentation_metrics(&expected_runs, &actual_runs, shape, (0.5, 0.25)).unwrap();
        let dense = dense_reference(&expected, &actual, shape);

        close(sparse.dice, dense.dice);
        close(
            sparse.centroid_distance_pixels,
            dense.centroid_distance_pixels,
        );
        close(sparse.hd95_pixels.unwrap(), dense.hd95_pixels);
        close(sparse.assd_pixels.unwrap(), dense.assd_pixels);
        assert_eq!(sparse.expected_components, Some(dense.expected_components));
        assert_eq!(sparse.actual_components, Some(dense.actual_components));
        assert_eq!(sparse.expected_holes, Some(dense.expected_holes));
        assert_eq!(sparse.actual_holes, Some(dense.actual_holes));
        assert_eq!(sparse.expected_overlap_pixels, dense.expected_overlap);
        assert_eq!(sparse.actual_overlap_pixels, dense.actual_overlap);
    }
}

#[test]
#[ignore = "calibration benchmark; run explicitly in release mode"]
fn sparse_metrics_calibration() {
    let shape = (32, 32);
    let mut state = 0x6a09_e667_f3bc_c909_u64;
    let expected = random_masks(&mut state, shape);
    let actual = random_masks(&mut state, shape);
    let expected_runs = masks_to_runs(&expected, shape);
    let actual_runs = masks_to_runs(&actual, shape);
    let dense = dense_reference(&expected, &actual, shape);
    let sparse = segmentation_metrics(&expected_runs, &actual_runs, shape, (0.5, 0.25)).unwrap();
    close(dense.dice, sparse.dice);
    close(dense.hd95_pixels, sparse.hd95_pixels.unwrap());

    eprintln!("workload,implementation,repetitions,median_us,min_us,max_us,checksum");
    benchmark("32x32-random", "dense-reference", || {
        let value = dense_reference(&expected, &actual, shape);
        value.expected_components + value.actual_components + value.expected_overlap
    });
    benchmark("32x32-random", "sparse", || {
        let value = segmentation_metrics(&expected_runs, &actual_runs, shape, (0.5, 0.25))
            .expect("sparse calibration succeeds");
        value.expected_components.unwrap_or(0)
            + value.actual_components.unwrap_or(0)
            + value.expected_overlap_pixels
    });

    let huge = [run(1, 99_998, 20, 4), run(2, 99_999, 22, 3)];
    benchmark("100000x100000-2-runs", "sparse", || {
        let value = segmentation_metrics(&huge, &huge, (100_000, 100_000), (0.000_25, 0.000_25))
            .expect("huge sparse calibration succeeds");
        value.expected_area_pixels + value.expected_overlap_pixels
    });

    let adversarial = [run(1, 0, 0, 1), run(1, 99_999, 99_999, 1)];
    benchmark("100000x100000-wide-bbox", "sparse-resource-limit", || {
        let value = segmentation_metrics_with_limits(
            &adversarial,
            &adversarial,
            (100_000, 100_000),
            (0.000_25, 0.000_25),
            MetricLimits {
                max_crop_pixels: 1_000,
            },
        )
        .expect("resource-limited calibration succeeds");
        value.expected_area_pixels
    });
}

fn benchmark(workload: &str, implementation: &str, mut operation: impl FnMut() -> usize) {
    const REPETITIONS: usize = 9;
    let checksum = std::hint::black_box(operation());
    let mut timings = Vec::with_capacity(REPETITIONS);
    for _ in 0..REPETITIONS {
        let started = std::time::Instant::now();
        assert_eq!(std::hint::black_box(operation()), checksum);
        timings.push(started.elapsed().as_micros());
    }
    timings.sort_unstable();
    eprintln!(
        "{workload},{implementation},{REPETITIONS},{},{},{},{checksum}",
        timings[REPETITIONS / 2],
        timings[0],
        timings[REPETITIONS - 1]
    );
}

fn random_masks(state: &mut u64, shape: (usize, usize)) -> Vec<Vec<bool>> {
    (0..2)
        .map(|_| {
            (0..shape.0 * shape.1)
                .map(|_| {
                    *state ^= *state << 13;
                    *state ^= *state >> 7;
                    *state ^= *state << 17;
                    (*state).is_multiple_of(5)
                })
                .collect()
        })
        .collect()
}

fn masks_to_runs(masks: &[Vec<bool>], shape: (usize, usize)) -> Vec<BinaryRun> {
    let mut runs = Vec::new();
    for (segment, mask) in masks.iter().enumerate() {
        for row in 0..shape.0 {
            let mut column = 0;
            while column < shape.1 {
                if !mask[row * shape.1 + column] {
                    column += 1;
                    continue;
                }
                let start = column;
                while column < shape.1 && mask[row * shape.1 + column] {
                    column += 1;
                }
                runs.push(run(
                    u16::try_from(segment + 1).unwrap(),
                    row,
                    start,
                    column - start,
                ));
            }
        }
    }
    runs
}

fn dense_reference(
    expected: &[Vec<bool>],
    actual: &[Vec<bool>],
    shape: (usize, usize),
) -> DenseReference {
    let expected_area = expected.iter().flatten().filter(|pixel| **pixel).count();
    let actual_area = actual.iter().flatten().filter(|pixel| **pixel).count();
    let intersection = expected
        .iter()
        .zip(actual)
        .map(|(left, right)| {
            left.iter()
                .zip(right)
                .filter(|(left, right)| **left && **right)
                .count()
        })
        .sum::<usize>();
    let total = expected_area + actual_area;
    let dice = if total == 0 {
        1.0
    } else {
        2.0 * to_f64(intersection) / to_f64(total)
    };
    let centroid_distance_pixels = match (
        dense_centroid(expected, shape),
        dense_centroid(actual, shape),
    ) {
        (None, None) => 0.0,
        (Some(_), None) | (None, Some(_)) => f64::INFINITY,
        (Some(left), Some(right)) => (right.0 - left.0).hypot(right.1 - left.1),
    };
    let distances = expected
        .iter()
        .zip(actual)
        .flat_map(|(left, right)| dense_surface_distances(left, right, shape))
        .collect::<Vec<_>>();
    let (hd95_pixels, assd_pixels) = dense_distance_summary(&distances);
    DenseReference {
        dice,
        centroid_distance_pixels,
        hd95_pixels,
        assd_pixels,
        expected_components: expected
            .iter()
            .map(|mask| dense_component_count(mask, shape, true))
            .sum(),
        actual_components: actual
            .iter()
            .map(|mask| dense_component_count(mask, shape, true))
            .sum(),
        expected_holes: expected
            .iter()
            .map(|mask| dense_hole_count(mask, shape))
            .sum(),
        actual_holes: actual
            .iter()
            .map(|mask| dense_hole_count(mask, shape))
            .sum(),
        expected_overlap: dense_overlap(expected),
        actual_overlap: dense_overlap(actual),
    }
}

fn dense_centroid(masks: &[Vec<bool>], shape: (usize, usize)) -> Option<(f64, f64)> {
    let mut rows = 0.0;
    let mut columns = 0.0;
    let mut count = 0_usize;
    for mask in masks {
        for (index, selected) in mask.iter().enumerate() {
            if *selected {
                rows += to_f64(index / shape.1);
                columns += to_f64(index % shape.1);
                count += 1;
            }
        }
    }
    (count != 0).then(|| (rows / to_f64(count), columns / to_f64(count)))
}

fn dense_surface_distances(left: &[bool], right: &[bool], shape: (usize, usize)) -> Vec<f64> {
    let left_surface = dense_surface(left, shape);
    let right_surface = dense_surface(right, shape);
    if left_surface.is_empty() && right_surface.is_empty() {
        return Vec::new();
    }
    if left_surface.is_empty() || right_surface.is_empty() {
        return vec![f64::INFINITY];
    }
    left_surface
        .iter()
        .map(|left| nearest(*left, &right_surface))
        .chain(
            right_surface
                .iter()
                .map(|right| nearest(*right, &left_surface)),
        )
        .collect()
}

fn dense_surface(mask: &[bool], shape: (usize, usize)) -> Vec<(usize, usize)> {
    let mut points = Vec::new();
    for row in 0..shape.0 {
        for column in 0..shape.1 {
            let index = row * shape.1 + column;
            let edge = row == 0 || column == 0 || row + 1 == shape.0 || column + 1 == shape.1;
            let touches_background = dense_neighbors(row, column, shape, false)
                .any(|(next_row, next_column)| !mask[next_row * shape.1 + next_column]);
            if mask[index] && (edge || touches_background) {
                points.push((row, column));
            }
        }
    }
    points
}

fn nearest(point: (usize, usize), others: &[(usize, usize)]) -> f64 {
    others
        .iter()
        .map(|other| {
            let rows = to_f64(point.0.abs_diff(other.0));
            let columns = to_f64(point.1.abs_diff(other.1));
            rows.hypot(columns)
        })
        .fold(f64::INFINITY, f64::min)
}

fn dense_distance_summary(distances: &[f64]) -> (f64, f64) {
    if distances.is_empty() {
        return (0.0, 0.0);
    }
    if distances.iter().any(|value| value.is_infinite()) {
        return (f64::INFINITY, f64::INFINITY);
    }
    let mut sorted = distances.to_vec();
    sorted.sort_by(f64::total_cmp);
    let steps = sorted.len() - 1;
    let lower = (steps / 20) * 19 + ((steps % 20) * 19) / 20;
    let remainder = ((steps % 20) * 19) % 20;
    let upper = lower + usize::from(remainder != 0);
    let percentile = sorted[lower] + (sorted[upper] - sorted[lower]) * (to_f64(remainder) / 20.0);
    let mean = sorted.iter().sum::<f64>() / to_f64(sorted.len());
    (percentile, mean)
}

fn dense_component_count(mask: &[bool], shape: (usize, usize), diagonal: bool) -> usize {
    let mut visited = vec![false; mask.len()];
    let mut count = 0;
    for start in 0..mask.len() {
        if visited[start] || !mask[start] {
            continue;
        }
        count += 1;
        visited[start] = true;
        let mut queue = std::collections::VecDeque::from([(start / shape.1, start % shape.1)]);
        while let Some((row, column)) = queue.pop_front() {
            for (next_row, next_column) in dense_neighbors(row, column, shape, diagonal) {
                let next = next_row * shape.1 + next_column;
                if mask[next] && !visited[next] {
                    visited[next] = true;
                    queue.push_back((next_row, next_column));
                }
            }
        }
    }
    count
}

fn dense_hole_count(mask: &[bool], shape: (usize, usize)) -> usize {
    let mut exterior = vec![false; mask.len()];
    let mut queue = std::collections::VecDeque::new();
    for index in 0..mask.len() {
        let row = index / shape.1;
        let column = index % shape.1;
        if (row == 0 || column == 0 || row + 1 == shape.0 || column + 1 == shape.1)
            && !mask[index]
            && !exterior[index]
        {
            exterior[index] = true;
            queue.push_back((row, column));
        }
    }
    while let Some((row, column)) = queue.pop_front() {
        for (next_row, next_column) in dense_neighbors(row, column, shape, false) {
            let next = next_row * shape.1 + next_column;
            if !mask[next] && !exterior[next] {
                exterior[next] = true;
                queue.push_back((next_row, next_column));
            }
        }
    }
    let holes = mask
        .iter()
        .zip(exterior)
        .map(|(selected, exterior)| !selected && !exterior)
        .collect::<Vec<_>>();
    dense_component_count(&holes, shape, true)
}

fn dense_overlap(masks: &[Vec<bool>]) -> usize {
    (0..masks[0].len())
        .filter(|index| masks.iter().filter(|mask| mask[*index]).count() > 1)
        .count()
}

fn dense_neighbors(
    row: usize,
    column: usize,
    shape: (usize, usize),
    diagonal: bool,
) -> impl Iterator<Item = (usize, usize)> {
    (-1..=1).flat_map(move |row_delta| {
        (-1..=1).filter_map(move |column_delta| {
            if (row_delta == 0 && column_delta == 0)
                || (!diagonal && row_delta != 0 && column_delta != 0)
            {
                return None;
            }
            let next_row = row.checked_add_signed(row_delta)?;
            let next_column = column.checked_add_signed(column_delta)?;
            (next_row < shape.0 && next_column < shape.1).then_some((next_row, next_column))
        })
    })
}

#[allow(clippy::cast_precision_loss)]
fn to_f64(value: usize) -> f64 {
    // The dense oracle only operates on 6 x 7 masks, so every converted integer is exact.
    value as f64
}
