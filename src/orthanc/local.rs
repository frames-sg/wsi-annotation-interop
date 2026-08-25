use std::fs::{self, File};
use std::io;
use std::net::TcpListener;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::thread;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use tempfile::TempDir;

use super::orthanc_configuration;
use crate::process::{CapturedStream, CommandSpec, ProcessError, join_reader, reader, run};

const ORTHANC_LOG_LIMIT_BYTES: usize = 4 * 1024 * 1024;
const ORTHANC_START_ATTEMPTS: usize = 3;
const ORTHANC_VERSION_ATTEMPTS: usize = 3;
const ORTHANC_VERSION_RETRY_DELAY: Duration = Duration::from_millis(25);

pub struct LocalOrthanc {
    executable: PathBuf,
    plugins: Vec<PathBuf>,
    startup_timeout: Duration,
    temporary: Option<TempDir>,
    child: Option<Child>,
    http_port: Option<u16>,
    version_stdout: String,
    version_stderr: String,
    stdout: String,
    stderr: String,
    stdout_reader: Option<JoinHandle<io::Result<CapturedStream>>>,
    stderr_reader: Option<JoinHandle<io::Result<CapturedStream>>>,
    stdout_truncated: bool,
    stderr_truncated: bool,
}

impl LocalOrthanc {
    /// Configure a user-supplied Orthanc process with isolated ephemeral state.
    ///
    /// # Errors
    ///
    /// Returns an error for a zero startup timeout.
    pub fn new(
        executable: PathBuf,
        plugins: Vec<PathBuf>,
        startup_timeout: Duration,
    ) -> Result<Self, String> {
        if startup_timeout.is_zero() {
            return Err("Orthanc startup timeout must be positive".to_owned());
        }
        Ok(Self {
            executable,
            plugins,
            startup_timeout,
            temporary: None,
            child: None,
            http_port: None,
            version_stdout: String::new(),
            version_stderr: String::new(),
            stdout: String::new(),
            stderr: String::new(),
            stdout_reader: None,
            stderr_reader: None,
            stdout_truncated: false,
            stderr_truncated: false,
        })
    }

    /// Start Orthanc and wait for its loopback REST API.
    ///
    /// # Errors
    ///
    /// Returns an error for missing inputs, process failures, unsafe configuration,
    /// premature exit, or startup timeout.
    pub fn start(&mut self) -> Result<(), String> {
        if self.child.is_some() {
            return Err("Orthanc is already running".to_owned());
        }
        if !self.executable.is_file() {
            return Err(format!(
                "Orthanc executable not found: {}",
                self.executable.display()
            ));
        }
        if let Some(plugin) = self.plugins.iter().find(|path| !path.exists()) {
            return Err(format!("Orthanc plugin not found: {}", plugin.display()));
        }
        self.capture_version()?;

        for attempt in 1..=ORTHANC_START_ATTEMPTS {
            self.clear_attempt_evidence();
            match self.start_attempt() {
                Ok(()) => return Ok(()),
                Err(error) => {
                    let stop_error = self.stop().err();
                    let port_collision = is_port_collision(&error, &self.stdout, &self.stderr);
                    if attempt < ORTHANC_START_ATTEMPTS && port_collision && stop_error.is_none() {
                        continue;
                    }
                    return match stop_error {
                        Some(stop_error) => Err(format!("{error}; cleanup failed: {stop_error}")),
                        None => Err(error),
                    };
                }
            }
        }
        Err("Orthanc startup attempts were exhausted".to_owned())
    }

    fn start_attempt(&mut self) -> Result<(), String> {
        let temporary = tempfile::Builder::new()
            .prefix("wsi-interop-orthanc-")
            .tempdir()
            .map_err(|error| format!("could not create Orthanc temporary directory: {error}"))?;
        let storage = temporary.path().join("storage");
        fs::create_dir(&storage)
            .map_err(|error| format!("could not create Orthanc storage: {error}"))?;
        let http_port = available_port()?;
        let configuration = orthanc_configuration(&storage, http_port, &self.plugins);
        let config_path = temporary.path().join("orthanc.json");
        let config = File::create(&config_path)
            .map_err(|error| format!("could not create Orthanc configuration: {error}"))?;
        serde_json::to_writer_pretty(config, &configuration)
            .map_err(|error| format!("could not write Orthanc configuration: {error}"))?;
        let mut child = Command::new(&self.executable)
            .arg(&config_path)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| format!("could not start Orthanc: {error}"))?;
        let Some(stdout) = child.stdout.take() else {
            let _ = child.kill();
            let _ = child.wait();
            return Err("could not capture Orthanc stdout".to_owned());
        };
        let Some(stderr) = child.stderr.take() else {
            let _ = child.kill();
            let _ = child.wait();
            return Err("could not capture Orthanc stderr".to_owned());
        };
        self.stdout_reader = Some(reader(stdout, ORTHANC_LOG_LIMIT_BYTES));
        self.stderr_reader = Some(reader(stderr, ORTHANC_LOG_LIMIT_BYTES));
        self.http_port = Some(http_port);
        self.child = Some(child);
        self.temporary = Some(temporary);
        self.wait_until_ready()
    }

    /// Return the active loopback-only `DICOMweb` root.
    ///
    /// # Errors
    ///
    /// Returns an error when Orthanc is not running.
    pub fn dicomweb_url(&self) -> Result<String, String> {
        self.http_port
            .map(|port| format!("http://127.0.0.1:{port}/dicom-web"))
            .ok_or_else(|| "Orthanc is not running".to_owned())
    }

    /// Stop Orthanc, capture its logs, and remove its isolated storage.
    ///
    /// # Errors
    ///
    /// Returns an error when termination or log capture fails.
    pub fn stop(&mut self) -> Result<(), String> {
        let mut failure = None;
        if let Some(mut child) = self.child.take() {
            if child
                .try_wait()
                .map_err(|error| format!("could not query Orthanc: {error}"))?
                .is_none()
                && let Err(error) = child.kill()
            {
                failure = Some(format!("could not terminate Orthanc: {error}"));
            }
            if let Err(error) = child.wait() {
                failure.get_or_insert_with(|| format!("could not wait for Orthanc: {error}"));
            }
        }
        self.capture_logs(&mut failure);
        drop(self.temporary.take());
        self.http_port = None;
        failure.map_or(Ok(()), Err)
    }

    #[must_use]
    pub fn version_stdout(&self) -> &str {
        &self.version_stdout
    }

    #[must_use]
    pub fn version_stderr(&self) -> &str {
        &self.version_stderr
    }

    #[must_use]
    pub fn stdout(&self) -> &str {
        &self.stdout
    }

    #[must_use]
    pub fn stderr(&self) -> &str {
        &self.stderr
    }

    #[must_use]
    pub const fn stdout_truncated(&self) -> bool {
        self.stdout_truncated
    }

    #[must_use]
    pub const fn stderr_truncated(&self) -> bool {
        self.stderr_truncated
    }

    fn capture_logs(&mut self, failure: &mut Option<String>) {
        if let Some(reader) = self.stdout_reader.take() {
            match join_reader(reader, "Orthanc stdout") {
                Ok(output) => {
                    self.stdout = String::from_utf8_lossy(&output.bytes).into_owned();
                    self.stdout_truncated = output.truncated;
                }
                Err(error) => {
                    failure.get_or_insert_with(|| error.to_string());
                }
            }
        }
        if let Some(reader) = self.stderr_reader.take() {
            match join_reader(reader, "Orthanc stderr") {
                Ok(output) => {
                    self.stderr = String::from_utf8_lossy(&output.bytes).into_owned();
                    self.stderr_truncated = output.truncated;
                }
                Err(error) => {
                    failure.get_or_insert_with(|| error.to_string());
                }
            }
        }
    }

    fn clear_attempt_evidence(&mut self) {
        self.stdout.clear();
        self.stderr.clear();
        self.stdout_truncated = false;
        self.stderr_truncated = false;
    }

    fn capture_version(&mut self) -> Result<(), String> {
        let command = CommandSpec::new(
            self.executable.as_os_str().to_owned(),
            vec!["--version".into()],
        )?;
        let process = command.process_spec(Duration::from_secs(10));
        for attempt in 1..=ORTHANC_VERSION_ATTEMPTS {
            match run(&process) {
                Ok(output) => {
                    self.version_stdout =
                        String::from_utf8_lossy(&output.stdout.bytes).into_owned();
                    self.version_stderr =
                        String::from_utf8_lossy(&output.stderr.bytes).into_owned();
                    return Ok(());
                }
                Err(ProcessError::Start(_)) if attempt < ORTHANC_VERSION_ATTEMPTS => {
                    thread::sleep(ORTHANC_VERSION_RETRY_DELAY);
                }
                Err(ProcessError::TimedOut { .. }) => {
                    return Err("Orthanc --version timed out after 10 seconds".to_owned());
                }
                Err(error) => return Err(format!("could not query Orthanc version: {error}")),
            }
        }
        unreachable!("Orthanc version attempts are nonzero")
    }

    fn wait_until_ready(&mut self) -> Result<(), String> {
        let deadline = Instant::now() + self.startup_timeout;
        let port = self
            .http_port
            .ok_or_else(|| "Orthanc startup has no assigned HTTP port".to_owned())?;
        let url = format!("http://127.0.0.1:{port}/system");
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_millis(250)))
            .build()
            .into();
        while Instant::now() < deadline {
            let child = self
                .child
                .as_mut()
                .ok_or_else(|| "Orthanc startup has no child process".to_owned())?;
            if let Some(status) = child
                .try_wait()
                .map_err(|error| format!("could not query Orthanc startup: {error}"))?
            {
                return Err(format!("Orthanc exited during startup with {status}"));
            }
            if agent.get(&url).call().is_ok() {
                return Ok(());
            }
            thread::sleep(Duration::from_millis(50));
        }
        Err("Orthanc did not become ready before the startup timeout".to_owned())
    }
}

impl Drop for LocalOrthanc {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

fn available_port() -> Result<u16, String> {
    TcpListener::bind(("127.0.0.1", 0))
        .and_then(|listener| listener.local_addr())
        .map(|address| address.port())
        .map_err(|error| format!("could not allocate a loopback port: {error}"))
}

fn is_port_collision(start_error: &str, stdout: &str, stderr: &str) -> bool {
    [start_error, stdout, stderr].iter().any(|text| {
        let lowercase = text.to_ascii_lowercase();
        lowercase.contains("address already in use")
            || lowercase.contains("failed to bind")
            || lowercase.contains("bind() failed")
    })
}
