use std::{env, fs, io::{Read, Write}, net::TcpListener, process::Command};

const REVISION: &str = "fixture-v1";

fn main() {
    let args: Vec<String> = env::args().collect();
    let option = |key: &str| args.windows(2).find(|pair| pair[0] == key).map(|pair| pair[1].as_str());
    match args.get(1).map(String::as_str) {
        Some("setup") => {
            let env = option("--env").unwrap();
            let url = option("--url").unwrap();
            let port = option("--port").unwrap();
            fs::write(format!("config.{env}.json"), format!(
                r#"{{"url":"{url}","server":{{"host":"127.0.0.1","port":{port}}},"database":{{"client":"sqlite3","connection":{{"filename":"content/data/site.db"}}}}}}"#,
            )).unwrap();
            fs::write("content/port", port).unwrap();
        }
        Some("doctor") => println!("fixture diagnostics passed"),
        Some("start") => {
            let sql = if REVISION == "fixture-broken" {
                "UPDATE example SET body = 'corrupted by migration';"
            } else {
                "CREATE TABLE IF NOT EXISTS example(body TEXT); INSERT INTO example SELECT 'preserved' WHERE NOT EXISTS (SELECT 1 FROM example);"
            };
            assert!(Command::new("sqlite3").args(["content/data/site.db", sql]).status().unwrap().success());
            if REVISION == "fixture-broken" {
                std::process::exit(1);
            }
            let port = fs::read_to_string("content/port").unwrap();
            let listener = TcpListener::bind(format!("127.0.0.1:{port}")).unwrap();
            println!("{REVISION} is serving");
            for stream in listener.incoming() {
                let mut stream = stream.unwrap();
                let mut request = [0; 1024];
                if stream.read(&mut request).unwrap_or_default() != 0 {
                    let _ = write!(stream, "HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n{REVISION}");
                }
            }
        }
        _ => panic!("unexpected command"),
    }
}
