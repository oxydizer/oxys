use std::{
    ffi::OsStr,
    process::{Child, Command, ExitStatus, Output, Stdio},
    sync::mpsc::{self, Receiver, Sender},
    thread::{self, JoinHandle},
};

use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandStep {
    pub description: String,
    pub program: String,
    pub args: Vec<String>,
}

impl CommandStep {
    pub(crate) fn new(
        description: impl Into<String>,
        program: impl Into<String>,
        args: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        Self {
            description: description.into(),
            program: program.into(),
            args: args.into_iter().map(Into::into).collect(),
        }
    }

    pub(crate) fn command_line(&self) -> String {
        std::iter::once(self.program.as_str())
            .chain(self.args.iter().map(String::as_str))
            .map(crate::util::shell_quote)
            .collect::<Vec<_>>()
            .join(" ")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StepEvent {
    StepStart { description: String },
    StepOutput { line: String },
    StepComplete { description: String },
    Error { step: String, message: String },
}

pub struct StepStream<E> {
    receiver: Receiver<StepEvent>,
    worker: Option<JoinHandle<Result<(), E>>>,
}

impl<E> StepStream<E>
where
    E: From<ExecError>,
{
    pub(crate) fn spawn<F>(run: F) -> Self
    where
        F: FnOnce(Sender<StepEvent>) -> Result<(), E> + Send + 'static,
        E: Send + 'static,
    {
        let (sender, receiver) = mpsc::channel();
        let worker = thread::spawn(move || run(sender));

        Self {
            receiver,
            worker: Some(worker),
        }
    }

    pub fn wait(mut self) -> Result<(), E> {
        for _ in &mut self {}

        match self.worker.take().expect("worker missing").join() {
            Ok(result) => result,
            Err(_) => Err(E::from(ExecError::WorkerThread)),
        }
    }
}

impl<E> Iterator for StepStream<E> {
    type Item = StepEvent;

    fn next(&mut self) -> Option<Self::Item> {
        self.receiver.recv().ok()
    }
}

#[derive(Debug, Error)]
pub enum ExecError {
    #[error("required tool is missing: {0}")]
    MissingTool(String),
    #[error("failed to spawn {program}: {source}")]
    Spawn {
        program: String,
        source: std::io::Error,
    },
    #[error("{stream} pipe was unavailable for {program}")]
    PipeUnavailable {
        program: String,
        stream: &'static str,
    },
    #[error("failed reading command output: {0}")]
    Read(#[from] std::io::Error),
    #[error("step failed: {step}: {status}")]
    StepFailed { step: String, status: ExitStatus },
    #[error("worker thread panicked")]
    WorkerThread,
    #[error("output reader thread panicked")]
    ReaderThread,
}

pub(crate) fn run_command_step(
    step: &CommandStep,
    sender: &Sender<StepEvent>,
) -> Result<(), ExecError> {
    run_command(&step.description, &step.program, &step.args, sender)
}

pub(crate) fn run_command(
    description: &str,
    program: &str,
    args: &[String],
    sender: &Sender<StepEvent>,
) -> Result<(), ExecError> {
    let _ = sender.send(StepEvent::StepStart {
        description: description.to_owned(),
    });

    let mut command = Command::new(program);
    command.args(args);
    let mut output = ProcessStream::spawn(command)?;
    while let Some(line) = output.next_line()? {
        let _ = sender.send(StepEvent::StepOutput { line });
    }

    let status = output.wait_for_exit()?;
    if !status.success() {
        return Err(ExecError::StepFailed {
            step: description.to_owned(),
            status,
        });
    }

    let _ = sender.send(StepEvent::StepComplete {
        description: description.to_owned(),
    });
    Ok(())
}

pub(crate) fn capture_command<I, S>(program: &str, args: I) -> Result<Output, ExecError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    Command::new(program)
        .args(args)
        .output()
        .map_err(|source| command_spawn_error(program, source))
}

pub(crate) struct ProcessStream {
    child: Child,
    receiver: Receiver<StreamMessage>,
    reader_threads: Vec<JoinHandle<()>>,
    exhausted: bool,
}

impl ProcessStream {
    pub(crate) fn spawn(mut command: Command) -> Result<Self, ExecError> {
        let program = command.get_program().to_string_lossy().to_string();
        command.stdout(Stdio::piped()).stderr(Stdio::piped());

        let mut child = command
            .spawn()
            .map_err(|source| command_spawn_error(&program, source))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| ExecError::PipeUnavailable {
                program: program.clone(),
                stream: "stdout",
            })?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| ExecError::PipeUnavailable {
                program,
                stream: "stderr",
            })?;

        let (sender, receiver) = mpsc::channel();
        let reader_threads = vec![
            spawn_reader(stdout, sender.clone()),
            spawn_reader(stderr, sender),
        ];

        Ok(Self {
            child,
            receiver,
            reader_threads,
            exhausted: false,
        })
    }

    pub(crate) fn next_line(&mut self) -> Result<Option<String>, ExecError> {
        if self.exhausted {
            return Ok(None);
        }

        match self.receiver.recv() {
            Ok(StreamMessage::Line(line)) => Ok(Some(line)),
            Ok(StreamMessage::ReadError(source)) => {
                self.exhausted = true;
                Err(ExecError::Read(source))
            }
            Err(_) => {
                self.exhausted = true;
                Ok(None)
            }
        }
    }

    pub(crate) fn wait_for_exit(mut self) -> Result<ExitStatus, ExecError> {
        let status = self.child.wait()?;
        self.join_reader_threads()?;
        Ok(status)
    }

    fn join_reader_threads(&mut self) -> Result<(), ExecError> {
        for handle in self.reader_threads.drain(..) {
            if handle.join().is_err() {
                return Err(ExecError::ReaderThread);
            }
        }

        Ok(())
    }
}

#[derive(Debug)]
enum StreamMessage {
    Line(String),
    ReadError(std::io::Error),
}

fn spawn_reader<R>(reader: R, sender: Sender<StreamMessage>) -> JoinHandle<()>
where
    R: std::io::Read + Send + 'static,
{
    crate::util::spawn_reader(reader, sender, |line| {
        Some(match line {
            Ok(line) => StreamMessage::Line(line),
            Err(err) => StreamMessage::ReadError(err),
        })
    })
}

fn command_spawn_error(program: &str, source: std::io::Error) -> ExecError {
    if source.kind() == std::io::ErrorKind::NotFound {
        ExecError::MissingTool(program.to_owned())
    } else {
        ExecError::Spawn {
            program: program.to_owned(),
            source,
        }
    }
}
