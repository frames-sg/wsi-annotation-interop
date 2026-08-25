use std::ffi::OsString;
use std::fmt;
use std::io::{self, Read};
use std::process::{Command, ExitStatus, Stdio};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, Signal, System};

pub(crate) const DEFAULT_SAMPLE_INTERVAL: Duration = Duration::from_millis(100);
pub(crate) const DEFAULT_STDOUT_LIMIT_BYTES: usize = 16 * 1024 * 1024;
pub(crate) const DEFAULT_STDERR_LIMIT_BYTES: usize = 4 * 1024 * 1024;

#[derive(Debug, Clone)]
pub(crate) struct ProcessSpec {
    pub program: OsString,
    pub args: Vec<OsString>,
    pub timeout: Duration,
    pub sample_interval: Option<Duration>,
    pub stdout_limit_bytes: usize,
    pub stderr_limit_bytes: usize,
}

#[derive(Debug, Clone)]
pub(crate) struct CommandSpec {
    program: OsString,
    args: Vec<OsString>,
}

impl CommandSpec {
    pub fn from_strings(command: Vec<String>, label: &str) -> Result<Self, String> {
        let mut command = command.into_iter();
        let program = command
            .next()
            .ok_or_else(|| format!("{label} command must not be empty"))?;
        if program.is_empty() {
            return Err(format!("{label} program must not be empty"));
        }
        Ok(Self {
            program: program.into(),
            args: command.map(Into::into).collect(),
        })
    }

    pub fn new(program: OsString, args: Vec<OsString>) -> Result<Self, String> {
        if program.is_empty() {
            return Err("process program must not be empty".to_owned());
        }
        Ok(Self { program, args })
    }

    pub fn push(&mut self, argument: impl Into<OsString>) {
        self.args.push(argument.into());
    }

    pub fn extend<I, S>(&mut self, arguments: I)
    where
        I: IntoIterator<Item = S>,
        S: Into<OsString>,
    {
        self.args.extend(arguments.into_iter().map(Into::into));
    }

    pub fn display(&self) -> Vec<String> {
        std::iter::once(&self.program)
            .chain(&self.args)
            .map(|part| part.to_string_lossy().into_owned())
            .collect()
    }

    pub fn process_spec(&self, timeout: Duration) -> ProcessSpec {
        ProcessSpec {
            program: self.program.clone(),
            args: self.args.clone(),
            timeout,
            sample_interval: Some(DEFAULT_SAMPLE_INTERVAL),
            stdout_limit_bytes: DEFAULT_STDOUT_LIMIT_BYTES,
            stderr_limit_bytes: DEFAULT_STDERR_LIMIT_BYTES,
        }
    }

    pub fn program(&self) -> &std::ffi::OsStr {
        &self.program
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct CapturedStream {
    pub bytes: Vec<u8>,
    pub total_bytes: u64,
    pub truncated: bool,
}

#[derive(Debug)]
pub(crate) struct ProcessOutput {
    pub status: ExitStatus,
    pub stdout: CapturedStream,
    pub stderr: CapturedStream,
    pub elapsed: Duration,
    pub peak_rss_bytes: u64,
    pub rss_sampled: bool,
    pub sample_interval: Option<Duration>,
}

#[derive(Debug)]
pub(crate) enum ProcessError {
    InvalidConfiguration(String),
    Start(io::Error),
    Wait(io::Error),
    PipeUnavailable(&'static str),
    Read {
        stream: &'static str,
        source: io::Error,
    },
    ReaderPanicked(&'static str),
    TimedOut {
        timeout: Duration,
        output: Box<PartialProcessOutput>,
    },
}

#[derive(Debug)]
pub(crate) struct PartialProcessOutput {
    pub stdout: CapturedStream,
    pub stderr: CapturedStream,
    pub elapsed: Duration,
    pub peak_rss_bytes: u64,
    pub rss_sampled: bool,
    pub sample_interval: Option<Duration>,
}

impl fmt::Display for ProcessError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidConfiguration(message) => {
                write!(formatter, "invalid process configuration: {message}")
            }
            Self::Start(error) => write!(formatter, "could not start process: {error}"),
            Self::Wait(error) => write!(formatter, "could not wait for process: {error}"),
            Self::PipeUnavailable(stream) => {
                write!(formatter, "piped process {stream} was unavailable")
            }
            Self::Read { stream, source } => {
                write!(formatter, "could not read process {stream}: {source}")
            }
            Self::ReaderPanicked(stream) => {
                write!(formatter, "process {stream} reader panicked")
            }
            Self::TimedOut { timeout, .. } => {
                write!(
                    formatter,
                    "process timed out after {} seconds",
                    timeout.as_secs_f64()
                )
            }
        }
    }
}

impl std::error::Error for ProcessError {}

pub(crate) fn run(spec: &ProcessSpec) -> Result<ProcessOutput, ProcessError> {
    validate_spec(spec)?;
    let mut child = Command::new(&spec.program)
        .args(&spec.args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(ProcessError::Start)?;
    let root = Pid::from_u32(child.id());
    let stdout = child
        .stdout
        .take()
        .ok_or(ProcessError::PipeUnavailable("stdout"));
    let stderr = child
        .stderr
        .take()
        .ok_or(ProcessError::PipeUnavailable("stderr"));
    let (stdout, stderr) = match (stdout, stderr) {
        (Ok(stdout), Ok(stderr)) => (
            reader(stdout, spec.stdout_limit_bytes),
            reader(stderr, spec.stderr_limit_bytes),
        ),
        (Err(error), _) | (_, Err(error)) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error);
        }
    };
    let started = Instant::now();
    let mut sampler = Sampler::new(root, spec.sample_interval);

    let status = loop {
        sampler.sample();
        if let Some(status) = child.try_wait().map_err(ProcessError::Wait)? {
            break status;
        }
        if started.elapsed() >= spec.timeout {
            terminate_process_tree(sampler.system_mut(), root);
            let _ = child.kill();
            let _ = child.wait();
            let stdout = join_reader(stdout, "stdout")?;
            let stderr = join_reader(stderr, "stderr")?;
            return Err(ProcessError::TimedOut {
                timeout: spec.timeout,
                output: Box::new(PartialProcessOutput {
                    stdout,
                    stderr,
                    elapsed: started.elapsed(),
                    peak_rss_bytes: sampler.peak_rss_bytes,
                    rss_sampled: sampler.enabled(),
                    sample_interval: spec.sample_interval,
                }),
            });
        }
        // Waiting is cheap and independent from RSS sampling. Keeping a modest 10 ms wait poll
        // avoids turning a 100 ms memory interval into 100 ms of exit-detection latency.
        thread::sleep(
            Duration::from_millis(10).min(spec.timeout.saturating_sub(started.elapsed())),
        );
    };
    sampler.sample();
    Ok(ProcessOutput {
        status,
        stdout: join_reader(stdout, "stdout")?,
        stderr: join_reader(stderr, "stderr")?,
        elapsed: started.elapsed(),
        peak_rss_bytes: sampler.peak_rss_bytes,
        rss_sampled: sampler.enabled(),
        sample_interval: spec.sample_interval,
    })
}

fn validate_spec(spec: &ProcessSpec) -> Result<(), ProcessError> {
    if spec.program.is_empty() {
        return Err(ProcessError::InvalidConfiguration(
            "program must not be empty".to_owned(),
        ));
    }
    if spec.timeout.is_zero() {
        return Err(ProcessError::InvalidConfiguration(
            "timeout must be positive".to_owned(),
        ));
    }
    if spec.sample_interval == Some(Duration::ZERO) {
        return Err(ProcessError::InvalidConfiguration(
            "sampling interval must be positive when enabled".to_owned(),
        ));
    }
    Ok(())
}

pub(crate) fn reader(
    mut stream: impl Read + Send + 'static,
    limit: usize,
) -> JoinHandle<io::Result<CapturedStream>> {
    thread::spawn(move || {
        let mut output = CapturedStream {
            bytes: Vec::with_capacity(limit.min(8 * 1024)),
            ..CapturedStream::default()
        };
        let mut buffer = [0_u8; 8 * 1024];
        loop {
            let count = stream.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            output.total_bytes = output
                .total_bytes
                .saturating_add(u64::try_from(count).unwrap_or(u64::MAX));
            let retained = limit.saturating_sub(output.bytes.len()).min(count);
            output.bytes.extend_from_slice(&buffer[..retained]);
            output.truncated |= retained < count;
        }
        Ok(output)
    })
}

pub(crate) fn join_reader(
    reader: JoinHandle<io::Result<CapturedStream>>,
    stream: &'static str,
) -> Result<CapturedStream, ProcessError> {
    reader
        .join()
        .map_err(|_| ProcessError::ReaderPanicked(stream))?
        .map_err(|source| ProcessError::Read { stream, source })
}

struct Sampler {
    system: System,
    tracked: Vec<Pid>,
    interval: Option<Duration>,
    last_sample: Option<Instant>,
    peak_rss_bytes: u64,
}

impl Sampler {
    fn new(root: Pid, interval: Option<Duration>) -> Self {
        let mut sampler = Self {
            system: System::new(),
            tracked: vec![root],
            interval,
            last_sample: None,
            peak_rss_bytes: 0,
        };
        if interval.is_some() {
            // One initial inventory discovers children already created during process startup.
            // Later samples refresh only these known PIDs; timeout cleanup takes one final inventory.
            sampler.system.refresh_processes_specifics(
                ProcessesToUpdate::All,
                true,
                ProcessRefreshKind::nothing().with_memory().without_tasks(),
            );
            sampler.tracked = sampler
                .system
                .processes()
                .keys()
                .copied()
                .filter(|pid| is_descendant(&sampler.system, *pid, root))
                .collect();
            if !sampler.tracked.contains(&root) {
                sampler.tracked.push(root);
            }
            sampler.record_rss();
            sampler.last_sample = Some(Instant::now());
        }
        sampler
    }

    fn enabled(&self) -> bool {
        self.interval.is_some()
    }

    fn sample(&mut self) {
        let Some(interval) = self.interval else {
            return;
        };
        if self
            .last_sample
            .is_some_and(|last_sample| last_sample.elapsed() < interval)
        {
            return;
        }
        self.system.refresh_processes_specifics(
            ProcessesToUpdate::Some(&self.tracked),
            true,
            ProcessRefreshKind::nothing().with_memory().without_tasks(),
        );
        self.record_rss();
        self.last_sample = Some(Instant::now());
    }

    fn record_rss(&mut self) {
        let total = self.tracked.iter().fold(0_u64, |total, pid| {
            total.saturating_add(
                self.system
                    .process(*pid)
                    .map_or(0, sysinfo::Process::memory),
            )
        });
        self.peak_rss_bytes = self.peak_rss_bytes.max(total);
    }

    fn system_mut(&mut self) -> &mut System {
        &mut self.system
    }
}

fn is_descendant(system: &System, mut candidate: Pid, root: Pid) -> bool {
    for _ in 0..=system.processes().len() {
        if candidate == root {
            return true;
        }
        let Some(parent) = system.process(candidate).and_then(sysinfo::Process::parent) else {
            return false;
        };
        candidate = parent;
    }
    false
}

fn terminate_process_tree(system: &mut System, root: Pid) {
    system.refresh_processes(ProcessesToUpdate::All, true);
    let descendants: Vec<_> = system
        .processes()
        .keys()
        .copied()
        .filter(|pid| *pid != root && is_descendant(system, *pid, root))
        .collect();
    for pid in descendants.into_iter().rev() {
        if let Some(process) = system.process(pid) {
            let _ = process.kill_with(Signal::Kill);
        }
    }
    if let Some(process) = system.process(root) {
        let _ = process.kill_with(Signal::Kill);
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::io::{self, Read};
    use std::time::Duration;

    use super::{CapturedStream, ProcessError, ProcessSpec, reader, run};

    fn shell(script: &str) -> ProcessSpec {
        ProcessSpec {
            program: OsString::from("/bin/sh"),
            args: vec![OsString::from("-c"), OsString::from(script)],
            timeout: Duration::from_secs(2),
            sample_interval: Some(Duration::from_millis(100)),
            stdout_limit_bytes: 64,
            stderr_limit_bytes: 64,
        }
    }

    #[test]
    fn rejects_empty_program_and_zero_timeout() {
        let mut empty = shell("true");
        empty.program = OsString::new();
        assert!(matches!(
            run(&empty),
            Err(ProcessError::InvalidConfiguration(_))
        ));

        let mut zero = shell("true");
        zero.timeout = Duration::ZERO;
        assert!(matches!(
            run(&zero),
            Err(ProcessError::InvalidConfiguration(_))
        ));
    }

    #[test]
    fn normal_process_records_output_timing_and_sampling_metadata() {
        let output = run(&shell("printf stdout; printf stderr >&2")).expect("process succeeds");
        assert!(output.status.success());
        assert_eq!(output.stdout.bytes, b"stdout");
        assert_eq!(output.stderr.bytes, b"stderr");
        assert!(output.elapsed > Duration::ZERO);
        assert!(output.rss_sampled);
        assert_eq!(output.sample_interval, Some(Duration::from_millis(100)));
    }

    #[test]
    fn disabled_sampling_is_explicit() {
        let mut spec = shell("true");
        spec.sample_interval = None;
        let output = run(&spec).expect("process succeeds");
        assert!(!output.rss_sampled);
        assert_eq!(output.peak_rss_bytes, 0);
        assert_eq!(output.sample_interval, None);
    }

    #[test]
    fn output_is_drained_but_retained_bytes_are_bounded() {
        let output = run(&shell(
            "printf 'abcdefghijklmnopqrstuvwxyz'; printf '0123456789' >&2",
        ))
        .expect("process succeeds");
        assert_eq!(output.stdout.bytes, b"abcdefghijklmnopqrstuvwxyz");
        assert_eq!(output.stdout.total_bytes, 26);
        assert!(!output.stdout.truncated);

        let mut capped = shell("printf 'abcdefghijklmnopqrstuvwxyz'; printf '0123456789' >&2");
        capped.stdout_limit_bytes = 8;
        capped.stderr_limit_bytes = 4;
        let output = run(&capped).expect("process succeeds with truncated diagnostics");
        assert_eq!(output.stdout.bytes, b"abcdefgh");
        assert_eq!(output.stdout.total_bytes, 26);
        assert!(output.stdout.truncated);
        assert_eq!(output.stderr.bytes, b"0123");
        assert_eq!(output.stderr.total_bytes, 10);
        assert!(output.stderr.truncated);
    }

    #[test]
    fn timeout_retains_bounded_stream_evidence() {
        let mut spec = shell("printf 'before-timeout'; printf 'error-before-timeout' >&2; sleep 2");
        spec.timeout = Duration::from_millis(40);
        spec.sample_interval = None;
        spec.stdout_limit_bytes = 6;
        spec.stderr_limit_bytes = 5;
        let ProcessError::TimedOut { output, .. } = run(&spec).expect_err("must time out") else {
            panic!("expected timeout");
        };
        assert_eq!(output.stdout.bytes, b"before");
        assert!(output.stdout.truncated);
        assert_eq!(output.stderr.bytes, b"error");
        assert!(output.stderr.truncated);
    }

    #[test]
    fn process_can_close_stdout_before_it_exits() {
        let output = run(&shell("exec 1>&-; sleep 0.02; printf done >&2"))
            .expect("early stdout close is valid");
        assert!(output.status.success());
        assert_eq!(output.stdout, CapturedStream::default());
        assert_eq!(output.stderr.bytes, b"done");
    }

    #[test]
    fn timeout_terminates_a_spawned_descendant() {
        let mut spec = shell("sleep 60 & child=$!; printf '%s' \"$child\"; wait");
        spec.timeout = Duration::from_millis(100);
        let ProcessError::TimedOut { output, .. } = run(&spec).expect_err("must time out") else {
            panic!("expected timeout");
        };
        let child_pid = String::from_utf8(output.stdout.bytes)
            .expect("PID is UTF-8")
            .parse::<u32>()
            .expect("PID is numeric");
        let status = std::process::Command::new("/bin/kill")
            .args(["-0", &child_pid.to_string()])
            .stderr(std::process::Stdio::null())
            .status()
            .expect("kill probe starts");
        assert!(!status.success(), "descendant {child_pid} survived timeout");
    }

    struct FailingReader;

    impl Read for FailingReader {
        fn read(&mut self, _buffer: &mut [u8]) -> io::Result<usize> {
            Err(io::Error::other("synthetic reader failure"))
        }
    }

    #[test]
    fn reader_failure_is_an_explicit_process_error() {
        let result = super::join_reader(reader(FailingReader, 32), "stdout");
        assert!(matches!(
            result,
            Err(ProcessError::Read {
                stream: "stdout",
                ..
            })
        ));
    }

    #[test]
    fn bounded_reader_handles_an_immediate_eof() {
        let result = super::join_reader(reader(io::empty(), 32), "stdout").expect("EOF is valid");
        assert_eq!(result, CapturedStream::default());
    }

    #[cfg(unix)]
    #[test]
    fn command_arguments_preserve_non_utf8_bytes() {
        use std::os::unix::ffi::OsStringExt;

        let mut spec = ProcessSpec {
            program: OsString::from("/usr/bin/printf"),
            args: vec![OsString::from_vec(vec![b'x', 0x80, b'y'])],
            timeout: Duration::from_secs(2),
            sample_interval: None,
            stdout_limit_bytes: 16,
            stderr_limit_bytes: 16,
        };
        let output = run(&spec).expect("non-UTF-8 argument reaches the process");
        assert_eq!(output.stdout.bytes, [b'x', 0x80, b'y']);
        spec.program = OsString::new();
    }

    #[test]
    #[ignore = "calibration benchmark; run explicitly in release mode"]
    fn calibrates_sampler_overhead_against_legacy_loop() {
        const REPETITIONS: usize = 7;
        let workloads = [
            ("noop", "true"),
            ("sleep-10ms", "sleep 0.01"),
            ("sleep-100ms", "sleep 0.1"),
            ("sleep-1s", "sleep 1"),
        ];
        eprintln!("workload,mode,repetitions,median_us,min_us,max_us,median_peak_rss_bytes");
        for (name, script) in workloads {
            let _ = measured_run(script, None);
            let _ = measured_run(script, Some(Duration::from_millis(100)));
            let _ = measured_run(script, Some(Duration::from_millis(10)));
            let _ = legacy_elapsed(script);
            for (mode, interval) in [
                ("sampling-disabled", None),
                ("sampling-100ms", Some(Duration::from_millis(100))),
                ("sampling-10ms", Some(Duration::from_millis(10))),
            ] {
                let mut elapsed = Vec::with_capacity(REPETITIONS);
                let mut rss = Vec::with_capacity(REPETITIONS);
                for _ in 0..REPETITIONS {
                    let (duration, peak) = measured_run(script, interval);
                    elapsed.push(duration.as_micros());
                    rss.push(u128::from(peak));
                }
                print_measurement(name, mode, &mut elapsed, &mut rss);
            }
            let mut elapsed = Vec::with_capacity(REPETITIONS);
            let mut rss = vec![0; REPETITIONS];
            for _ in 0..REPETITIONS {
                elapsed.push(legacy_elapsed(script).as_micros());
            }
            print_measurement(name, "legacy-all-processes-1ms", &mut elapsed, &mut rss);
        }
    }

    fn measured_run(script: &str, sample_interval: Option<Duration>) -> (Duration, u64) {
        let mut spec = shell(script);
        spec.timeout = Duration::from_secs(5);
        spec.sample_interval = sample_interval;
        let output = run(&spec).expect("calibration command succeeds");
        assert!(output.status.success());
        assert_eq!(output.stdout.total_bytes, 0);
        assert_eq!(output.stderr.total_bytes, 0);
        (output.elapsed, output.peak_rss_bytes)
    }

    fn legacy_elapsed(script: &str) -> Duration {
        let mut child = std::process::Command::new("/bin/sh")
            .args(["-c", script])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("legacy calibration command starts");
        let started = std::time::Instant::now();
        let mut system = sysinfo::System::new();
        loop {
            system.refresh_processes(sysinfo::ProcessesToUpdate::All, true);
            if let Some(status) = child.try_wait().expect("legacy wait succeeds") {
                assert!(status.success());
                return started.elapsed();
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    fn print_measurement(workload: &str, mode: &str, elapsed: &mut [u128], rss: &mut [u128]) {
        elapsed.sort_unstable();
        rss.sort_unstable();
        eprintln!(
            "{workload},{mode},{},{},{},{},{}",
            elapsed.len(),
            elapsed[elapsed.len() / 2],
            elapsed[0],
            elapsed[elapsed.len() - 1],
            rss[rss.len() / 2]
        );
    }
}
