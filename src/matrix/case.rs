use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use serde_json::Value;

use crate::compare::{ComparisonResult, compare_ann, compare_seg};
use crate::ground_truth::build_core_ground_truth;
use crate::probe::ViewerProbe;
use crate::shim::{FixtureSet, ReferenceShim};

use super::phases::run_case;
use super::{CoreMatrixResult, MILLIMETER_TOLERANCE, MatrixObservation, PIXEL_TOLERANCE};

pub(super) struct Case<'a> {
    pub(super) id: &'a str,
    pub(super) expected: &'a Value,
    pub(super) path: &'a Path,
    pub(super) source: &'a Path,
}

pub(super) struct MatrixServices<'a> {
    pub(super) reference: &'a ReferenceShim,
    pub(super) probe: &'a ViewerProbe,
    pub(super) output_directory: &'a Path,
}

/// Run the independent-reference → viewer → independent-reference core matrix.
///
/// # Errors
///
/// Returns an error when the declarative oracle or output directory is invalid.
/// Individual case failures are retained as observations.
pub fn run_core_matrix(
    fixtures: &FixtureSet,
    reference: &ReferenceShim,
    probe: &ViewerProbe,
    output_directory: &Path,
) -> Result<CoreMatrixResult, String> {
    fs::create_dir_all(output_directory)
        .map_err(|error| format!("could not create matrix output directory: {error}"))?;
    let truth = load_ground_truth(&fixtures.ground_truth)?;
    let cases = truth
        .get("cases")
        .and_then(Value::as_object)
        .ok_or_else(|| "ground truth cases must be an object".to_owned())?;
    let services = MatrixServices {
        reference,
        probe,
        output_directory,
    };
    let mut observations = run_ann_matrix(fixtures, cases, &services)?;
    observations.extend(run_seg_matrix(fixtures, cases, &services)?);
    Ok(CoreMatrixResult { observations })
}

fn run_ann_matrix(
    fixtures: &FixtureSet,
    cases: &serde_json::Map<String, Value>,
    services: &MatrixServices<'_>,
) -> Result<Vec<MatrixObservation>, String> {
    let mut observations = Vec::with_capacity(5);
    for (form, case_id) in [
        ("2D_VOLUME", "ann-2d-volume"),
        ("2D_FRAME", "ann-2d-frame"),
        ("3D_COMMON_Z", "ann-3d-common-z"),
        ("3D_XYZ", "ann-3d-xyz"),
    ] {
        let path = fixture(&fixtures.ann, form)?;
        observations.push(run_ann(
            &Case {
                id: case_id,
                expected: required_case(cases, case_id)?,
                path,
                source: &fixtures.source,
            },
            services,
            None,
            0,
        ));
    }

    let pyramid_case = "ann-2d-volume-level1";
    observations.push(run_ann(
        &Case {
            id: pyramid_case,
            expected: required_case(cases, pyramid_case)?,
            path: &fixtures.pyramid_ann,
            source: &fixtures.pyramid_source,
        },
        services,
        Some(&fixtures.source),
        1,
    ));

    Ok(observations)
}

fn run_seg_matrix(
    fixtures: &FixtureSet,
    cases: &serde_json::Map<String, Value>,
    services: &MatrixServices<'_>,
) -> Result<Vec<MatrixObservation>, String> {
    let mut observations = Vec::with_capacity(4);
    for (kind, case_id) in [
        ("BINARY", "seg-binary"),
        ("LABELMAP", "seg-labelmap"),
        ("FRACTIONAL", "seg-fractional"),
    ] {
        let path = fixture(&fixtures.seg, kind)?;
        observations.push(run_seg(
            &Case {
                id: case_id,
                expected: required_case(cases, case_id)?,
                path,
                source: &fixtures.source,
            },
            services,
            kind,
        ));
    }

    let reordered_case = "seg-binary-reordered";
    observations.push(run_seg(
        &Case {
            id: reordered_case,
            expected: required_case(cases, reordered_case)?,
            path: &fixtures.reordered_seg,
            source: &fixtures.source,
        },
        services,
        "BINARY",
    ));

    Ok(observations)
}

fn load_ground_truth(path: &Path) -> Result<Value, String> {
    let data = fs::read(path)
        .map_err(|error| format!("could not read ground truth {}: {error}", path.display()))?;
    let truth: Value = serde_json::from_slice(&data)
        .map_err(|error| format!("ground truth is invalid JSON: {error}"))?;
    if truth != build_core_ground_truth() {
        return Err("generated ground truth differs from the Rust declarative oracle".to_owned());
    }
    Ok(truth)
}

fn fixture<'a>(
    fixtures: &'a BTreeMap<String, std::path::PathBuf>,
    key: &str,
) -> Result<&'a Path, String> {
    fixtures
        .get(key)
        .map(std::path::PathBuf::as_path)
        .ok_or_else(|| format!("core fixture {key} is missing"))
}

fn required_case<'a>(
    cases: &'a serde_json::Map<String, Value>,
    case_id: &str,
) -> Result<&'a Value, String> {
    cases
        .get(case_id)
        .ok_or_else(|| format!("ground truth case {case_id} is missing"))
}

pub(super) fn run_ann(
    case: &Case<'_>,
    services: &MatrixServices<'_>,
    canonical_source: Option<&Path>,
    pyramid_level: u8,
) -> MatrixObservation {
    run_case(
        case,
        services,
        CaseDomain::Ann {
            canonical_source,
            pyramid_level,
        },
    )
}

fn run_seg(case: &Case<'_>, services: &MatrixServices<'_>, kind: &str) -> MatrixObservation {
    run_case(
        case,
        services,
        CaseDomain::Seg {
            rewrite: if kind == "BINARY" {
                RewritePolicy::Required
            } else {
                RewritePolicy::RejectUnsupported
            },
        },
    )
}

#[derive(Clone, Copy)]
pub(super) enum RewritePolicy {
    Required,
    RejectUnsupported,
}

#[derive(Clone, Copy)]
pub(super) enum CaseDomain<'a> {
    Ann {
        canonical_source: Option<&'a Path>,
        pyramid_level: u8,
    },
    Seg {
        rewrite: RewritePolicy,
    },
}

impl<'a> CaseDomain<'a> {
    pub(super) const fn canonical_source(self) -> Option<&'a Path> {
        match self {
            Self::Ann {
                canonical_source, ..
            } => canonical_source,
            Self::Seg { .. } => None,
        }
    }

    pub(super) const fn pyramid_level(self) -> u8 {
        match self {
            Self::Ann { pyramid_level, .. } => pyramid_level,
            Self::Seg { .. } => 0,
        }
    }

    pub(super) fn normalize(
        self,
        reference: &ReferenceShim,
        annotation: &Path,
        source: &Path,
    ) -> Result<Value, String> {
        match self {
            Self::Ann {
                canonical_source, ..
            } => reference
                .normalize_ann(annotation, source, canonical_source)
                .map_err(|error| error.to_string()),
            Self::Seg { .. } => reference
                .normalize_seg(annotation, source)
                .map_err(|error| error.to_string()),
        }
    }

    pub(super) fn compare(
        self,
        expected: &Value,
        actual: &Value,
    ) -> Result<ComparisonResult, String> {
        match self {
            Self::Ann { .. } => {
                compare_ann(expected, actual, PIXEL_TOLERANCE, MILLIMETER_TOLERANCE)
            }
            Self::Seg { .. } => compare_seg(expected, actual),
        }
    }

    pub(super) const fn rewrite_policy(self) -> RewritePolicy {
        match self {
            Self::Ann { .. } => RewritePolicy::Required,
            Self::Seg { rewrite } => rewrite,
        }
    }
}
