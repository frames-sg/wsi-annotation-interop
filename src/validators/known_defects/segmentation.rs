use std::path::Path;

use super::super::{ValidatorObservation, ValidatorStatus};

/// Reclassify one documented `validate_iods` SEG functional-group table
/// signature after an independent highdicom SEG exhibits the same signature.
pub fn qualify_validate_iods_seg_defect(
    observations: &mut [ValidatorObservation],
    highdicom_seg: &Path,
) -> bool {
    let oracle_path = highdicom_seg.to_string_lossy();
    let Some(oracle_signature) = observations
        .iter()
        .find(|observation| observation.files.as_slice() == [oracle_path.as_ref()])
        .and_then(segmentation_table_signature)
    else {
        return false;
    };
    for observation in observations {
        if segmentation_table_signature(observation) == Some(oracle_signature) {
            observation.status = ValidatorStatus::KnownValidatorDefect;
        }
    }
    true
}

fn segmentation_table_signature(observation: &ValidatorObservation) -> Option<u8> {
    const FUNCTIONAL_GROUPS: [&str; 5] = [
        "Derivation Image Sequence",
        "Pixel Measures Sequence",
        "Frame Content Sequence",
        "Plane Position (Slide) Sequence",
        "Segment Identification Sequence",
    ];
    if observation.name != "validate_iods"
        || observation.status != ValidatorStatus::Failed
        || observation.files.len() != 1
        || observation.stdout_capture.truncated
        || observation.stderr_capture.truncated
    {
        return None;
    }
    let output = format!("{}{}", observation.stdout, observation.stderr);
    if !output.contains("(Segmentation IOD)") {
        return None;
    }
    let signature = FUNCTIONAL_GROUPS
        .iter()
        .enumerate()
        .fold(0_u8, |signature, (index, name)| {
            if output.contains(&format!("({name}) is unexpected")) {
                signature | (1 << index)
            } else {
                signature
            }
        });
    if signature == 0 {
        return None;
    }
    output
        .lines()
        .all(|line| {
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
                    && FUNCTIONAL_GROUPS
                        .iter()
                        .any(|name| line.ends_with(&format!("({name}) is unexpected"))))
        })
        .then_some(signature)
}
