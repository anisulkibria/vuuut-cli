use std::{
    env, fs,
    io::{Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    path::{Path, PathBuf},
    process::{Command, Output},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

struct Fixture {
    root: PathBuf,
    site: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = env::temp_dir().join(format!("vuuut-cli-test-{}-{stamp}", std::process::id()));
        fs::create_dir(&root).unwrap();
        let site = root.join("site with spaces");
        Self { root, site }
    }

    fn cli(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_vuuut"))
            .arg("--dir")
            .arg(&self.site)
            .args(args)
            .env("CARGO_NET_OFFLINE", "true")
            .output()
            .unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if self.site.join(".vuuut-cli/install.json").is_file() {
            let _ = self.cli(&["stop"]);
        }
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn succeeds(output: Output) -> String {
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

fn git(repo: &Path, arguments: &[&str]) -> String {
    succeeds(
        Command::new("git")
            .current_dir(repo)
            .args([
                "-c",
                "user.name=Fixture",
                "-c",
                "user.email=fixture@example.test",
            ])
            .args(arguments)
            .output()
            .unwrap(),
    )
    .trim()
    .to_owned()
}

fn publish(repo: &Path, label: &str) -> String {
    let source = include_str!("fixtures/server.rs").replace("fixture-v1", label);
    fs::write(repo.join("src/main.rs"), source).unwrap();
    git(repo, &["add", "."]);
    git(repo, &["commit", "-qm", label]);
    git(repo, &["rev-parse", "HEAD"])
}

fn get(port: u16) -> String {
    let mut connection = TcpStream::connect_timeout(
        &SocketAddr::from(([127, 0, 0, 1], port)),
        Duration::from_secs(2),
    )
    .unwrap();
    connection
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    connection
        .write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\n\r\n")
        .unwrap();
    let mut body = String::new();
    connection.read_to_string(&mut body).unwrap();
    body
}

#[test]
fn site_survives_updates_and_can_be_uninstalled() {
    let f = Fixture::new();
    let repo = f.root.join("upstream");
    fs::create_dir_all(repo.join("src")).unwrap();
    fs::create_dir_all(repo.join("content/themes/source")).unwrap();
    fs::write(
        repo.join("content/themes/source/index.hbs"),
        "shipped theme",
    )
    .unwrap();
    fs::write(
        repo.join("Cargo.toml"),
        "[package]\nname='vuuut'\nversion='0.0.1'\nedition='2021'\n",
    )
    .unwrap();
    fs::write(
        repo.join("Cargo.lock"),
        "version = 4\n\n[[package]]\nname = \"vuuut\"\nversion = \"0.0.1\"\n",
    )
    .unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    let first = publish(&repo, "fixture-v1");

    let socket = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = socket.local_addr().unwrap().port();
    drop(socket);
    succeeds(f.cli(&[
        "install",
        "--repo",
        repo.to_str().unwrap(),
        "--port",
        &port.to_string(),
    ]));
    assert!(get(port).ends_with("fixture-v1"));
    let status = succeeds(f.cli(&["status"]));
    assert!(status.contains(&first) && status.contains("running"));
    assert!(succeeds(f.cli(&["doctor"])).contains("fixture diagnostics passed"));
    assert!(succeeds(f.cli(&["log"])).contains("fixture-v1 is serving"));
    fs::write(f.site.join("content/test.txt"), "user content").unwrap();

    let second = publish(&repo, "fixture-v2");
    succeeds(f.cli(&["update"]));
    assert!(succeeds(f.cli(&["status"])).contains(&second));
    assert!(get(port).ends_with("fixture-v2"));
    assert_eq!(
        fs::read_to_string(f.site.join("content/test.txt")).unwrap(),
        "user content"
    );

    publish(&repo, "fixture-broken");
    let failed = f.cli(&["update"]);
    assert!(!failed.status.success());
    let error = String::from_utf8_lossy(&failed.stderr);
    assert!(
        error.contains("Restored the prior revision and database"),
        "{error}"
    );
    assert!(get(port).ends_with("fixture-v2"));
    let data = succeeds(
        Command::new("sqlite3")
            .arg(f.site.join("content/data/site.db"))
            .arg("SELECT body FROM example")
            .output()
            .unwrap(),
    );
    assert_eq!(data.trim(), "preserved");

    succeeds(f.cli(&["stop"]));
    assert!(succeeds(f.cli(&["status"])).contains("stopped"));
    succeeds(f.cli(&["start"]));
    succeeds(f.cli(&["restart"]));
    assert!(get(port).ends_with("fixture-v2"));

    succeeds(f.cli(&["uninstall"]));
    assert!(!f.site.join(".vuuut-cli").exists());
    assert!(f.site.join("config.production.json").is_file());
    assert!(f.site.join("content/test.txt").is_file());

    succeeds(f.cli(&[
        "install",
        "--repo",
        repo.to_str().unwrap(),
        "--ref",
        &second,
        "--no-start",
    ]));
    let purge = f.cli(&["uninstall", "--purge"]);
    assert!(!purge.status.success());
    succeeds(f.cli(&["uninstall", "--purge", "--yes"]));
    assert!(!f.site.join("content").exists());
    assert!(!f.site.join("config.production.json").exists());
}
