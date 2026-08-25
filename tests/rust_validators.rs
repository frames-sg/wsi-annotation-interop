#![cfg(unix)]

use std::fs;
use std::time::Duration;

use tempfile::tempdir;
use wsi_annotation_interop::validators::{
    ValidatorInvocation, ValidatorSpec, ValidatorStatus,
    qualify_tiled_segmentation_sr_validator_defect, qualify_validate_iods_pm_defect,
    qualify_validate_iods_seg_defect, run_validator, standard_validator_specs,
};

#[test]
fn validator_captures_version_edition_commands_outputs_and_unsupported_status() {
    let directory = tempdir().unwrap();
    let first = directory.path().join("first.dcm");
    let second = directory.path().join("second.dcm");
    fs::write(&first, b"dicom").unwrap();
    fs::write(&second, b"dicom").unwrap();
    let validation_script = "printf 'unsupported object'; printf 'detail' >&2; exit 3";
    let version_script = "printf 'validator 1.2.3'";
    let spec = ValidatorSpec {
        name: "synthetic".to_owned(),
        command: vec![
            "/bin/sh".to_owned(),
            "-c".to_owned(),
            validation_script.to_owned(),
        ],
        version_command: vec![
            "/bin/sh".to_owned(),
            "-c".to_owned(),
            version_script.to_owned(),
        ],
        validation_args: Vec::new(),
        invocation: ValidatorInvocation::Set,
        edition: Some("2026c".to_owned()),
        unsupported_markers: vec!["unsupported object".to_owned()],
    };

    let observations = run_validator(&spec, &[first, second], Duration::from_secs(2)).unwrap();

    assert_eq!(observations.len(), 1);
    let observation = &observations[0];
    assert_eq!(observation.status, ValidatorStatus::Unsupported);
    assert_eq!(observation.returncode, Some(3));
    assert_eq!(observation.stdout, "unsupported object");
    assert_eq!(observation.stderr, "detail");
    assert_eq!(observation.version_stdout, "validator 1.2.3");
    assert_eq!(observation.edition.as_deref(), Some("2026c"));
    assert!(observation.elapsed_ms >= 0.0);
    assert!(observation.peak_rss_bytes > 0);
    assert!(observation.sampling.sampled);
    assert_eq!(observation.sampling.interval_ms, Some(100));
    assert_eq!(observation.stdout_capture.total_bytes, 18);
    assert_eq!(observation.stderr_capture.total_bytes, 6);
    assert!(!observation.stdout_capture.truncated);
    assert!(!observation.stderr_capture.truncated);
}

#[test]
fn missing_validator_is_reported_once_as_unavailable() {
    let directory = tempdir().unwrap();
    let dicom = directory.path().join("input.dcm");
    fs::write(&dicom, b"dicom").unwrap();
    let spec = ValidatorSpec {
        name: "missing".to_owned(),
        command: vec![
            directory
                .path()
                .join("not-installed")
                .to_string_lossy()
                .into_owned(),
        ],
        version_command: Vec::new(),
        validation_args: Vec::new(),
        invocation: ValidatorInvocation::Each,
        edition: None,
        unsupported_markers: Vec::new(),
    };

    let observations = run_validator(&spec, &[dicom], Duration::from_secs(2)).unwrap();

    assert_eq!(observations.len(), 1);
    assert_eq!(observations[0].status, ValidatorStatus::Unavailable);
    assert!(observations[0].stderr.contains("not found"));
}

#[test]
fn dciodvfy_error_diagnostic_fails_even_when_the_process_exits_zero() {
    let directory = tempdir().unwrap();
    let dicom = directory.path().join("input.dcm");
    fs::write(&dicom, b"dicom").unwrap();
    let spec = ValidatorSpec {
        name: "dciodvfy".to_owned(),
        command: vec![
            "/bin/sh".to_owned(),
            "-c".to_owned(),
            "printf 'Comprehensive3DSR\nError - invalid content\n'; exit 0".to_owned(),
        ],
        version_command: Vec::new(),
        validation_args: Vec::new(),
        invocation: ValidatorInvocation::Each,
        edition: None,
        unsupported_markers: Vec::new(),
    };

    let observations = run_validator(&spec, &[dicom], Duration::from_secs(2)).unwrap();

    assert_eq!(observations[0].status, ValidatorStatus::Failed);
}

#[test]
fn standard_profile_declares_all_required_validators() {
    let specs = standard_validator_specs("2026c");
    let names: Vec<_> = specs.iter().map(|spec| spec.name.as_str()).collect();

    assert_eq!(names, ["validate_iods", "dciodvfy", "dcentvfy", "dcm2json"]);
    assert_eq!(specs[0].edition.as_deref(), Some("2026c"));
    assert_eq!(specs[2].invocation, ValidatorInvocation::Set);
}

#[test]
#[ignore = "adapter calibration; run in release mode"]
fn representative_validator_adapter_calibration() {
    let directory = tempdir().unwrap();
    let dicom = directory.path().join("input.dcm");
    fs::write(&dicom, b"dicom").unwrap();
    let spec = ValidatorSpec {
        name: "synthetic-validator".to_owned(),
        command: vec![
            "/bin/sh".to_owned(),
            "-c".to_owned(),
            "i=0; while [ $i -lt 200 ]; do printf 'Info - validated item %s\\n' \"$i\"; i=$((i+1)); done".to_owned(),
            "validator-calibration".to_owned(),
        ],
        version_command: vec!["/bin/sh".to_owned(), "-c".to_owned(), "printf 'validator 1.0'".to_owned()],
        validation_args: Vec::new(),
        invocation: ValidatorInvocation::Each,
        edition: Some("calibration".to_owned()),
        unsupported_markers: Vec::new(),
    };
    let mut samples = Vec::new();
    for repetition in 0..8 {
        let observation =
            run_validator(&spec, std::slice::from_ref(&dicom), Duration::from_secs(2))
                .unwrap()
                .pop()
                .unwrap();
        assert_eq!(observation.status, ValidatorStatus::Passed);
        assert!(observation.stdout.contains("validated item 199"));
        if repetition > 0 {
            samples.push(observation.elapsed_ms);
        }
    }
    samples.sort_by(f64::total_cmp);
    println!(
        "validator-adapter repetitions=7 median_ms={:.3} min_ms={:.3} max_ms={:.3}",
        samples[3], samples[0], samples[6]
    );
}

#[test]
fn known_pm_table_defect_requires_an_oracle_control_and_exact_diagnostics() {
    let directory = tempdir().unwrap();
    let oracle = directory.path().join("oracle-pm.dcm");
    let rust = directory.path().join("rust-pm.dcm");
    let unrelated = directory.path().join("unrelated.dcm");
    for path in [&oracle, &rust, &unrelated] {
        fs::write(path, b"dicom").unwrap();
    }
    let script = r#"
printf 'Using DICOM edition 2026c\nValidating DICOM file %s\nSOP class is "1.2.840.10008.5.1.4.1.1.30" (Parametric Map IOD)\n\nErrors\n======\n\nModule "Multi-frame Functional Groups":\n(5200,9229) (Shared Functional Groups Sequence):\n' "$1"
for name in 'Pixel Measures Sequence' 'Frame VOI LUT Sequence' 'Pixel Value Transformation Sequence' 'Parametric Map Frame Type Sequence' 'Real World Value Mapping Sequence'; do
  printf '  Tag (0000,0000) (%s) is unexpected\n' "$name"
done
case "$1" in *unrelated*) printf 'Required attribute is missing\n';; esac
exit 5
"#;
    let spec = ValidatorSpec {
        name: "validate_iods".to_owned(),
        command: vec![
            "/bin/sh".to_owned(),
            "-c".to_owned(),
            script.to_owned(),
            "synthetic-validator".to_owned(),
        ],
        version_command: Vec::new(),
        validation_args: Vec::new(),
        invocation: ValidatorInvocation::Each,
        edition: Some("2026c".to_owned()),
        unsupported_markers: Vec::new(),
    };
    let mut observations = run_validator(
        &spec,
        &[oracle.clone(), rust, unrelated],
        Duration::from_secs(2),
    )
    .unwrap();

    assert!(qualify_validate_iods_pm_defect(&mut observations, &oracle));
    assert_eq!(
        observations[0].status,
        ValidatorStatus::KnownValidatorDefect
    );
    assert_eq!(
        observations[1].status,
        ValidatorStatus::KnownValidatorDefect
    );
    assert_eq!(observations[2].status, ValidatorStatus::Failed);

    for observation in &mut observations {
        if observation.status == ValidatorStatus::KnownValidatorDefect {
            observation.status = ValidatorStatus::Failed;
        }
    }
    observations[0].stdout_capture.truncated = true;
    assert!(!qualify_validate_iods_pm_defect(&mut observations, &oracle));
    assert!(
        observations
            .iter()
            .all(|observation| observation.status == ValidatorStatus::Failed)
    );
}

#[test]
fn known_seg_table_defect_requires_a_sparse_oracle_and_exact_diagnostics() {
    let directory = tempdir().unwrap();
    let oracle = directory.path().join("oracle-seg.dcm");
    let rust = directory.path().join("rust-seg.dcm");
    let dense_oracle = directory.path().join("dense-oracle-seg.dcm");
    let dense_rust = directory.path().join("dense-rust-seg.dcm");
    let unrelated = directory.path().join("unrelated.dcm");
    for path in [&oracle, &rust, &dense_oracle, &dense_rust, &unrelated] {
        fs::write(path, b"dicom").unwrap();
    }
    let script = r#"
printf 'Using DICOM edition 2026c
Validating DICOM file %s
SOP class is "1.2.840.10008.5.1.4.1.1.66.4" (Segmentation IOD)

Errors
======

Module "Multi-frame Functional Groups":
(5200,9229) (Shared Functional Groups Sequence):
' "$1"
case "$1" in
  *dense*) printf '  Tag (0000,0000) (Pixel Measures Sequence) is unexpected
' ;;
  *)
    for name in 'Derivation Image Sequence' 'Pixel Measures Sequence'; do
      printf '  Tag (0000,0000) (%s) is unexpected
' "$name"
    done
    printf '(5200,9230) (Per-Frame Functional Groups Sequence):
'
    for name in 'Frame Content Sequence' 'Plane Position (Slide) Sequence' 'Segment Identification Sequence'; do
      printf '  Tag (0000,0000) (%s) is unexpected
' "$name"
    done
  ;;
esac
case "$1" in *unrelated*) printf 'Required attribute is missing
';; esac
exit 5
"#;
    let spec = ValidatorSpec {
        name: "validate_iods".to_owned(),
        command: vec![
            "/bin/sh".to_owned(),
            "-c".to_owned(),
            script.to_owned(),
            "synthetic-validator".to_owned(),
        ],
        version_command: Vec::new(),
        validation_args: Vec::new(),
        invocation: ValidatorInvocation::Each,
        edition: Some("2026c".to_owned()),
        unsupported_markers: Vec::new(),
    };
    let mut observations = run_validator(
        &spec,
        &[
            oracle.clone(),
            rust,
            dense_oracle.clone(),
            dense_rust,
            unrelated,
        ],
        Duration::from_secs(2),
    )
    .unwrap();

    assert!(qualify_validate_iods_seg_defect(&mut observations, &oracle));
    assert_eq!(
        observations[0].status,
        ValidatorStatus::KnownValidatorDefect
    );
    assert_eq!(
        observations[1].status,
        ValidatorStatus::KnownValidatorDefect
    );
    assert_eq!(observations[2].status, ValidatorStatus::Failed);
    assert_eq!(observations[3].status, ValidatorStatus::Failed);
    assert_eq!(observations[4].status, ValidatorStatus::Failed);

    assert!(qualify_validate_iods_seg_defect(
        &mut observations,
        &dense_oracle
    ));
    assert_eq!(
        observations[0].status,
        ValidatorStatus::KnownValidatorDefect
    );
    assert_eq!(
        observations[1].status,
        ValidatorStatus::KnownValidatorDefect
    );
    assert_eq!(
        observations[2].status,
        ValidatorStatus::KnownValidatorDefect
    );
    assert_eq!(
        observations[3].status,
        ValidatorStatus::KnownValidatorDefect
    );
    assert_eq!(observations[4].status, ValidatorStatus::Failed);
}

#[test]
fn tiled_segmentation_sr_defect_requires_matching_independent_controls() {
    let directory = tempdir().unwrap();
    let oracle = directory.path().join("oracle-sr.dcm");
    let rust = directory.path().join("rust-sr.dcm");
    let unrelated = directory.path().join("unrelated-sr.dcm");
    for path in [&oracle, &rust, &unrelated] {
        fs::write(path, b"dicom").unwrap();
    }
    let dciodvfy_script = r#"
printf 'Comprehensive3DSR\nError - Shall not be present when ReferencedFrameNumber is present - attribute <ReferencedSegmentNumber>\n'
case "$1" in *unrelated*) printf 'Error - Required attribute is missing\n';; esac
exit 0
"#;
    let validate_iods_script = r#"
printf 'Using DICOM edition 2026c\nValidating DICOM file %s\nSOP class is "1.2.840.10008.5.1.4.1.1.88.34" (Comprehensive 3D SR IOD)\n\nErrors\n======\n\nModule "SR Document Content":\n(0040,A730) (Content Sequence) / (0040,A730) (Content Sequence) / (0040,A730) (Content Sequence) / (0008,1199) (Referenced SOP Sequence):\n  Tag (0008,1160) (Referenced Frame Number) is unexpected\n  Tag (0062,000B) (Referenced Segment Number) is unexpected\n' "$1"
case "$1" in *unrelated*) printf '  Required attribute is missing\n';; esac
exit 2
"#;
    let mut observations = Vec::new();
    for (name, script) in [
        ("dciodvfy", dciodvfy_script),
        ("validate_iods", validate_iods_script),
    ] {
        let spec = ValidatorSpec {
            name: name.to_owned(),
            command: vec![
                "/bin/sh".to_owned(),
                "-c".to_owned(),
                script.to_owned(),
                "synthetic-validator".to_owned(),
            ],
            version_command: Vec::new(),
            validation_args: Vec::new(),
            invocation: ValidatorInvocation::Each,
            edition: Some("2026c".to_owned()),
            unsupported_markers: Vec::new(),
        };
        observations.extend(
            run_validator(
                &spec,
                &[oracle.clone(), rust.clone(), unrelated.clone()],
                Duration::from_secs(2),
            )
            .unwrap(),
        );
    }

    assert!(qualify_tiled_segmentation_sr_validator_defect(
        &mut observations,
        &oracle
    ));
    for observation in &observations {
        let is_unrelated = observation.files == [unrelated.to_string_lossy().as_ref()];
        assert_eq!(
            observation.status,
            if is_unrelated {
                ValidatorStatus::Failed
            } else {
                ValidatorStatus::KnownValidatorDefect
            }
        );
    }

    let mut without_oracle = observations
        .into_iter()
        .filter(|observation| observation.files != [oracle.to_string_lossy().as_ref()])
        .collect::<Vec<_>>();
    for observation in &mut without_oracle {
        if observation.status == ValidatorStatus::KnownValidatorDefect {
            observation.status = ValidatorStatus::Failed;
        }
    }
    assert!(!qualify_tiled_segmentation_sr_validator_defect(
        &mut without_oracle,
        &oracle
    ));
    assert!(
        without_oracle
            .iter()
            .all(|observation| observation.status == ValidatorStatus::Failed)
    );
}
