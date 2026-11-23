use crate::{release, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    env, fs, io,
    net::{SocketAddr, TcpListener, TcpStream, ToSocketAddrs},
    os::unix::process::CommandExt,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};
use url::Url;

pub struct Config {
    environment: String,
    variables: BTreeMap<String, String>,
    pub url: Url,
    pub host: String,
    pub port: u16,
    pub database: Option<PathBuf>,
}

impl Config {
    pub fn load(root: &Path, environment: &str) -> Result<Self> {
        let file = root.join(format!("config.{environment}.json"));
        let json: Value = serde_json::from_str(&fs::read_to_string(&file)?)?;
        let variables = dotenv(&root.join(".env"))?;
        let get = |key: &str| env::var(key).ok().or_else(|| variables.get(key).cloned());
        let url = get("GHOST_URL")
            .or_else(|| string(&json, "/url"))
            .unwrap_or_else(|| "http://localhost:1971".to_owned());
        let explicit_port = url_port(&url);
        let url = Url::parse(&url)?;
        let host = get("GHOST_SERVER__HOST")
            .or_else(|| string(&json, "/server/host"))
            .unwrap_or_else(|| "127.0.0.1".to_owned());
        let tls = get("GHOST_SSL").unwrap_or_else(|| {
            if json["ssl"].is_object() || json["ssl"] == true {
                "true".into()
            } else {
                "false".into()
            }
        });
        let tls = get("GHOST_SSL__MODE")
            .or_else(|| string(&json, "/ssl/mode"))
            .unwrap_or(tls);
        let port = get("GHOST_SERVER__PORT")
            .map(|port| port.parse::<u16>())
            .transpose()?
            .or(json
                .pointer("/server/port")
                .and_then(Value::as_u64)
                .map(u16::try_from)
                .transpose()?)
            .or(explicit_port)
            .unwrap_or(if matches!(tls.as_str(), "off" | "false" | "none") {
                1971
            } else {
                443
            });
        if port == 0 {
            return Err("The managed server needs a fixed, nonzero listening port.".into());
        }
        let client = get("GHOST_DATABASE__CLIENT")
            .or_else(|| string(&json, "/database/client"))
            .unwrap_or_else(|| "sqlite3".to_owned());
        let database = if matches!(client.as_str(), "sqlite3" | "better-sqlite3" | "sqlite") {
            let filename = get("GHOST_DATABASE__CONNECTION__FILENAME")
                .or_else(|| string(&json, "/database/connection/filename"))
                .unwrap_or_else(|| {
                    if environment == "production" {
                        "content/data/ghost.db".into()
                    } else {
                        "content/data/ghost-dev.db".into()
                    }
                });
            Some(root.join(filename))
        } else {
            None
        };
        Ok(Self {
            environment: environment.into(),
            variables,
            url,
            host,
            port,
            database,
        })
    }

    pub fn command(&self, binary: &Path, root: &Path) -> Command {
        let mut command = Command::new(binary);
        command
            .current_dir(root)
            .env("GHOST_ENV", &self.environment);
        for (key, value) in &self.variables {
            if key != "GHOST_ENV" && env::var_os(key).is_none() {
                command.env(key, value);
            }
        }
        command
    }

    pub fn addresses(&self) -> Result<Vec<SocketAddr>> {
        let address = match self.host.as_str() {
            "0.0.0.0" => "127.0.0.1",
            "::" | "[::]" => "::1",
            other => other,
        };
        let addresses = (address, self.port).to_socket_addrs()?.collect::<Vec<_>>();
        if addresses.is_empty() {
            return Err(format!("Could not resolve {address}.").into());
        }
        Ok(addresses)
    }
}

/// Preserve explicit default ports, which URL normalization discards.
pub fn url_port(url: &str) -> Option<u16> {
    url.split_once("://")?
        .1
        .split(['/', '?', '#'])
        .next()?
        .rsplit_once(':')?
        .1
        .parse()
        .ok()
}

fn string(doc: &Value, path: &str) -> Option<String> {
    doc.pointer(path)?.as_str().map(str::to_owned)
}

fn dotenv(path: &Path) -> Result<BTreeMap<String, String>> {
    let content = match fs::read_to_string(path) {
        Ok(content) => content,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(BTreeMap::new()),
        Err(error) => return Err(error.into()),
    };
    let mut values = BTreeMap::new();
    for (index, line) in content.lines().enumerate() {
        let line = line.trim_start();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let line = line.strip_prefix("export ").unwrap_or(line);
        let Some((name, value)) = line.split_once('=') else {
            return Err(format!(
                "Invalid assignment in {} at line {}.",
                path.display(),
                index + 1
            )
            .into());
        };
        if name.is_empty()
            || name.as_bytes()[0].is_ascii_digit()
            || !name
                .bytes()
                .all(|byte| byte == b'_' || byte.is_ascii_alphanumeric())
        {
            return Err(
                format!("Invalid name in {} at line {}.", path.display(), index + 1).into(),
            );
        }
        let value = value
            .strip_prefix('"')
            .and_then(|value| value.strip_suffix('"'))
            .or_else(|| {
                value
                    .strip_prefix('\'')
                    .and_then(|value| value.strip_suffix('\''))
            })
            .unwrap_or(value);
        values
            .entry(name.to_owned())
            .or_insert_with(|| value.to_owned());
    }
    Ok(values)
}

pub fn free_port(host: &str) -> Result<u16> {
    (1971..=2020)
        .find(|port| TcpListener::bind((host, *port)).is_ok())
        .ok_or_else(|| {
            "No listening port is available from 1971 through 2020. Specify --port.".into()
        })
}

#[derive(Deserialize, Serialize)]
pub struct Process {
    pub pid: u32,
    started: String,
    executable: PathBuf,
}

impl Process {
    pub fn read(state: &Path) -> Result<Option<Self>> {
        match fs::read(state.join("process.json")) {
            Ok(data) => Ok(Some(serde_json::from_slice(&data)?)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    pub fn running(&self, state: &Path) -> Result<bool> {
        if self.pid < 2
            || self.pid > i32::MAX as u32
            || !self.executable.starts_with(state.join("releases"))
        {
            return Ok(false);
        }
        let Some((started, command)) = inspect(self.pid)? else {
            return Ok(false);
        };
        Ok(started == self.started && command == format!("{} start", self.executable.display()))
    }
}

fn inspect(pid: u32) -> Result<Option<(String, String)>> {
    let output = Command::new("ps")
        .args([
            "-ww",
            "-p",
            &pid.to_string(),
            "-o",
            "stat=",
            "-o",
            "lstart=",
            "-o",
            "command=",
        ])
        .env("LC_ALL", "C")
        .output()?;
    if !output.status.success() {
        if output.stderr.is_empty() {
            return Ok(None);
        }
        return Err(format!(
            "Could not inspect server process {pid}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )
        .into());
    }
    let line = String::from_utf8(output.stdout)?;
    let mut tail = line.trim_start();
    let mut parts = Vec::new();
    for _ in 0..6 {
        let Some(end) = tail.find(char::is_whitespace) else {
            return Ok(None);
        };
        parts.push(&tail[..end]);
        tail = tail[end..].trim_start();
    }
    if parts[0].starts_with('Z') {
        return Ok(None);
    }
    Ok(Some((
        parts[1..].join(" "),
        tail.trim_end_matches(['\r', '\n']).into(),
    )))
}

pub fn start(root: &Path, state: &Path, binary: &Path, config: &Config) -> Result<u32> {
    if let Some(process) = Process::read(state)? {
        if process.running(state)? {
            return Ok(process.pid);
        }
    }
    let addresses = config.addresses()?;
    if reachable(&addresses) {
        return Err(format!("Port {} is occupied by another process.", config.port).into());
    }
    let log_path = state.join("server.log");
    let log = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)?;
    let mut cmd = config.command(binary, root);
    cmd.arg("start")
        .stdin(Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log);
    // The child has its own session, so terminal exit does not stop the server.
    unsafe {
        cmd.pre_exec(|| {
            if libc::setsid() == -1 {
                Err(io::Error::last_os_error())
            } else {
                Ok(())
            }
        });
    }
    let mut child = cmd.spawn()?;
    let pid = child.id();
    let initialized = (|| -> Result<()> {
        let deadline = Instant::now() + Duration::from_secs(45);
        loop {
            if let Some(status) = child.try_wait()? {
                return Err(
                    format!("Server exited with {status}. See {}.", log_path.display()).into(),
                );
            }
            if reachable(&addresses) {
                let (started, _) = inspect(pid)?.ok_or("The server exited during startup.")?;
                let process = Process {
                    pid,
                    started,
                    executable: binary.to_path_buf(),
                };
                crate::instance::write_json(&state.join("process.json"), &process)?;
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(format!(
                    "Server did not listen on port {}. See {}.",
                    config.port,
                    log_path.display()
                )
                .into());
            }
            thread::sleep(Duration::from_millis(150));
        }
    })();
    if initialized.is_err() {
        let _ = child.kill();
        let _ = child.wait();
    }
    initialized.map(|()| pid)
}

fn reachable(addresses: &[SocketAddr]) -> bool {
    addresses
        .iter()
        .any(|address| TcpStream::connect_timeout(address, Duration::from_millis(150)).is_ok())
}

pub fn stop(state: &Path) -> Result<bool> {
    let Some(process) = Process::read(state)? else {
        return Ok(false);
    };
    if !process.running(state)? {
        fs::remove_file(state.join("process.json"))?;
        return Ok(false);
    }
    let pid = i32::try_from(process.pid)?;
    if unsafe { libc::kill(pid, libc::SIGTERM) } != 0 {
        let error = io::Error::last_os_error();
        if error.raw_os_error() != Some(libc::ESRCH) {
            return Err(error.into());
        }
    }
    let deadline = Instant::now() + Duration::from_secs(15);
    while process.running(state)? {
        if Instant::now() >= deadline {
            return Err(format!(
                "Server {} did not finish shutting down. See {}.",
                process.pid,
                state.join("server.log").display()
            )
            .into());
        }
        thread::sleep(Duration::from_millis(150));
    }
    fs::remove_file(state.join("process.json"))?;
    Ok(true)
}

pub fn backup(database: &Path, path: &Path) -> Result<()> {
    let path = path
        .to_str()
        .ok_or("The database backup path must be UTF-8.")?
        .replace('\'', "''");
    release::checked(
        Command::new("sqlite3")
            .args(["-batch", "-bail"])
            .arg(database)
            .arg(format!("VACUUM INTO '{path}'")),
        "back up the SQLite database",
    )
}

pub fn restore(database: &Path, backup: &Path) -> Result<()> {
    let parent = database
        .parent()
        .ok_or("The database file has no containing directory.")?;
    let temporary = parent.join(format!(".vuuut-restore-{}.db", std::process::id()));
    fs::copy(backup, &temporary)?;
    if let Ok(original) = fs::metadata(database) {
        fs::set_permissions(&temporary, original.permissions())?;
    }
    for suffix in ["-wal", "-shm"] {
        let mut name = database.as_os_str().to_owned();
        name.push(suffix);
        match fs::remove_file(PathBuf::from(name)) {
            Ok(()) => (),
            Err(error) if error.kind() == io::ErrorKind::NotFound => (),
            Err(error) => return Err(error.into()),
        }
    }
    fs::rename(temporary, database)?;
    Ok(())
}
