use clap::{Args, Parser, Subcommand};
use std::{env, path::PathBuf, process::ExitCode};

mod instance;
mod release;
mod runtime;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

#[derive(Parser)]
#[command(name = "vuuut", version, about = "Install and manage Vuuut CMS")]
struct Cli {
    /// Site directory. Defaults to the current directory or its managed parent.
    #[arg(long, global = true)]
    dir: Option<PathBuf>,

    #[command(subcommand)]
    command: Action,
}

#[derive(Subcommand)]
enum Action {
    /// Download, build, configure, and start a new site
    Install(Install),
    /// Build and activate a newer revision
    Update {
        /// Branch, tag, or commit to track instead of the current reference
        #[arg(long)]
        r#ref: Option<String>,
        /// Leave the site stopped after updating
        #[arg(long)]
        no_start: bool,
    },
    /// Launch the site's server
    Start,
    /// Stop the site's server gracefully
    Stop,
    /// Stop and start the site's server
    Restart,
    /// Print the installed revision and server status
    Status,
    /// Diagnose the installation and its configuration
    Doctor,
    /// Read the server log
    Log {
        #[arg(short, long, default_value_t = 40)]
        lines: u32,
        #[arg(short, long)]
        follow: bool,
    },
    /// Remove the managed server and build files
    Uninstall {
        /// Also delete the site's content and Vuuut configuration
        #[arg(long, requires = "yes")]
        purge: bool,
        /// Confirm deletion of the site's content and configuration
        #[arg(long)]
        yes: bool,
    },
}

#[derive(Args)]
struct Install {
    /// Git remote or local source repository
    #[arg(long, default_value = "https://github.com/anisulkibria/rust-ghost.git")]
    repo: String,
    /// Branch, tag, or commit to install
    #[arg(long)]
    r#ref: Option<String>,
    /// Public site URL
    #[arg(long)]
    url: Option<String>,
    /// Address for the server to bind
    #[arg(long)]
    host: Option<String>,
    /// Listening port
    #[arg(long, value_parser = clap::value_parser!(u16).range(1..))]
    port: Option<u16>,
    /// Configure Vuuut's automatic HTTPS
    #[arg(long)]
    ssl: bool,
    /// Contact email for automatic HTTPS
    #[arg(long, requires = "ssl")]
    email: Option<String>,
    /// Write production or development configuration
    #[arg(long, default_value = "production", value_parser = ["production", "development"])]
    env: String,
    /// Build and configure without starting the site
    #[arg(long)]
    no_start: bool,
}

fn run(cli: Cli) -> Result<()> {
    let dir = cli.dir.clone().unwrap_or(env::current_dir()?);
    if let Action::Install(options) = cli.command {
        return instance::install(&dir, options);
    }
    let site = match if cli.dir.is_some() {
        instance::Site::open(&dir)
    } else {
        instance::Site::discover(&dir)
    } {
        Ok(site) => site,
        Err(error) if matches!(cli.command, Action::Doctor) => {
            println!("No managed Vuuut installation here. Checking build prerequisites.");
            release::prerequisites()?;
            return Err(error);
        }
        Err(error) => return Err(error),
    };
    match cli.command {
        Action::Update { r#ref, no_start } => site.update(r#ref, no_start),
        Action::Start => site.start(),
        Action::Stop => site.stop(),
        Action::Restart => site.restart(),
        Action::Status => site.status(),
        Action::Doctor => site.doctor(),
        Action::Log { lines, follow } => site.log(lines, follow),
        Action::Uninstall { purge, .. } => site.uninstall(purge),
        Action::Install(_) => unreachable!(),
    }
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("vuuut: {error}");
            ExitCode::FAILURE
        }
    }
}
