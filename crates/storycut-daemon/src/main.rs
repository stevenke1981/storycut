use std::path::PathBuf;

use storycut_daemon::{DaemonError, ServerOutcome, serve_workspace};

fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(2);
    }
}

fn run() -> Result<(), DaemonError> {
    let mut args = std::env::args_os().skip(1);
    let Some(command) = args.next() else {
        return Err(usage());
    };
    match command.to_string_lossy().as_ref() {
        "serve" => {
            let workspace = parse_workspace(args)?;
            match serve_workspace(&workspace, |endpoint| {
                println!(
                    "{{\"event\":\"ready\",\"endpoint\":{}}}",
                    serde_json::to_string(endpoint).unwrap()
                );
                use std::io::Write;
                let _ = std::io::stdout().flush();
            })? {
                ServerOutcome::Owner => {
                    unreachable!("the owner loop is intentionally non-returning")
                }
                ServerOutcome::AlreadyRunning => println!("{{\"event\":\"already_running\"}}"),
            }
        }
        "request" => {
            let (workspace, method, params, request_id) = parse_request(args)?;
            let response =
                storycut_daemon::request_workspace(&workspace, &request_id, &method, params)?;
            println!(
                "{}",
                serde_json::to_string(&response)
                    .map_err(|error| DaemonError::Protocol(error.to_string()))?
            );
            if !response.ok {
                std::process::exit(1);
            }
        }
        _ => return Err(usage()),
    }
    Ok(())
}

fn parse_workspace(
    mut args: impl Iterator<Item = std::ffi::OsString>,
) -> Result<PathBuf, DaemonError> {
    let mut workspace = None;
    while let Some(arg) = args.next() {
        if arg == "--workspace" {
            workspace = args.next().map(PathBuf::from);
        } else {
            return Err(usage());
        }
    }
    workspace.ok_or_else(usage)
}

fn parse_request(
    mut args: impl Iterator<Item = std::ffi::OsString>,
) -> Result<(PathBuf, String, serde_json::Value, String), DaemonError> {
    let mut workspace = None;
    let mut method = None;
    let mut params = serde_json::Value::Object(Default::default());
    let mut request_id = uuid::Uuid::new_v4().to_string();
    while let Some(arg) = args.next() {
        let value = args.next().ok_or_else(usage)?;
        match arg.to_string_lossy().as_ref() {
            "--workspace" => workspace = Some(PathBuf::from(value)),
            "--method" => method = Some(value.to_string_lossy().into_owned()),
            "--params" => {
                params = serde_json::from_str(&value.to_string_lossy())
                    .map_err(|error| DaemonError::Protocol(error.to_string()))?
            }
            "--request-id" => request_id = value.to_string_lossy().into_owned(),
            _ => return Err(usage()),
        }
    }
    Ok((
        workspace.ok_or_else(usage)?,
        method.ok_or_else(usage)?,
        params,
        request_id,
    ))
}

fn usage() -> DaemonError {
    DaemonError::Initialization("usage: storycut-daemon serve --workspace <directory> | request --workspace <directory> --method <name> [--params <json>] [--request-id <id>]".to_owned())
}
