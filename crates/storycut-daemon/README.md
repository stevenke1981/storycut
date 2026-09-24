# StoryCut local daemon

The daemon is a Windows-only, per-user command owner for one canonical project
workspace. It acquires an exclusive `fs2` lease under `%LOCALAPPDATA%`, then
serves a current-user-SID-only named pipe. Each connection carries one newline-
framed JSON request and one response, up to 8 MiB per frame.

Start it directly from the workspace root:

```powershell
cargo run -p storycut-daemon -- serve --workspace "D:\StoryCut Projects\My Project"
```

The process prints one JSON `ready` event and stays alive independently of CLI,
MCP, or desktop client processes. Starting a second owner for the same
workspace prints `already_running` and exits. The request helper is useful for
smoke checks:

```powershell
cargo run -p storycut-daemon -- request --workspace "D:\StoryCut Projects\My Project" --method capabilities --params '{}'
```

The client library is `storycut-client`; it offers capabilities, project read,
and shared command dispatch calls. Queue dispatch is synchronous. Capabilities
explicitly report `background_render_jobs: false` and `job_cancel: false`.
Timeouts currently bound waiting for a free named-pipe instance only. Once a
request connects, request writing and the synchronous response read have no
deadline, so a stuck command can keep that client waiting. This daemon does not
yet claim background render ownership, recovery, or cancellation.

Run the Windows process and pipe acceptance tests with:

```powershell
cargo test -p storycut-daemon -p storycut-client --locked
```
