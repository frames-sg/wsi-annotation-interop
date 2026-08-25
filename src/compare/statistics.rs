use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct ErrorStats {
    pub count: usize,
    pub max: f64,
    pub median: f64,
    pub rms: f64,
}

pub(super) fn stats(values: &[f64]) -> ErrorStats {
    if values.is_empty() {
        return ErrorStats {
            count: 0,
            max: 0.0,
            median: 0.0,
            rms: 0.0,
        };
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    let middle = sorted.len() / 2;
    let median = if sorted.len().is_multiple_of(2) {
        f64::midpoint(sorted[middle - 1], sorted[middle])
    } else {
        sorted[middle]
    };
    ErrorStats {
        count: values.len(),
        max: sorted.last().copied().unwrap_or(0.0),
        median,
        rms: (values.iter().map(|value| value * value).sum::<f64>() / usize_as_f64(values.len()))
            .sqrt(),
    }
}

#[allow(clippy::cast_precision_loss)]
fn usize_as_f64(value: usize) -> f64 {
    value as f64
}
