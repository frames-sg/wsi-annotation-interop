use std::path::Path;

use super::super::{ValidatorObservation, ValidatorStatus};

/// Reclassify the documented `validate_iods` Parametric Map functional-group
/// table defect after the independent highdicom PM control exhibits it too.
///
/// Returns `true` only when the control confirms the exact defect signature.
pub fn qualify_validate_iods_pm_defect(
    observations: &mut [ValidatorObservation],
    highdicom_pm: &Path,
) -> bool {
    let oracle_path = highdicom_pm.to_string_lossy();
    let oracle_confirmed = observations.iter().any(|observation| {
        observation.files.as_slice() == [oracle_path.as_ref()]
            && is_known_parametric_map_table_defect(observation)
    });
    if !oracle_confirmed {
        return false;
    }
    for observation in observations {
        if is_known_parametric_map_table_defect(observation) {
            observation.status = ValidatorStatus::KnownValidatorDefect;
        }
    }
    true
}

fn is_known_parametric_map_table_defect(observation: &ValidatorObservation) -> bool {
    const REQUIRED: [&str; 5] = [
        "Pixel Measures Sequence",
        "Frame VOI LUT Sequence",
        "Pixel Value Transformation Sequence",
        "Parametric Map Frame Type Sequence",
        "Real World Value Mapping Sequence",
    ];
    const ALLOWED: [&str; 7] = [
        "Pixel Measures Sequence",
        "Frame VOI LUT Sequence",
        "Pixel Value Transformation Sequence",
        "Parametric Map Frame Type Sequence",
        "Real World Value Mapping Sequence",
        "Derivation Image Sequence",
        "Frame Content Sequence",
    ];
    if observation.name != "validate_iods"
        || observation.status != ValidatorStatus::Failed
        || observation.files.len() != 1
        || observation.stdout_capture.truncated
        || observation.stderr_capture.truncated
    {
        return false;
    }
    let output = format!("{}{}", observation.stdout, observation.stderr);
    if !output.contains("(Parametric Map IOD)")
        || REQUIRED
            .iter()
            .any(|name| !output.contains(&format!("({name}) is unexpected")))
    {
        return false;
    }
    output.lines().all(|line| {
        let line = line.trim();
        line.is_empty()
            || line.starts_with("Using DICOM edition ")
            || line.starts_with("Validating DICOM file ")
            || line.starts_with("SOP class is ")
            || matches!(
                line,
                "Errors" | "======" | "Module \"Multi-frame Functional Groups\":"
            )
            || matches!(
                line,
                "(5200,9229) (Shared Functional Groups Sequence):"
                    | "(5200,9230) (Per-Frame Functional Groups Sequence):"
            )
            || (line.starts_with("Tag ")
                && ALLOWED
                    .iter()
                    .any(|name| line.ends_with(&format!("({name}) is unexpected"))))
    })
}
