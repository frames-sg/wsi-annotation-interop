use std::fs::{self, File, OpenOptions};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use tempfile::TempDir;

use super::orthanc_configuration;
use crate::process::{ProcessError, run};

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
        let stdout_path = temporary.path().join("stdout.log");
        let stderr_path = temporary.path().join("stderr.log");
        let stdout = create_log(&stdout_path)?;
        let stderr = create_log(&stderr_path)?;
        let child = Command::new(&self.executable)
            .arg(&config_path)
            .stdout(Stdio::from(stdout))
            .stderr(Stdio::from(stderr))
            .spawn()
            .map_err(|error| format!("could not start Orthanc: {error}"))?;
        self.http_port = Some(http_port);
        self.child = Some(child);
        self.temporary = Some(temporary);
        if let Err(error) = self.wait_until_ready() {
            let _ = self.stop();
            return Err(error);
        }
        Ok(())
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
        if let Some(temporary) = self.temporary.take() {
            match read_log(&temporary.path().join("stdout.log")) {
                Ok(log) => self.stdout = log,
                Err(error) => {
                    failure.get_or_insert(error);
                }
            }
            match read_log(&temporary.path().join("stderr.log")) {
                Ok(log) => self.stderr = log,
                Err(error) => {
                    failure.get_or_insert(error);
                }
            }
            drop(temporary);
        }
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

    fn capture_version(&mut self) -> Result<(), String> {
        let command = vec![
            self.executable.to_string_lossy().into_owned(),
            "--version".to_owned(),
        ];
        match run(&command, Duration::from_secs(10)) {
            Ok(output) => {
                self.version_stdout = String::from_utf8_lossy(&output.stdout).into_owned();
                self.version_stderr = String::from_utf8_lossy(&output.stderr).into_owned();
                Ok(())
            }
            Err(ProcessError::TimedOut { .. }) => {
                Err("Orthanc --version timed out after 10 seconds".to_owned())
            }
            Err(error) => Err(format!("could not query Orthanc version: {error}")),
        }
    }

    fn wait_until_ready(&mut self) -> Result<(), String> {
        let deadline = Instant::now() + self.startup_timeout;
        let url = format!(
            "http://127.0.0.1:{}/system",
            self.http_port.expect("start assigned an HTTP port")
        );
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_millis(250)))
            .build()
            .into();
        while Instant::now() < deadline {
            if let Some(status) = self
                .child
                .as_mut()
                .expect("start assigned an Orthanc child")
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

fn create_log(path: &Path) -> Result<File, String> {
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| format!("could not create Orthanc log {}: {error}", path.display()))
}

fn read_log(path: &Path) -> Result<String, String> {
    fs::read(path)
        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
        .map_err(|error| format!("could not read Orthanc log {}: {error}", path.display()))
}
