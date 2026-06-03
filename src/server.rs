use crate::config::LaunchConfig;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

#[cfg(windows)]
use std::os::windows::process::CommandExt;

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[derive(Debug, thiserror::Error)]
pub enum ServerError {
    #[error("Failed to start server: {0}")]
    StartFailed(#[from] std::io::Error),
    #[error("Executable not found: {0}")]
    ExeNotFound(String),
}

#[derive(Debug, Clone, PartialEq)]
pub enum ServerStatus {
    Stopped,
    Running,
    Healthy,
    Error(String),
}

pub struct LogBuffer {
    lines: Vec<String>,
    max_lines: usize,
}

impl LogBuffer {
    pub fn new() -> Self {
        Self {
            lines: Vec::new(),
            max_lines: 500,
        }
    }

    pub fn push(&mut self, line: String) {
        self.lines.push(line);
        if self.lines.len() > self.max_lines {
            let excess = self.lines.len() - self.max_lines;
            self.lines.drain(0..excess);
        }
    }

    pub fn lines(&self) -> &[String] {
        &self.lines
    }

    pub fn clear(&mut self) {
        self.lines.clear();
    }
}

pub struct ServerManager {
    handle: Arc<Mutex<Option<Child>>>,
    log: Arc<Mutex<LogBuffer>>,
    config: Arc<Mutex<Option<LaunchConfig>>>,
    exe_path: String,
    last_exit_code: Arc<Mutex<Option<i32>>>,
}

impl ServerManager {
    pub fn new(exe_path: &str) -> Self {
        Self {
            handle: Arc::new(Mutex::new(None)),
            log: Arc::new(Mutex::new(LogBuffer::new())),
            config: Arc::new(Mutex::new(None)),
            exe_path: exe_path.to_string(),
            last_exit_code: Arc::new(Mutex::new(None)),
        }
    }

    pub fn start(&self, config: &LaunchConfig) -> Result<(), ServerError> {
        self.stop_internal();

        let args = config.to_args();

        if !std::path::Path::new(&self.exe_path).exists() {
            return Err(ServerError::ExeNotFound(self.exe_path.clone()));
        }

        {
            let mut log = self.log.lock().unwrap();
            log.clear();
            log.push(format!("Starting: {} {}", self.exe_path, args.join(" ")));
        }

        let mut command = Command::new(&self.exe_path);
        command.args(&args).stdout(Stdio::piped()).stderr(Stdio::piped());
        configure_server_command(&mut command);

        let mut child = command.spawn()?;

        let stdout = child.stdout.take();
        let stderr = child.stderr.take();

        {
            *self.last_exit_code.lock().unwrap() = None;
        }

        {
            let mut guard = self.handle.lock().unwrap();
            *guard = Some(child);
        }
        {
            let mut cfg = self.config.lock().unwrap();
            *cfg = Some(config.clone());
        }

        if let Some(stdout) = stdout {
            let log = self.log.clone();
            thread::spawn(move || {
                let reader = BufReader::new(stdout);
                for line in reader.lines() {
                    if let Ok(line) = line {
                        let mut log = log.lock().unwrap();
                        log.push(line);
                    }
                }
            });
        }

        if let Some(stderr) = stderr {
            let log = self.log.clone();
            thread::spawn(move || {
                let reader = BufReader::new(stderr);
                for line in reader.lines() {
                    if let Ok(line) = line {
                        let mut log = log.lock().unwrap();
                        log.push(line);
                    }
                }
            });
        }

        Ok(())
    }

    fn stop_internal(&self) {
        let mut guard = self.handle.lock().unwrap();
        if let Some(mut child) = guard.take() {
            let _ = child.kill();
            let exit = child.wait().ok().and_then(|s| s.code());
            *self.last_exit_code.lock().unwrap() = exit;
        }
    }

    pub fn stop(&self) {
        self.stop_internal();
        {
            let mut cfg = self.config.lock().unwrap();
            *cfg = None;
        }
    }

    pub fn status(&self) -> ServerStatus {
        let mut guard = self.handle.lock().unwrap();
        if let Some(child) = &mut *guard {
            match child.try_wait() {
                Ok(Some(status)) => {
                    let code = status.code();
                    *self.last_exit_code.lock().unwrap() = code;
                    if status.success() {
                        ServerStatus::Stopped
                    } else {
                        ServerStatus::Error(format!("Exit code: {:?}", code))
                    }
                }
                Ok(None) => ServerStatus::Running,
                Err(_) => ServerStatus::Error("Process error".into()),
            }
        } else {
            ServerStatus::Stopped
        }
    }

    pub fn health_check(&self) -> ServerStatus {
        let base = self.status();
        if base != ServerStatus::Running {
            return base;
        }
        let cfg = self.config.lock().unwrap();
        if let Some(ref cfg) = *cfg {
            let addr = format!("{}:{}", cfg.host, cfg.port);
            let socket_addr = match addr.parse() {
                Ok(a) => a,
                Err(_) => return ServerStatus::Running,
            };

            match TcpStream::connect_timeout(&socket_addr, Duration::from_millis(500)) {
                Ok(mut stream) => {
                    let request = format!(
                        "GET /health HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n\r\n",
                        addr
                    );
                    if stream.write_all(request.as_bytes()).is_err() {
                        return ServerStatus::Running;
                    }
                    if stream
                        .set_read_timeout(Some(Duration::from_millis(500)))
                        .is_err()
                    {
                        return ServerStatus::Running;
                    }
                    let mut response = [0u8; 128];
                    match stream.read(&mut response) {
                        Ok(n) => {
                            let head = String::from_utf8_lossy(&response[..n]);
                            if head.contains("200") {
                                ServerStatus::Healthy
                            } else {
                                ServerStatus::Running
                            }
                        }
                        Err(_) => ServerStatus::Running,
                    }
                }
                Err(_) => ServerStatus::Running,
            }
        } else {
            ServerStatus::Running
        }
    }

    pub fn last_exit_code(&self) -> Option<i32> {
        *self.last_exit_code.lock().unwrap()
    }

    pub fn log_lines(&self) -> Vec<String> {
        let log = self.log.lock().unwrap();
        log.lines().to_vec()
    }

    pub fn active_config(&self) -> Option<LaunchConfig> {
        let cfg = self.config.lock().unwrap();
        cfg.clone()
    }
}

#[cfg(windows)]
fn configure_server_command(command: &mut Command) {
    command.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(windows))]
fn configure_server_command(_command: &mut Command) {}
