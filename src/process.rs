use std::fmt;
use std::io::{self, Read};
use std::process::{Command, ExitStatus, Stdio};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use sysinfo::{Pid, ProcessesToUpdate, Signal, System};

#[derive(Debug)]
pub(crate) struct ProcessOutput {
    pub status: ExitStatus,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub elapsed: Duration,
    pub peak_rss_bytes: u64,
}

#[derive(Debug)]
pub(crate) enum ProcessError {
    Start(io::Error),
    Wait(io::Error),
    Read(io::Error),
    ReaderPanicked,
    TimedOut {
        timeout: Duration,
        stdout: Vec<u8>,
        stderr: Vec<u8>,
        peak_rss_bytes: u64,
    },
}

impl fmt::Display for ProcessError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Start(error) => write!(formatter, "could not start process: {error}"),
            Self::Wait(error) => write!(formatter, "could not wait for process: {error}"),
            Self::Read(error) => write!(formatter, "could not read process output: {error}"),
            Self::ReaderPanicked => formatter.write_str("process output reader panicked"),
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

pub(crate) fn run(command: &[String], timeout: Duration) -> Result<ProcessOutput, ProcessError> {
    let Some(executable) = command.first() else {
        return Err(ProcessError::Start(io::Error::new(
            io::ErrorKind::InvalidInput,
            "process command must not be empty",
        )));
    };
    let mut child = Command::new(executable)
        .args(&command[1..])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(ProcessError::Start)?;
    let root = Pid::from_u32(child.id());
    let stdout = reader(child.stdout.take().expect("piped stdout is present"));
    let stderr = reader(child.stderr.take().expect("piped stderr is present"));
    let started = Instant::now();
    let mut system = System::new();
    let mut peak_rss_bytes = 0;

    let status = loop {
        system.refresh_processes(ProcessesToUpdate::All, true);
        peak_rss_bytes = peak_rss_bytes.max(process_tree_rss(&system, root));
        if let Some(status) = child.try_wait().map_err(ProcessError::Wait)? {
            break status;
        }
        if started.elapsed() >= timeout {
            terminate_process_tree(&mut system, root);
            let _ = child.kill();
            let _ = child.wait();
            let stdout = join_reader(stdout)?;
            let stderr = join_reader(stderr)?;
            return Err(ProcessError::TimedOut {
                timeout,
                stdout,
                stderr,
                peak_rss_bytes,
            });
        }
        thread::sleep(Duration::from_millis(1));
    };
    Ok(ProcessOutput {
        status,
        stdout: join_reader(stdout)?,
        stderr: join_reader(stderr)?,
        elapsed: started.elapsed(),
        peak_rss_bytes,
    })
}

fn reader(mut stream: impl Read + Send + 'static) -> JoinHandle<io::Result<Vec<u8>>> {
    thread::spawn(move || {
        let mut output = Vec::new();
        stream.read_to_end(&mut output)?;
        Ok(output)
    })
}

fn join_reader(reader: JoinHandle<io::Result<Vec<u8>>>) -> Result<Vec<u8>, ProcessError> {
    reader
        .join()
        .map_err(|_| ProcessError::ReaderPanicked)?
        .map_err(ProcessError::Read)
}

fn process_tree_rss(system: &System, root: Pid) -> u64 {
    system
        .processes()
        .iter()
        .filter(|(pid, _)| is_descendant(system, **pid, root))
        .fold(0, |total, (_, process)| {
            total.saturating_add(process.memory())
        })
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
