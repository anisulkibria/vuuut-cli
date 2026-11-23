use crate::{
    release,
    runtime::{self, Config, Process},
    Install, Result,
};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Write},
    os::{
        fd::AsRawFd,
        unix::fs::{DirBuilderExt, OpenOptionsExt},
    },
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{SystemTime, UNIX_EPOCH},
};
use url::Url;

const STATE: &str = ".vuuut-cli";
const MANIFEST: &str = "install.json";

#[derive(Deserialize, Serialize)]
struct InstallState {
    environment: String,
    reference: Option<String>,
    revision: String,
}

pub struct Site {
    root: PathBuf,
    state: PathBuf,
}

pub fn write_json(path: &Path, contents: &impl Serialize) -> Result<()> {
    let temporary = path.with_extension(format!("{}.tmp", std::process::id()));
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(&temporary)?;
    let written = (|| -> Result<()> {
        serde_json::to_writer_pretty(&mut file, contents)?;
        file.write_all(b"\n")?;
        file.sync_all()?;
        fs::rename(&temporary, path)?;
        Ok(())
    })();
    if written.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    written
}

impl Site {
    fn at(root: PathBuf) -> Result<Self> {
        let state = root.join(STATE);
        let metadata = fs::symlink_metadata(&state)?;
        if !metadata.is_dir() {
            return Err(format!("{} is not a managed state directory.", state.display()).into());
        }
        Ok(Self { root, state })
    }

    pub fn open(dir: &Path) -> Result<Self> {
        let root = fs::canonicalize(dir)?;
        if !root.join(STATE).join(MANIFEST).is_file() {
            return Err(format!("No managed Vuuut installation in {}.", root.display()).into());
        }
        Self::at(root)
    }

    pub fn discover(dir: &Path) -> Result<Self> {
        for parent in fs::canonicalize(dir)?.ancestors() {
            if parent.join(STATE).join(MANIFEST).is_file() {
                return Self::open(parent);
            }
        }
        Err("No managed Vuuut installation here. Run vuuut install or specify --dir.".into())
    }

    fn lock(&self) -> Result<File> {
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open(self.state.join("lock"))?;
        // Keep the descriptor open while operating on this installation.
        if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::WouldBlock {
                return Err("Another Vuuut command is working on this installation.".into());
            }
            return Err(error.into());
        }
        Ok(lock)
    }

    fn metadata(&self) -> Result<InstallState> {
        let data = fs::read(self.state.join(MANIFEST))?;
        let installed = serde_json::from_slice::<InstallState>(&data)?;
        if !matches!(installed.environment.as_str(), "production" | "development")
            || !release::valid_commit(&installed.revision)
        {
            return Err("Invalid Vuuut installation metadata.".into());
        }
        Ok(installed)
    }

    fn save(&self, installed: &InstallState) -> Result<()> {
        write_json(&self.state.join(MANIFEST), installed)
    }

    fn launch(&self, installed: &InstallState, config: &Config) -> Result<()> {
        let binary = release::binary(&self.state, &installed.revision);
        let pid = runtime::start(&self.root, &self.state, &binary, config)?;
        println!("Vuuut is running at {} (PID {pid}).", config.url);
        Ok(())
    }

    pub fn start(&self) -> Result<()> {
        let _lock = self.lock()?;
        let installed = self.metadata()?;
        let config = Config::load(&self.root, &installed.environment)?;
        self.launch(&installed, &config)
    }

    pub fn stop(&self) -> Result<()> {
        let _lock = self.lock()?;
        if runtime::stop(&self.state)? {
            println!("Vuuut stopped.");
        } else {
            println!("Vuuut is stopped.");
        }
        Ok(())
    }

    pub fn restart(&self) -> Result<()> {
        let _lock = self.lock()?;
        let installed = self.metadata()?;
        let config = Config::load(&self.root, &installed.environment)?;
        runtime::stop(&self.state)?;
        self.launch(&installed, &config)
    }

    pub fn status(&self) -> Result<()> {
        let _lock = self.lock()?;
        let installed = self.metadata()?;
        let config = Config::load(&self.root, &installed.environment)?;
        println!("Site         {}", self.root.display());
        println!("URL          {}", config.url);
        println!("Revision     {}", installed.revision);
        match Process::read(&self.state)? {
            Some(process) if process.running(&self.state)? => {
                println!("Status       running (PID {})", process.pid)
            }
            _ => println!("Status       stopped"),
        }
        Ok(())
    }

    pub fn doctor(&self) -> Result<()> {
        let _lock = self.lock()?;
        let installed = self.metadata()?;
        let config = Config::load(&self.root, &installed.environment)?;
        println!(
            "Checking {} at revision {}",
            self.root.display(),
            installed.revision
        );
        let binary = release::binary(&self.state, &installed.revision);
        release::checked(
            config.command(&binary, &self.root).arg("doctor"),
            "Vuuut diagnostics",
        )
    }

    pub fn log(&self, lines: u32, follow: bool) -> Result<()> {
        let path = self.state.join("server.log");
        if !path.is_file() {
            println!("No log yet. Start Vuuut to create it.");
            return Ok(());
        }
        let mut command = Command::new("tail");
        command.arg("-n").arg(lines.to_string());
        if follow {
            command.arg("-f");
        }
        release::checked(command.arg(path), "read server log")
    }

    pub fn update(&self, reference: Option<String>, no_start: bool) -> Result<()> {
        let _lock = self.lock()?;
        let installed = self.metadata()?;
        let config = Config::load(&self.root, &installed.environment)?;
        let reference = reference.or_else(|| installed.reference.clone());
        let source = self.state.join("source");
        release::refresh(&source)?;
        let revision = release::commit(&source, reference.as_deref())?;
        let next = InstallState {
            environment: installed.environment.clone(),
            reference,
            revision,
        };
        if next.revision == installed.revision {
            self.save(&next)?;
            println!("Vuuut is already at {}.", next.revision);
            return Ok(());
        }
        // Build before taking the running site offline.
        release::build(&self.state, &next.revision)?;
        let running = match Process::read(&self.state)? {
            Some(process) => process.running(&self.state)?,
            None => false,
        };
        let old_db = config.database.as_deref().filter(|db| db.is_file());
        if old_db.is_some() && !no_start && running {
            release::checked(
                Command::new("sqlite3").arg("-version"),
                "check SQLite backup support",
            )?;
        }
        if running {
            runtime::stop(&self.state)?;
        }
        if no_start || !running {
            self.save(&next)?;
            println!(
                "Installed {}. Start Vuuut to apply database migrations.",
                next.revision
            );
            return Ok(());
        }

        let backup = if let Some(database) = old_db {
            let backups = self.state.join("backups");
            let timestamp = SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis();
            let path = backups.join(format!("before-{}-{timestamp}.db", &next.revision[..12]));
            let snapshot = fs::create_dir_all(backups)
                .map_err(Into::into)
                .and_then(|()| runtime::backup(database, &path));
            if let Err(error) = snapshot {
                self.launch(&installed, &config)?;
                return Err(error);
            }
            Some(path)
        } else {
            None
        };
        let prepared = self.save(&next).and_then(|()| self.launch(&next, &config));
        if let Err(error) = prepared {
            if let (Some(database), Some(backup)) = (old_db, backup.as_deref()) {
                runtime::restore(database, backup).map_err(|failure| {
                    format!("{error}. Could not restore {}: {failure}", backup.display())
                })?;
                self.save(&installed)?;
                self.launch(&installed, &config)?;
                return Err(format!(
                    "Update failed. Restored the prior revision and database. {error}"
                )
                .into());
            }
            return Err(format!("Update failed. Check the server log and address the startup error before retrying. {error}").into());
        }
        println!("Updated to {}.", next.revision);
        Ok(())
    }

    pub fn uninstall(&self, purge: bool) -> Result<()> {
        let _lock = self.lock()?;
        let installed = self.metadata()?;
        runtime::stop(&self.state)?;
        if purge {
            remove_if_exists(&self.root.join("content"))?;
            remove_if_exists(
                &self
                    .root
                    .join(format!("config.{}.json", installed.environment)),
            )?;
            remove_if_exists(&self.root.join(".env"))?;
        }
        fs::remove_dir_all(&self.state)?;
        println!("Uninstalled Vuuut from {}.", self.root.display());
        if !purge {
            println!("Site content and configuration are still in that directory.");
        }
        Ok(())
    }
}

fn remove_if_exists(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() => fs::remove_dir_all(path)?,
        Ok(_) => fs::remove_file(path)?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => (),
        Err(error) => return Err(error.into()),
    }
    Ok(())
}

pub fn install(dir: &Path, options: Install) -> Result<()> {
    release::prerequisites()?;
    fs::create_dir_all(dir)?;
    let root = fs::canonicalize(dir)?;
    let state = root.join(STATE);
    if !state.exists() {
        // A previous ordinary uninstall may have left site content and config.
        // Other files are a signal that this is someone's unrelated directory.
        let config = format!("config.{}.json", options.env);
        for entry in fs::read_dir(&root)? {
            let name = entry?.file_name();
            if name != "content" && name != ".env" && name != config.as_str() {
                return Err(format!(
                    "Choose an empty site directory. {} already contains other files.",
                    root.display()
                )
                .into());
            }
        }
        fs::DirBuilder::new().mode(0o700).create(&state)?;
    }
    let site = Site::at(root)?;
    let _lock = site.lock()?;
    if site.state.join(MANIFEST).exists() {
        return Err("Vuuut is already installed here. Use vuuut update.".into());
    }
    let repo = release::repository(&options.repo)?;
    release::clone_into(&site.state, &repo)?;
    let source = site.state.join("source");
    let revision = release::commit(&source, options.r#ref.as_deref())?;
    let binary = release::build(&site.state, &revision)?;
    release::seed_content(&site.state, &site.root)?;

    if !site
        .root
        .join(format!("config.{}.json", options.env))
        .is_file()
    {
        setup(&site.root, &binary, &options)?;
    }
    let installed = InstallState {
        environment: options.env,
        reference: options.r#ref,
        revision,
    };
    let config = Config::load(&site.root, &installed.environment)?;
    site.save(&installed)?;
    if !options.no_start {
        site.launch(&installed, &config)?;
    }
    println!("Installed revision {}.", installed.revision);
    println!("Admin setup  {}vuuut/", with_trailing_slash(config.url));
    Ok(())
}

fn with_trailing_slash(url: Url) -> String {
    format!("{}/", url.as_str().trim_end_matches('/'))
}

fn setup(root: &Path, binary: &Path, options: &Install) -> Result<()> {
    if let Some(url) = &options.url {
        let parsed = Url::parse(url)?;
        if !matches!(parsed.scheme(), "http" | "https")
            || parsed.host().is_none()
            || !parsed.username().is_empty()
            || parsed.password().is_some()
            || parsed.query().is_some()
            || parsed.fragment().is_some()
        {
            return Err(
                "Enter an HTTP or HTTPS site URL without credentials, a query, or a fragment."
                    .into(),
            );
        }
    }
    let host = options
        .host
        .as_deref()
        .unwrap_or(if options.ssl { "0.0.0.0" } else { "127.0.0.1" });
    let port = match options
        .port
        .or_else(|| options.url.as_deref().and_then(runtime::url_port))
    {
        Some(port) => port,
        None if options.ssl => 443,
        None => runtime::free_port(host)?,
    };
    let url = options
        .url
        .clone()
        .unwrap_or_else(|| format!("http://localhost:{port}"));
    let mut command = Command::new(binary);
    command.current_dir(root).args([
        "setup",
        "--env",
        &options.env,
        "--url",
        &url,
        "--host",
        host,
        "--port",
        &port.to_string(),
        if options.ssl { "--ssl" } else { "--no-ssl" },
    ]);
    if let Some(email) = &options.email {
        command.args(["--email", email]);
    }
    // Vuuut's internal setup also prints instructions for running the server
    // binary directly. The manager prints the URL and its own next steps.
    release::checked(command.stdout(Stdio::null()), "configure Vuuut")
}
