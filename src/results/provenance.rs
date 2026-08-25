use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Serialize;

use crate::metrics::MetricLimits;
use crate::orthanc::DicomwebLimits;
use crate::process::{
    CommandSpec, DEFAULT_SAMPLE_INTERVAL, DEFAULT_STDERR_LIMIT_BYTES, DEFAULT_STDOUT_LIMIT_BYTES,
    run,
};

use super::manifest::sha256_file;

#[derive(Debug, Serialize)]
pub struct Provenance {
    pub schema_version: u8,
    pub repositories: RepositoryProvenance,
    pub build: BuildProvenance,
    pub study: StudyProvenance,
    pub ci: CiProvenance,
}

#[derive(Debug, Serialize)]
pub struct RepositoryProvenance {
    pub harness: GitIdentity,
    pub annotation_probe: GitIdentity,
}

#[derive(Debug, Serialize)]
pub struct GitIdentity {
    pub sha: Option<String>,
    pub dirty_tree: Option<bool>,
    pub branch: Option<String>,
    pub executable_sha256: Option<String>,
    pub unknown_reason: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct BuildProvenance {
    pub rustc_vv: Option<String>,
    pub cargo_version: Option<String>,
    pub target_triple: Option<String>,
    pub build_profile: String,
    pub rustflags: Option<String>,
    pub cargo_features: Vec<String>,
    pub cargo_lock_sha256: Option<String>,
    pub executable_sha256: Option<String>,
    pub python_version: Option<String>,
    pub uv_version: Option<String>,
    pub uv_lock_sha256: Option<String>,
    pub python_packages: BTreeMap<String, String>,
    pub python_packages_unknown_reason: Option<String>,
    pub operating_system: &'static str,
    pub architecture: &'static str,
}

#[derive(Debug, Serialize)]
pub struct StudyProvenance {
    pub profile: Option<String>,
    pub dicom_edition: Option<String>,
    pub matrix_schema_version: u8,
    pub probe_schema_version: u8,
    pub conversion_schema_version: u8,
    pub process_sample_interval_ms: u64,
    pub process_stdout_limit_bytes: usize,
    pub process_stderr_limit_bytes: usize,
    pub metric_max_crop_pixels: usize,
    pub stow_response_limit_bytes: u64,
    pub qido_response_limit_bytes: u64,
    pub wado_response_limit_bytes: u64,
    pub profile_definition_version: u32,
    pub validator_provenance_artifact: &'static str,
    pub orthanc_provenance_artifact: &'static str,
}

#[derive(Debug, Serialize)]
pub struct CiProvenance {
    pub run_id: Option<String>,
    pub run_attempt: Option<String>,
    pub runner_name: Option<String>,
    pub runner_environment: Option<String>,
}

/// Collect source, build, study, and CI identity without inventing unavailable values.
#[must_use]
pub fn collect_provenance(harness_root: &Path, annotation_probe: Option<&Path>) -> Provenance {
    let rustc_vv = command_text("rustc", ["-Vv"]);
    let target_triple = rustc_vv.as_deref().and_then(|output| {
        output
            .lines()
            .find_map(|line| line.strip_prefix("host: ").map(str::to_owned))
    });
    let current_executable = std::env::current_exe().ok();
    let python_version = command_text_os(
        "uv",
        [
            OsString::from("run"),
            OsString::from("--project"),
            harness_root.as_os_str().to_owned(),
            OsString::from("python"),
            OsString::from("--version"),
        ],
    );
    Provenance {
        schema_version: 1,
        repositories: RepositoryProvenance {
            harness: git_identity(harness_root, None),
            annotation_probe: annotation_probe.map_or_else(
                || GitIdentity::unknown("annotation_probe executable was not supplied"),
                |path| {
                    let root = find_git_root(path.parent().unwrap_or(path));
                    root.map_or_else(
                        || {
                            let mut identity = GitIdentity::unknown(
                                "annotation_probe executable is not inside a Git worktree",
                            );
                            identity.executable_sha256 = sha256_file(path).ok();
                            identity
                        },
                        |root| git_identity(&root, Some(path)),
                    )
                },
            ),
        },
        build: BuildProvenance {
            rustc_vv,
            cargo_version: command_text("cargo", ["-V"]),
            target_triple,
            build_profile: if cfg!(debug_assertions) {
                "debug".to_owned()
            } else {
                "release".to_owned()
            },
            rustflags: std::env::var("RUSTFLAGS").ok(),
            cargo_features: Vec::new(),
            cargo_lock_sha256: sha256_file(&harness_root.join("Cargo.lock")).ok(),
            executable_sha256: current_executable
                .as_deref()
                .and_then(|path| sha256_file(path).ok()),
            python_version,
            uv_version: command_text("uv", ["--version"]),
            uv_lock_sha256: sha256_file(&harness_root.join("uv.lock")).ok(),
            python_packages: BTreeMap::new(),
            python_packages_unknown_reason: Some(
                "package versions are retained in metadata.reference.packages".to_owned(),
            ),
            operating_system: std::env::consts::OS,
            architecture: std::env::consts::ARCH,
        },
        study: StudyProvenance {
            profile: None,
            dicom_edition: None,
            matrix_schema_version: 2,
            probe_schema_version: 1,
            conversion_schema_version: 1,
            process_sample_interval_ms: u64::try_from(DEFAULT_SAMPLE_INTERVAL.as_millis())
                .unwrap_or(u64::MAX),
            process_stdout_limit_bytes: DEFAULT_STDOUT_LIMIT_BYTES,
            process_stderr_limit_bytes: DEFAULT_STDERR_LIMIT_BYTES,
            metric_max_crop_pixels: MetricLimits::default().max_crop_pixels,
            stow_response_limit_bytes: DicomwebLimits::default().stow_response_bytes,
            qido_response_limit_bytes: DicomwebLimits::default().qido_response_bytes,
            wado_response_limit_bytes: DicomwebLimits::default().wado_response_bytes,
            profile_definition_version: 1,
            validator_provenance_artifact: "observations.jsonl",
            orthanc_provenance_artifact: "observations.jsonl",
        },
        ci: CiProvenance {
            run_id: std::env::var("GITHUB_RUN_ID").ok(),
            run_attempt: std::env::var("GITHUB_RUN_ATTEMPT").ok(),
            runner_name: std::env::var("RUNNER_NAME").ok(),
            runner_environment: std::env::var("RUNNER_ENVIRONMENT").ok(),
        },
    }
}

impl GitIdentity {
    fn unknown(reason: &str) -> Self {
        Self {
            sha: None,
            dirty_tree: None,
            branch: None,
            executable_sha256: None,
            unknown_reason: Some(reason.to_owned()),
        }
    }
}

fn git_identity(root: &Path, executable: Option<&Path>) -> GitIdentity {
    let sha = git_text(root, ["rev-parse", "HEAD"]);
    let Some(sha) = sha else {
        let mut identity = GitIdentity::unknown("Git identity could not be read for this path");
        identity.executable_sha256 = executable.and_then(|path| sha256_file(path).ok());
        return identity;
    };
    GitIdentity {
        sha: Some(sha),
        dirty_tree: git_text(root, ["status", "--porcelain=v1"]).map(|value| !value.is_empty()),
        branch: git_text(root, ["branch", "--show-current"]).filter(|value| !value.is_empty()),
        executable_sha256: executable.and_then(|path| sha256_file(path).ok()),
        unknown_reason: None,
    }
}

fn git_text<const N: usize>(root: &Path, arguments: [&str; N]) -> Option<String> {
    let mut args = vec![OsString::from("-C"), root.as_os_str().to_owned()];
    args.extend(arguments.into_iter().map(OsString::from));
    command_text_os("git", args)
}

fn find_git_root(start: &Path) -> Option<PathBuf> {
    start
        .ancestors()
        .find(|directory| directory.join(".git").exists())
        .map(Path::to_path_buf)
}

fn command_text<const N: usize>(program: &str, arguments: [&str; N]) -> Option<String> {
    command_text_os(program, arguments.into_iter().map(OsString::from))
}

fn command_text_os(program: &str, arguments: impl IntoIterator<Item = OsString>) -> Option<String> {
    let command = CommandSpec::new(program.into(), arguments.into_iter().collect()).ok()?;
    let output = run(&command.process_spec(Duration::from_secs(10))).ok()?;
    if !output.status.success() || output.stdout.truncated || output.stderr.truncated {
        return None;
    }
    let stdout = String::from_utf8(output.stdout.bytes).ok()?;
    let stderr = String::from_utf8(output.stderr.bytes).ok()?;
    let value = if stdout.trim().is_empty() {
        stderr
    } else {
        stdout
    };
    Some(value.trim().to_owned())
}
