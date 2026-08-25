use super::usize_as_f64;

pub(crate) fn distance_transform(
    features: &[bool],
    shape: (usize, usize),
    sampling: (f64, f64),
) -> Vec<f64> {
    let (rows, columns) = shape;
    let mut horizontal = vec![f64::INFINITY; features.len()];
    for row in 0..rows {
        let input: Vec<_> = (0..columns)
            .map(|column| {
                if features[row * columns + column] {
                    0.0
                } else {
                    f64::INFINITY
                }
            })
            .collect();
        let transformed = squared_distance_transform_1d(&input, sampling.1);
        horizontal[row * columns..(row + 1) * columns].copy_from_slice(&transformed);
    }
    let mut result = vec![f64::INFINITY; features.len()];
    for column in 0..columns {
        let input: Vec<_> = (0..rows)
            .map(|row| horizontal[row * columns + column])
            .collect();
        let transformed = squared_distance_transform_1d(&input, sampling.0);
        for (row, distance) in transformed.into_iter().enumerate() {
            result[row * columns + column] = distance.sqrt();
        }
    }
    result
}

// Lower-envelope transform from Felzenszwalb and Huttenlocher, generalized for spacing.
fn squared_distance_transform_1d(input: &[f64], spacing: f64) -> Vec<f64> {
    let candidates: Vec<_> = input
        .iter()
        .enumerate()
        .filter_map(|(index, value)| value.is_finite().then_some(index))
        .collect();
    if candidates.is_empty() {
        return vec![f64::INFINITY; input.len()];
    }
    let scale = spacing * spacing;
    let mut sites = vec![0_usize; candidates.len()];
    let mut boundaries = vec![0.0; candidates.len() + 1];
    let mut envelope = 0_usize;
    sites[0] = candidates[0];
    boundaries[0] = f64::NEG_INFINITY;
    boundaries[1] = f64::INFINITY;
    for &site in &candidates[1..] {
        let mut boundary = intersection(input, scale, site, sites[envelope]);
        while boundary <= boundaries[envelope] {
            envelope -= 1;
            boundary = intersection(input, scale, site, sites[envelope]);
        }
        envelope += 1;
        sites[envelope] = site;
        boundaries[envelope] = boundary;
        boundaries[envelope + 1] = f64::INFINITY;
    }
    let mut result = vec![0.0; input.len()];
    envelope = 0;
    for (position, output) in result.iter_mut().enumerate() {
        while boundaries[envelope + 1] < usize_as_f64(position) {
            envelope += 1;
        }
        let delta = usize_as_f64(position.abs_diff(sites[envelope]));
        *output = scale * delta * delta + input[sites[envelope]];
    }
    result
}

fn intersection(input: &[f64], scale: f64, left: usize, right: usize) -> f64 {
    let left_position = usize_as_f64(left);
    let right_position = usize_as_f64(right);
    ((input[left] + scale * left_position * left_position)
        - (input[right] + scale * right_position * right_position))
        / (2.0 * scale * (left_position - right_position))
}

pub(crate) fn distance_summary(distances: &[f64]) -> (f64, f64) {
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
    let fraction = usize_as_f64(remainder) / 20.0;
    let percentile = sorted[lower] + (sorted[upper] - sorted[lower]) * fraction;
    let mean = sorted.iter().sum::<f64>() / usize_as_f64(sorted.len());
    (percentile, mean)
}

#[cfg(test)]
mod tests {
    use super::{distance_transform, squared_distance_transform_1d};

    #[test]
    fn distance_transform_handles_leading_infinity_and_spacing() {
        let result = squared_distance_transform_1d(&[f64::INFINITY, 0.0, f64::INFINITY], 0.5);
        for (actual, expected) in result.iter().zip([0.25, 0.0, 0.25]) {
            assert!((actual - expected).abs() < f64::EPSILON);
        }
    }

    #[test]
    fn distance_transform_matches_an_obvious_reference() {
        let shape = (4, 5);
        let mut features = vec![false; shape.0 * shape.1];
        features[1] = true;
        features[3 * shape.1 + 4] = true;
        let sampling = (0.5, 0.25);
        let actual = distance_transform(&features, shape, sampling);
        for row in 0..shape.0 {
            for column in 0..shape.1 {
                let expected = [(0_usize, 1_usize), (3, 4)]
                    .iter()
                    .map(|(feature_row, feature_column)| {
                        let row_delta = super::usize_as_f64(row.abs_diff(*feature_row));
                        let column_delta = super::usize_as_f64(column.abs_diff(*feature_column));
                        (row_delta * sampling.0).hypot(column_delta * sampling.1)
                    })
                    .fold(f64::INFINITY, f64::min);
                assert!((actual[row * shape.1 + column] - expected).abs() < 1e-12);
            }
        }
    }
}
