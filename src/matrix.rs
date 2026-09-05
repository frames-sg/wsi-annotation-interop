const PIXEL_TOLERANCE: f64 = 1e-6;
const MILLIMETER_TOLERANCE: f64 = 1e-9;

mod case;
mod finalize;
mod observation;
mod phases;
#[cfg(test)]
mod tests;

pub use case::run_core_matrix;
pub use observation::{
    CoreMatrixResult, MatrixObservation, MatrixPhase, PhaseObservation, PhaseStatus,
};
