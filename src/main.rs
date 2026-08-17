use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use clap::{Args, Parser, Subcommand};
use serde::Serialize;
use serde_json::json;
use tempfile::TempDir;
use wsi_annotation_interop::probe::ViewerProbe;
use wsi_annotation_interop::profiles::{run_core_profile, run_full_profile};
use wsi_annotation_interop::shim::ReferenceShim;
use wsi_annotation_interop::validators::run_standard_validators;

#[derive(Parser)]
#[command(
    name = "wsi-annotation-interop",
    version,
    about = "Neutral DICOM WSI ANN/SEG/SR/PM interoperability harness"
)]
struct Cli {
    #[arg(long, global = true, default_value = ".venv/bin/python")]
    reference_python: PathBuf,
    #[arg(long, global = true, default_value = "shim/reference_shim.py")]
    reference_shim: PathBuf,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    QualifyPydcm {
        #[arg(long)]
        source: Option<PathBuf>,
        #[arg(long)]
        ann: Option<PathBuf>,
        #[arg(long)]
        seg: Option<PathBuf>,
    },
    GenerateFixtures {
        #[arg(long)]
        output: PathBuf,
    },
    Validate {
        #[arg(
            long = "dicom-edition",
            visible_alias = "edition",
            default_value = "2026c"
        )]
        edition: String,
        #[arg(long, default_value_t = 300.0)]
        timeout: f64,
        #[arg(required = true)]
        files: Vec<PathBuf>,
    },
    RunCore(ProfileArguments),
    RunFull(FullArguments),
}

#[derive(Args)]
struct ProfileArguments {
    #[arg(long)]
    probe: PathBuf,
    #[arg(long)]
    results: PathBuf,
    #[arg(long)]
    run_id: Option<String>,
}

#[derive(Args)]
struct FullArguments {
    #[command(flatten)]
    profile: ProfileArguments,
    #[arg(
        long = "dicom-edition",
        visible_alias = "edition",
        default_value = "2026c"
    )]
    edition: String,
    #[arg(long)]
    orthanc: Option<PathBuf>,
    #[arg(long = "orthanc-plugin")]
    orthanc_plugins: Vec<PathBuf>,
}

fn main() -> ExitCode {
    match execute(Cli::parse()) {
        Ok(code) => ExitCode::from(code),
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}

fn execute(cli: Cli) -> Result<u8, String> {
    let reference = ReferenceShim::new(
        vec![
            project_path(&cli.reference_python)
                .to_string_lossy()
                .into_owned(),
            project_path(&cli.reference_shim)
                .to_string_lossy()
                .into_owned(),
        ],
        Duration::from_mins(10),
    )?;
    match cli.command {
        Command::QualifyPydcm { source, ann, seg } => {
            let temporary;
            let (source, ann, seg) = match (source, ann, seg) {
                (Some(source), Some(ann), Some(seg)) => (source, ann, seg),
                (None, None, None) => {
                    temporary = TempDir::new()
                        .map_err(|error| format!("could not create fixture directory: {error}"))?;
                    let fixtures = reference
                        .generate_core(temporary.path())
                        .map_err(|error| error.to_string())?;
                    (
                        fixtures.source,
                        fixtures.ann["2D_VOLUME"].clone(),
                        fixtures.seg["BINARY"].clone(),
                    )
                }
                _ => {
                    return Err("--source, --ann, and --seg must be supplied together".to_owned());
                }
            };
            let qualification = reference
                .qualify_pydcm(&source, &ann, &seg)
                .map_err(|error| error.to_string())?;
            print_json(&qualification)?;
            Ok(0)
        }
        Command::GenerateFixtures { output } => {
            let fixtures = reference
                .generate_core(&output)
                .map_err(|error| error.to_string())?;
            print_json(&fixtures)?;
            Ok(0)
        }
        Command::Validate {
            edition,
            timeout,
            files,
        } => {
            if !timeout.is_finite() || timeout <= 0.0 {
                return Err("validator timeout must be positive and finite".to_owned());
            }
            let observations =
                run_standard_validators(&files, &edition, Duration::from_secs_f64(timeout))
                    .map_err(|error| error.to_string())?;
            let passed = observations
                .iter()
                .all(|observation| observation.status == "passed");
            print_json(&observations)?;
            Ok(u8::from(!passed))
        }
        Command::RunCore(arguments) => {
            let probe = viewer_probe(&arguments.probe)?;
            let run_id = arguments.run_id.map_or_else(default_run_id, Ok)?;
            let result = run_core_profile(&reference, &probe, &arguments.results, &run_id)?;
            print_json(&json!({
                "manifest": result.manifest,
                "status": if result.ok { "passed" } else { "failed" },
            }))?;
            Ok(u8::from(!result.ok))
        }
        Command::RunFull(arguments) => {
            let probe = viewer_probe(&arguments.profile.probe)?;
            let run_id = arguments.profile.run_id.map_or_else(default_run_id, Ok)?;
            let result = run_full_profile(
                &reference,
                &probe,
                &arguments.profile.results,
                &run_id,
                &arguments.edition,
                arguments.orthanc.as_deref(),
                &arguments.orthanc_plugins,
            )?;
            print_json(&json!({
                "manifest": result.manifest,
                "status": if result.ok { "passed" } else { "failed" },
            }))?;
            Ok(u8::from(!result.ok))
        }
    }
}

fn viewer_probe(path: &Path) -> Result<ViewerProbe, String> {
    ViewerProbe::new(
        vec![path.to_string_lossy().into_owned()],
        Some(Duration::from_mins(10)),
    )
}

fn project_path(path: &Path) -> PathBuf {
    if path.is_absolute() || path.exists() {
        path.to_path_buf()
    } else {
        Path::new(env!("CARGO_MANIFEST_DIR")).join(path)
    }
}

fn default_run_id() -> Result<String, String> {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| format!("system clock precedes Unix epoch: {error}"))?
        .as_secs();
    Ok(format!("run-{seconds}"))
}

fn print_json(value: &impl Serialize) -> Result<(), String> {
    println!(
        "{}",
        serde_json::to_string(value)
            .map_err(|error| format!("could not serialize command report: {error}"))?
    );
    Ok(())
}
