# Vuuut CLI

A Rust command-line manager for Vuuut CMS installations on macOS and Linux. The CLI downloads a Git revision, builds its server with Cargo, and runs it as a background process.

## Install the CLI

Run this from the `vuuut-cli` directory. Git, Rust, and Cargo must be on your PATH. SQLite updates that restart the site also use the `sqlite3` command to back up the database.

```sh
cargo install --locked --path .
vuuut --version
```

Cargo normally installs the command into `~/.cargo/bin`. The CLI command is named `vuuut`; it launches a separate server executable stored inside each managed installation.

## Create a site

```sh
vuuut install --dir ./my-site
cd my-site
vuuut status
```

Open the admin URL printed by the installer to create the site owner. The default installation uses SQLite and binds to loopback, starting with port 1971 and choosing a free port when it is occupied. Use `--port` to select a fixed port and `--url` to set the public URL. `--host` changes the bind address. `--ssl` and `--email` pass automatic HTTPS options to Vuuut’s setup command. Use `--no-start` to configure the site and start it later.

The default source is the Vuuut Git repository at `https://github.com/anisulkibria/rust-ghost.git`. Use `--repo` for a different remote or a local repository and `--ref` to install a branch, tag, or commit. A local Git source must contain the desired changes in a commit.

```sh
vuuut install --dir ./my-site --repo ../vuuut --ref main --port 1980
```

## Manage the site

Run commands from the site directory or pass `--dir /path/to/site`.

| Command | Effect |
| --- | --- |
| `vuuut start` | Start the site and wait for its port to listen. |
| `vuuut stop` | Shut down the managed server gracefully. |
| `vuuut restart` | Stop and start the site. |
| `vuuut status` | Show the site URL, installed revision, and process status. |
| `vuuut doctor` | Run the installed server’s diagnostic checks. |
| `vuuut log --follow` | Follow the server log. |
| `vuuut update` | Fetch and build a newer revision. Restart the site if it was running. |
| `vuuut uninstall` | Stop the site and remove its managed source, binaries, build cache, and logs. Keep the site content and configuration. |

An update of a running SQLite site takes a database backup before starting the new revision. If the new server fails to start, the CLI restores the SQLite backup and starts the previous release. Backups are stored under `.vuuut-cli/backups/`. Updates of stopped sites and updates with `--no-start` apply migrations on the next start. For a separately configured MySQL database, take a database backup before upgrading.

`content/` and `config.production.json` (or `config.development.json`) live in the site directory. Downloaded source, release executables, logs, and CLI state live under `.vuuut-cli/`. The CLI also reads plain `KEY=value` entries from the site’s optional `.env`. Use the same directory to reinstall after an ordinary uninstall and reuse its configuration and content.

To remove local content and configuration as well, run `vuuut uninstall --purge --yes`. A database or content directory configured outside the managed site directory must be removed separately.
