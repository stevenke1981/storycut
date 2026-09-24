use serde_json::{json, Value};
use std::env;
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

fn main() {
    let code = run(env::args().skip(1).collect());
    std::process::exit(code);
}

fn run(raw: Vec<String>) -> i32 {
    let requested_json = raw.iter().any(|arg| arg == "--json");
    let mut args = raw;
    let workspace = match take_flag(&mut args, "--workspace") {
        Ok(Some(v)) => PathBuf::from(v),
        Ok(None) => env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
        Err(msg) => return fail("INVALID_ARGUMENT", &msg, requested_json),
    };
    let json_mode = take_switch(&mut args, "--json");
    if args.first().map(String::as_str) == Some("mcp") {
        if !take_switch(&mut args, "--stdio") || args.len() != 1 {
            return fail(
                "INVALID_ARGUMENT",
                "Usage: storycut mcp --stdio --workspace DIR",
                false,
            );
        }
        return match storycut_mcp::run_stdio(
            &workspace,
            &storycut_command::supported_tools(),
            storycut_command::dispatch,
        ) {
            Ok(()) => 0,
            Err(e) => {
                eprintln!("MCP error: {e}");
                10
            }
        };
    }
    let (tool, payload) = match parse_call(&args, &workspace) {
        Ok(v) => v,
        Err(msg) => return fail("INVALID_ARGUMENT", &msg, json_mode),
    };
    if !json_mode {
        eprintln!("Tip: use --json for a machine-readable result.");
    }
    match storycut_command::dispatch(&workspace, &tool, payload) {
        Ok(value) => {
            println!("{}", value);
            0
        }
        Err(e) => {
            let code = e.code.as_str();
            let result = json!({
                "api_version":"0.2.0", "ok":false, "request_id":"cli",
                "project_id":null, "revision":null, "data":null,
                "error":{"code":code,"message":e.message,"retryable":false,"details":e.details},
                "warnings":[]
            });
            println!("{}", result);
            exit_for_code(code)
        }
    }
}

fn parse_call(args: &[String], _workspace: &Path) -> Result<(String, Value), String> {
    if args.is_empty() {
        return Err("Usage: storycut --json call TOOL --args-file FILE".into());
    }
    if args[0] == "call" {
        if args.len() != 4 || args[2] != "--args-file" {
            return Err("Usage: storycut --json call TOOL --args-file FILE".into());
        }
        return Ok((args[1].clone(), read_json(&args[3])?));
    }
    if args[0] == "capabilities" && args.len() == 1 {
        return Ok(("storycut_capabilities".into(), json!({})));
    }
    if args.len() >= 2 {
        let alias = format!("{}_{}", args[0], args[1]);
        let tool = format!("storycut_{alias}");
        if args.len() == 4 && args[2] == "--request" {
            return Ok((tool, read_json(&args[3])?));
        }
        if args.len() == 4 && args[0] == "project" && args[1] == "open" && args[2] == "--file" {
            return Ok((tool, json!({"path":args[3]})));
        }
        if args.len() == 4 && args[0] == "project" && args[1] == "get" && args[2] == "--project-id"
        {
            return Ok((tool, json!({"project_id":args[3]})));
        }
        if args[0] == "timeline" && args[1] == "get" {
            let mut project_id = None;
            let mut start_tick = 0_u64;
            let mut end_tick = None;
            let mut i = 2;
            while i + 1 < args.len() {
                match args[i].as_str() {
                    "--project-id" => project_id = Some(args[i + 1].clone()),
                    "--start-tick" => {
                        start_tick = args[i + 1].parse().map_err(|_| "Invalid --start-tick")?
                    }
                    "--end-tick" => {
                        end_tick = Some(
                            args[i + 1]
                                .parse::<u64>()
                                .map_err(|_| "Invalid --end-tick")?,
                        )
                    }
                    _ => return Err(format!("Unknown flag {}", args[i])),
                }
                i += 2;
            }
            if i != args.len() {
                return Err("A flag is missing its value".into());
            }
            return Ok((
                tool,
                json!({"project_id":project_id.ok_or("Missing --project-id")?,"start_tick":start_tick,"end_tick":end_tick}),
            ));
        }
        if args[0] == "job" && args[1] == "get" && args.len() == 4 && args[2] == "--job-id" {
            return Ok((tool, json!({"job_id":args[3],"include_preview":false})));
        }
    }
    Err("Unsupported alias or flags; use --json call TOOL --args-file FILE".into())
}

fn read_json(path: &str) -> Result<Value, String> {
    let content = if path == "-" {
        let mut s = String::new();
        io::stdin()
            .read_to_string(&mut s)
            .map_err(|e| e.to_string())?;
        s
    } else {
        fs::read_to_string(path).map_err(|e| e.to_string())?
    };
    let value: Value = serde_json::from_str(&content).map_err(|e| e.to_string())?;
    if !value.is_object() {
        return Err("Request must be a JSON object".into());
    }
    Ok(value)
}

fn take_switch(args: &mut Vec<String>, flag: &str) -> bool {
    if let Some(i) = args.iter().position(|v| v == flag) {
        args.remove(i);
        true
    } else {
        false
    }
}

fn take_flag(args: &mut Vec<String>, flag: &str) -> Result<Option<String>, String> {
    if let Some(i) = args.iter().position(|v| v == flag) {
        if i + 1 >= args.len() {
            return Err(format!("Missing value for {flag}"));
        }
        args.remove(i);
        Ok(Some(args.remove(i)))
    } else {
        Ok(None)
    }
}

fn fail(code: &str, message: &str, json_mode: bool) -> i32 {
    if json_mode {
        println!(
            "{}",
            json!({
                "api_version":"0.2.0", "ok":false, "request_id":"cli",
                "project_id":null, "revision":null, "data":null,
                "error":{"code":code,"message":message,"retryable":false,"details":{}},
                "warnings":[]
            })
        );
    } else {
        eprintln!("{code}: {message}");
    }
    2
}

fn exit_for_code(code: &str) -> i32 {
    match code {
        "INVALID_ARGUMENT" => 2,
        "NOT_FOUND" => 3,
        "REVISION_CONFLICT" | "LOCKED_TRACK" | "IDEMPOTENCY_CONFLICT" | "DEPENDENCY_CONFLICT" => 4,
        "PATH_DENIED" | "OVERWRITE_DENIED" => 5,
        "UNSUPPORTED_FEATURE" | "UNSUPPORTED_PROTOCOL" | "MEDIA_OFFLINE" => 6,
        "RENDER_FAILED" | "DISK_FULL" | "MEDIA_CHANGED" => 7,
        "CANCELLED" => 8,
        _ => 10,
    }
}
