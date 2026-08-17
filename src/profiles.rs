use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::{Value, json};

use crate::conversion_matrix::{ConversionMatrixResult, run_conversion_matrices};
use crate::matrix::run_core_matrix;
use crate::orthanc::{DicomwebObject, LocalOrthanc, verify_dicomweb_transport};
use crate::probe::ViewerProbe;
use crate::results::RunWriter;
use crate::scale::{default_scale_cases, run_scale_cases};
use crate::shim::{FixtureSet, ReferenceShim};
use crate::validators::{
    qualify_tiled_segmentation_sr_validator_defect, qualify_validate_iods_pm_defect,
    qualify_validate_iods_seg_defect, run_standard_validators,
};

#[derive(Debug)]
pub struct ProfileResult {
    pub manifest: PathBuf,
    pub ok: bool,
}

/// Run the required highdicom/viewer core profile and write immutable results.
///
/// # Errors
///
/// Returns an error for fixture, process, or artifact failures. Matrix failures
/// remain observations and set `ok` to false.
pub fn run_core_profile(
    reference: &ReferenceShim,
    probe: &ViewerProbe,
    results_root: &Path,
    run_id: &str,
) -> Result<ProfileResult, String> {
    let environment = reference.environment().map_err(|error| error.to_string())?;
    let mut writer = RunWriter::new(
        results_root,
        run_id,
        json!({"profile": "core", "reference": environment}),
    )
    .map_err(|error| error.to_string())?;
    let fixtures = reference
        .generate_core(&writer.path().join("fixtures"))
        .map_err(|error| error.to_string())?;
    let matrix = run_core_matrix(
        &fixtures,
        reference,
        probe,
        &writer.path().join("roundtrips"),
    )?;
    let conversion = run_conversion_matrices(
        &fixtures,
        reference,
        probe,
        &writer.path().join("conversions"),
    )?;
    let ok = matrix.is_ok() && conversion.is_ok();
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
    writer
        .write_observations(&observations)
        .map_err(|error| error.to_string())?;
    let manifest = writer.finalize().map_err(|error| error.to_string())?;
    Ok(ProfileResult { manifest, ok })
}

/// Run validators, scale cases, and local Orthanc transport in addition to core.
///
/// # Errors
///
/// Returns an error for fixture, process, configuration, or artifact failures.
/// Study-arm failures remain observations and set `ok` to false.
pub fn run_full_profile(
    reference: &ReferenceShim,
    probe: &ViewerProbe,
    results_root: &Path,
    run_id: &str,
    edition: &str,
    orthanc_executable: Option<&Path>,
    orthanc_plugins: &[PathBuf],
) -> Result<ProfileResult, String> {
    let environment = reference.environment().map_err(|error| error.to_string())?;
    let mut writer = RunWriter::new(
        results_root,
        run_id,
        json!({"profile": "full", "dicom_edition": edition, "reference": environment}),
    )
    .map_err(|error| error.to_string())?;
    let fixtures = reference
        .generate_core(&writer.path().join("fixtures"))
        .map_err(|error| error.to_string())?;
    let matrix = run_core_matrix(
        &fixtures,
        reference,
        probe,
        &writer.path().join("roundtrips"),
    )?;
    let matrix_ok = matrix.is_ok();
    let conversion = run_conversion_matrices(
        &fixtures,
        reference,
        probe,
        &writer.path().join("conversions"),
    )?;
    let conversion_ok = conversion.is_ok();
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

    let validator_files =
        validator_files(&fixtures, &writer.path().join("roundtrips"), &conversion)?;
    let mut validators = run_standard_validators(&validator_files, edition, Duration::from_mins(5))
        .map_err(|error| error.to_string())?;
    qualify_validate_iods_pm_defect(&mut validators, &fixtures.pm);
    for highdicom_seg in fixtures
        .seg
        .values()
        .chain(std::iter::once(&fixtures.reordered_seg))
    {
        qualify_validate_iods_seg_defect(&mut validators, highdicom_seg);
    }
    qualify_tiled_segmentation_sr_validator_defect(&mut validators, &fixtures.sr_seg);
    let validators_ok = validators.iter().all(|observation| {
        matches!(
            observation.status.as_str(),
            "passed" | "known_validator_defect"
        )
    });
    for (index, validator) in validators.iter().enumerate() {
        let mut observation = tagged("validator", validator)?;
        observation["case_id"] = json!(format!("validator-{}-{}", validator.name, index + 1));
        observations.push(observation);
    }

    let scale = run_scale_cases(
        reference,
        probe,
        &fixtures.source,
        &writer.path().join("scale"),
        &default_scale_cases(),
    )?;
    let required_scale: Vec<_> = scale
        .iter()
        .filter(|observation| observation.required)
        .collect();
    let scale_ok = !required_scale.is_empty()
        && required_scale
            .iter()
            .all(|observation| observation.status == "passed");
    observations.extend(
        scale
            .iter()
            .map(|observation| tagged("scale", observation))
            .collect::<Result<Vec<_>, _>>()?,
    );

    let (orthanc, orthanc_ok) = run_orthanc(
        reference,
        &fixtures,
        &conversion,
        orthanc_executable,
        orthanc_plugins,
        &writer.path().join("orthanc-retrieved"),
    )?;
    observations.extend(orthanc);
    writer
        .write_observations(&observations)
        .map_err(|error| error.to_string())?;
    let manifest = writer.finalize().map_err(|error| error.to_string())?;
    Ok(ProfileResult {
        manifest,
        ok: matrix_ok && conversion_ok && validators_ok && scale_ok && orthanc_ok,
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

fn validator_files(
    fixtures: &FixtureSet,
    roundtrips: &Path,
    conversion: &ConversionMatrixResult,
) -> Result<Vec<PathBuf>, String> {
    let mut files = vec![fixtures.source.clone(), fixtures.pyramid_source.clone()];
    files.extend(fixtures.ann.values().cloned());
    files.push(fixtures.pyramid_ann.clone());
    files.extend(fixtures.seg.values().cloned());
    files.push(fixtures.reordered_seg.clone());
    files.push(fixtures.sr.clone());
    files.push(fixtures.sr_seg.clone());
    files.push(fixtures.pm.clone());
    let mut rewritten = fs::read_dir(roundtrips)
        .map_err(|error| format!("could not read roundtrip directory: {error}"))?
        .map(|entry| {
            entry
                .map(|entry| entry.path())
                .map_err(|error| format!("could not read roundtrip artifact: {error}"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    rewritten.retain(|path| path.extension().and_then(|value| value.to_str()) == Some("dcm"));
    rewritten.sort();
    files.extend(rewritten);
    files.extend(
        conversion
            .observations
            .iter()
            .filter(|observation| observation.status == "passed")
            .flat_map(|observation| observation.output_paths.iter().cloned()),
    );
    Ok(files)
}

enum TransportKind {
    Wsi,
    Ann {
        source: PathBuf,
        canonical: Option<PathBuf>,
    },
    Seg {
        source: PathBuf,
    },
    Sr,
    Pm,
}

struct TransportSpec {
    object: DicomwebObject,
    kind: TransportKind,
}

fn run_orthanc(
    reference: &ReferenceShim,
    fixtures: &FixtureSet,
    conversion: &ConversionMatrixResult,
    executable: Option<&Path>,
    plugins: &[PathBuf],
    retrieval_directory: &Path,
) -> Result<(Vec<Value>, bool), String> {
    let Some(executable) = executable else {
        return Ok((
            vec![json!({
                "phase": "orthanc",
                "case_id": "orthanc-runtime",
                "status": "unavailable",
                "message": "no local Orthanc executable was supplied",
            })],
            false,
        ));
    };
    let started = Instant::now();
    let mut runner = LocalOrthanc::new(
        executable.to_path_buf(),
        plugins.to_vec(),
        Duration::from_secs(30),
    )?;
    if let Err(error) = runner.start() {
        return Ok((
            vec![orthanc_runtime(&runner, "failed", started, &error)],
            false,
        ));
    }
    let specs = transport_specs(reference, fixtures, conversion)?;
    let objects: Vec<_> = specs.iter().map(|spec| spec.object.clone()).collect();
    let transport = verify_dicomweb_transport(
        &runner.dicomweb_url()?,
        &objects,
        retrieval_directory,
        |index, path| normalize_transport(reference, &specs[index].kind, path),
    );
    let stop = runner.stop();
    let mut observations = Vec::new();
    match (transport, stop) {
        (Ok(transport), Ok(())) => {
            observations.push(orthanc_runtime(&runner, "passed", started, ""));
            let ok = transport.is_ok();
            for item in &transport.observations {
                let mut observation = tagged("orthanc", item)?;
                observation["case_id"] = json!(format!("orthanc-{}", item.sop_instance_uid));
                observation["status"] =
                    json!(
                        if item.stow && item.qido && item.wado && item.semantic_equal {
                            "passed"
                        } else {
                            "failed"
                        }
                    );
                observations.push(observation);
            }
            Ok((observations, ok))
        }
        (Err(error), stop) => {
            let message = match stop {
                Ok(()) => error,
                Err(stop) => format!("{error}; {stop}"),
            };
            observations.push(orthanc_runtime(&runner, "failed", started, &message));
            Ok((observations, false))
        }
        (Ok(_), Err(error)) => {
            observations.push(orthanc_runtime(&runner, "failed", started, &error));
            Ok((observations, false))
        }
    }
}

fn transport_specs(
    reference: &ReferenceShim,
    fixtures: &FixtureSet,
    conversion: &ConversionMatrixResult,
) -> Result<Vec<TransportSpec>, String> {
    let mut specs = vec![
        transport_spec(reference, &fixtures.source, TransportKind::Wsi)?,
        transport_spec(reference, &fixtures.pyramid_source, TransportKind::Wsi)?,
    ];
    for path in fixtures.ann.values() {
        specs.push(transport_spec(
            reference,
            path,
            TransportKind::Ann {
                source: fixtures.source.clone(),
                canonical: None,
            },
        )?);
    }
    specs.push(transport_spec(
        reference,
        &fixtures.pyramid_ann,
        TransportKind::Ann {
            source: fixtures.pyramid_source.clone(),
            canonical: Some(fixtures.source.clone()),
        },
    )?);
    for path in fixtures
        .seg
        .values()
        .chain(std::iter::once(&fixtures.reordered_seg))
    {
        specs.push(transport_spec(
            reference,
            path,
            TransportKind::Seg {
                source: fixtures.source.clone(),
            },
        )?);
    }
    specs.push(transport_spec(reference, &fixtures.sr, TransportKind::Sr)?);
    specs.push(transport_spec(
        reference,
        &fixtures.sr_seg,
        TransportKind::Sr,
    )?);
    specs.push(transport_spec(reference, &fixtures.pm, TransportKind::Pm)?);
    for observation in &conversion.observations {
        if observation.status != "passed" {
            continue;
        }
        for path in &observation.output_paths {
            let kind = match observation.target.as_str() {
                "ann" => TransportKind::Ann {
                    source: fixtures.source.clone(),
                    canonical: None,
                },
                "seg" => TransportKind::Seg {
                    source: fixtures.source.clone(),
                },
                "sr" => TransportKind::Sr,
                "pm" => TransportKind::Pm,
                target => return Err(format!("unsupported conversion transport target {target}")),
            };
            specs.push(transport_spec(reference, path, kind)?);
        }
    }
    Ok(specs)
}

fn transport_spec(
    reference: &ReferenceShim,
    path: &Path,
    kind: TransportKind,
) -> Result<TransportSpec, String> {
    let metadata = reference
        .metadata(path)
        .map_err(|error| error.to_string())?;
    Ok(TransportSpec {
        object: DicomwebObject {
            path: path.to_path_buf(),
            study_instance_uid: metadata.study_instance_uid,
            series_instance_uid: metadata.series_instance_uid,
            sop_instance_uid: metadata.sop_instance_uid,
        },
        kind,
    })
}

fn normalize_transport(
    reference: &ReferenceShim,
    kind: &TransportKind,
    path: &Path,
) -> Result<Value, String> {
    match kind {
        TransportKind::Wsi => reference.normalize_wsi(path),
        TransportKind::Ann { source, canonical } => {
            reference.normalize_ann(path, source, canonical.as_deref())
        }
        TransportKind::Seg { source } => reference.normalize_seg(path, source),
        TransportKind::Sr => reference.normalize_sr(path),
        TransportKind::Pm => reference.normalize_pm(path),
    }
    .map_err(|error| error.to_string())
}

fn orthanc_runtime(runner: &LocalOrthanc, status: &str, started: Instant, message: &str) -> Value {
    json!({
        "phase": "orthanc",
        "case_id": "orthanc-runtime",
        "status": status,
        "runtime_ms": started.elapsed().as_secs_f64() * 1000.0,
        "version_stdout": runner.version_stdout(),
        "version_stderr": runner.version_stderr(),
        "stdout": runner.stdout(),
        "stderr": runner.stderr(),
        "message": message,
    })
}

fn tagged(phase: &str, value: &impl Serialize) -> Result<Value, String> {
    let mut value = serde_json::to_value(value)
        .map_err(|error| format!("could not serialize {phase} observation: {error}"))?;
    let object = value
        .as_object_mut()
        .ok_or_else(|| format!("{phase} observation must be an object"))?;
    object.insert("phase".to_owned(), json!(phase));
    Ok(value)
}
