use std::path::Path;

use super::super::{ValidatorObservation, ValidatorStatus};

/// Reclassify stale TID 1410 checks for tiled SEG references only after the
/// same validator reports the exact defect for an independent highdicom SR.
///
/// Current TID 1410 requires both frame and segment selectors for a tiled SEG.
pub fn qualify_tiled_segmentation_sr_validator_defect(
    observations: &mut [ValidatorObservation],
    highdicom_sr: &Path,
) -> bool {
    let oracle_path = highdicom_sr.to_string_lossy();
    let confirmed_validators = observations
        .iter()
        .filter(|observation| {
            observation.files.as_slice() == [oracle_path.as_ref()]
                && is_known_tiled_segmentation_sr_defect(observation)
        })
        .map(|observation| observation.name.clone())
        .collect::<Vec<_>>();
    if confirmed_validators.is_empty() {
        return false;
    }
    for observation in observations {
        if confirmed_validators.contains(&observation.name)
            && is_known_tiled_segmentation_sr_defect(observation)
        {
            observation.status = ValidatorStatus::KnownValidatorDefect;
        }
    }
    true
}

fn is_known_tiled_segmentation_sr_defect(observation: &ValidatorObservation) -> bool {
    if observation.status != ValidatorStatus::Failed
        || observation.files.len() != 1
        || observation.stdout_capture.truncated
        || observation.stderr_capture.truncated
    {
        return false;
    }
    let output = format!("{}{}", observation.stdout, observation.stderr);
    match observation.name.as_str() {
        "dciodvfy" => {
            const DEFECT: &str = "Error - Shall not be present when ReferencedFrameNumber is present - attribute <ReferencedSegmentNumber>";
            output.contains("Comprehensive3DSR")
                && output
                    .lines()
                    .filter(|line| line.trim_start().starts_with("Error - "))
                    .eq([DEFECT])
        }
        "validate_iods" => is_known_validate_iods_tiled_segmentation_sr_defect(&output),
        _ => false,
    }
}

fn is_known_validate_iods_tiled_segmentation_sr_defect(output: &str) -> bool {
    const FRAME_ERROR: &str = "Tag (0008,1160) (Referenced Frame Number) is unexpected";
    const SEGMENT_ERROR: &str = "Tag (0062,000B) (Referenced Segment Number) is unexpected";
    if !output.contains("(Comprehensive 3D SR IOD)")
        || !output.contains(FRAME_ERROR)
        || !output.contains(SEGMENT_ERROR)
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
                "Errors" | "======" | "Module \"SR Document Content\":"
            )
            || (line.starts_with("(0040,A730) ")
                && line.ends_with("(0008,1199) (Referenced SOP Sequence):"))
            || matches!(line, FRAME_ERROR | SEGMENT_ERROR)
    })
}
