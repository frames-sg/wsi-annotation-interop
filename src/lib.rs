#![forbid(unsafe_code)]

pub mod compare;
pub mod metrics;
pub mod orthanc;
pub mod probe;
pub mod results;
pub mod schema;
pub mod shim;
pub mod validators;

mod conversion_matrix;
mod ground_truth;
mod matrix;
mod process;
mod profiles;
mod scale;

pub use conversion_matrix::{
    ConversionMatrixKind, ConversionMatrixResult, ConversionObservation, ConversionStatus,
    ConversionTarget, run_conversion_matrices,
};
pub use ground_truth::build_core_ground_truth;
pub use matrix::{
    CoreMatrixResult, MatrixObservation, MatrixPhase, PhaseObservation, PhaseStatus,
    run_core_matrix,
};
pub use profiles::{ProfileResult, run_core_profile, run_full_profile};
pub use scale::{ScaleCase, ScaleObservation, ScaleStatus, default_scale_cases, run_scale_cases};
