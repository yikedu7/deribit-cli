//! Neutral temporary-workspace and subprocess support for later CLI tests.

use std::{
    ffi::{OsStr, OsString},
    fs,
    io::{self, Write},
    path::{Component, Path, PathBuf},
    process::{Command, Output, Stdio},
};

use tempfile::TempDir;

/// An isolated test directory that rejects absolute and parent-relative writes.
pub struct TestWorkspace {
    directory: TempDir,
}

impl TestWorkspace {
    pub fn new() -> io::Result<Self> {
        tempfile::tempdir().map(|directory| Self { directory })
    }

    pub fn path(&self) -> &Path {
        self.directory.path()
    }

    pub fn write(&self, relative: impl AsRef<Path>, bytes: &[u8]) -> io::Result<PathBuf> {
        let relative = relative.as_ref();
        if relative.is_absolute()
            || relative
                .components()
                .any(|component| !matches!(component, Component::Normal(_)))
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "test workspace path must be a normal relative path",
            ));
        }
        let destination = self.path().join(relative);
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&destination, bytes)?;
        Ok(destination)
    }
}

/// One deterministic subprocess case with explicit argv, stdin, cwd, and env.
#[derive(Default)]
pub struct ProcessCase {
    args: Vec<OsString>,
    stdin: Vec<u8>,
    current_dir: Option<PathBuf>,
    environment: Vec<(OsString, OsString)>,
}

impl ProcessCase {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn arg(mut self, value: impl Into<OsString>) -> Self {
        self.args.push(value.into());
        self
    }

    pub fn stdin(mut self, bytes: impl Into<Vec<u8>>) -> Self {
        self.stdin = bytes.into();
        self
    }

    pub fn current_dir(mut self, path: impl Into<PathBuf>) -> Self {
        self.current_dir = Some(path.into());
        self
    }

    pub fn env(mut self, key: impl Into<OsString>, value: impl Into<OsString>) -> Self {
        self.environment.push((key.into(), value.into()));
        self
    }

    pub fn run(self, program: impl AsRef<OsStr>) -> io::Result<Output> {
        let mut command = Command::new(program);
        command
            .args(self.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(current_dir) = self.current_dir {
            command.current_dir(current_dir);
        }
        for (key, value) in self.environment {
            command.env(key, value);
        }

        let mut child = command.spawn()?;
        if !self.stdin.is_empty() {
            child
                .stdin
                .take()
                .expect("piped stdin must be available")
                .write_all(&self.stdin)?;
        }
        child.wait_with_output()
    }
}
