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
and shared command dispatch calls. Edits are serialized; export jobs run in
bounded background workers. Capabilities report `background_render_jobs: true`
and `job_cancel: true`. See [background exports](../../docs/ASYNC_EXPORT.md).
Timeouts currently bound waiting for a free named-pipe instance only. Once a
request connects, request writing and the synchronous response read have no
deadline, so a stuck synchronous preview or edit can keep that client waiting.
Exports return before encoding, survive adapter EOF, and support cancellation.
Interrupted exports are detected on subsequent job reads; automatic resumption
and incremental job events are not implemented.

Run the Windows process and pipe acceptance tests with:

```powershell
cargo test -p storycut-daemon -p storycut-client --locked
```
