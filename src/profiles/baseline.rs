use std::path::Path;

use serde_json::{Value, json};

use crate::conversion_matrix::{ConversionMatrixResult, run_conversion_matrices};
use crate::matrix::run_core_matrix;
use crate::probe::ViewerProbe;
use crate::shim::{FixtureSet, ReferenceShim};

use super::tagged;

pub(super) const DEFINITION_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy)]
pub(super) struct BaselineStatus {
    matrix: bool,
    conversion: bool,
}

impl BaselineStatus {
    pub(super) const fn new(matrix: bool, conversion: bool) -> Self {
        Self { matrix, conversion }
    }

    pub(super) const fn is_ok(self) -> bool {
        self.matrix && self.conversion
    }
}

pub(super) struct BaselineRun {
    pub(super) fixtures: FixtureSet,
    pub(super) conversion: ConversionMatrixResult,
    pub(super) observations: Vec<Value>,
    pub(super) status: BaselineStatus,
}

pub(super) fn run(
    reference: &ReferenceShim,
    probe: &ViewerProbe,
    run_directory: &Path,
) -> Result<BaselineRun, String> {
    let fixtures = reference
        .generate_core(&run_directory.join("fixtures"))
        .map_err(|error| error.to_string())?;
    let matrix = run_core_matrix(
        &fixtures,
        reference,
        probe,
        &run_directory.join("roundtrips"),
    )?;
    let conversion = run_conversion_matrices(
        &fixtures,
        reference,
        probe,
        &run_directory.join("conversions"),
    )?;
    let status = BaselineStatus::new(matrix.is_ok(), conversion.is_ok());
    let mut observations = matrix
        .observations
        .iter()
        .map(|observation| tagged("matrix", observation))
        .collect::<Result<Vec<_>, _>>()?;
    observations.extend(
        conversion
            .observations
            .iter()
            .map(|observation| tagged("conversion", observation))
            .collect::<Result<Vec<_>, _>>()?,
    );
    observations.push(qualification_observation(reference, &fixtures)?);
    Ok(BaselineRun {
        fixtures,
        conversion,
        observations,
        status,
    })
}

fn qualification_observation(
    reference: &ReferenceShim,
    fixtures: &FixtureSet,
) -> Result<Value, String> {
    let qualification = reference
        .qualify_pydcm(
            &fixtures.source,
            &fixtures.ann["2D_VOLUME"],
            &fixtures.seg["BINARY"],
        )
        .map_err(|error| error.to_string())?;
    let mut observation = tagged("qualification", &qualification)?;
    observation["case_id"] = json!("pydcm-qualification");
    observation["status"] = json!(if qualification.qualified {
        "qualified"
    } else {
        "unqualified"
    });
    Ok(observation)
}

#[cfg(test)]
mod tests {
    use super::BaselineStatus;

    #[test]
    fn status_requires_matrix_and_conversion() {
        assert!(BaselineStatus::new(true, true).is_ok());
        assert!(!BaselineStatus::new(false, true).is_ok());
        assert!(!BaselineStatus::new(true, false).is_ok());
    }
}
