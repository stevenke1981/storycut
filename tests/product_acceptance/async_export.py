"""Real Windows daemon/CLI/MCP export lifecycle, using generated media only."""
import argparse
import hashlib
import json
from pathlib import Path
import queue
import subprocess
import tempfile
import threading
import time

from export_jobs import await_export

ROOT = Path(__file__).resolve().parents[2]
TIMEBASE = 705_600_000


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--cli", type=Path, default=ROOT / "target/debug/storycut.exe")
    options = parser.parse_args()
    cli = options.cli.resolve(strict=True)
    daemon = cli.with_name("storycut-daemon.exe")
    base = ROOT / "target/async-export-acceptance"
    base.mkdir(parents=True, exist_ok=True)
    work = Path(tempfile.mkdtemp(prefix="run-", dir=base)) / "非同步 export"
    work.mkdir()
    report = {"status": "RUNNING", "workspace": str(work), "cli_sha256": hashlib.sha256(cli.read_bytes()).hexdigest()}
    owner = None
    mcp = None
    counter = 0

    def start_owner():
        process = subprocess.Popen([str(daemon), "serve", "--workspace", str(work)], stdin=subprocess.DEVNULL,
                                   stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, encoding="utf-8",
                                   creationflags=subprocess.CREATE_NO_WINDOW)
        messages = queue.Queue()
        threading.Thread(target=lambda: messages.put(process.stdout.readline()), daemon=True).start()
        assert json.loads(messages.get(timeout=10))["event"] == "ready"
        return process

    def call(tool, args, error=None):
        nonlocal counter
        counter += 1
        request = work / f"request-{counter:03}.json"
        request.write_text(json.dumps(args, ensure_ascii=False), "utf-8")
        started = time.monotonic()
        result = subprocess.run([str(cli), "--json", "--workspace", str(work), "call", tool, "--args-file", str(request)],
                                capture_output=True, text=True, encoding="utf-8", timeout=15)
        response = json.loads(result.stdout)
        if error: assert response["error"]["code"] == error and result.returncode != 0, response
        else: assert response["ok"] and result.returncode == 0, (response, result.stderr)
        (work / f"response-{counter:03}.json").write_text(json.dumps(response, ensure_ascii=False, indent=2), "utf-8")
        report.setdefault("latency_seconds", {}).setdefault(tool, []).append(round(time.monotonic() - started, 4))
        return response

    def render(pid, revision, name, duration=None):
        return {"project_id": pid, "revision": revision, "idempotency_key": name, "path": str(work / f"{name}.mp4"),
                "overwrite": False, "encoder": "h264_cpu", "subtitle_mode": "none", "range_start_tick": 0,
                "range_end_tick": duration * TIMEBASE if duration else None}

    try:
        owner = start_owner()
        capabilities = call("storycut_capabilities", {})["data"]
        assert capabilities["background_render_jobs"] and capabilities["job_cancel"]
        image = work / "generated.png"
        subprocess.run(["ffmpeg", "-v", "error", "-f", "lavfi", "-i", "testsrc2=s=640x360", "-frames:v", "1", str(image)], check=True, capture_output=True)
        source_sha = hashlib.sha256(image.read_bytes()).hexdigest()
        project = call("storycut_project_create", {"path":str(work/"acceptance.storycut.json"), "name":"Async Export Acceptance",
                       "canvas":{"width":1920,"height":1080,"fps":{"num":30,"den":1},"background":"#000000","color_mode":"sdr_bt709"},
                       "audio_sample_rate":48000,"idempotency_key":"create"})["data"]["project"]
        pid = project["project_id"]
        imported = call("storycut_media_import", {"project_id":pid,"expected_revision":project["revision"],"dry_run":False,
                        "idempotency_key":"import","paths":[str(image)]})
        assembled = call("storycut_storyboard_assemble", {"project_id":pid,"expected_revision":imported["revision"],"dry_run":False,
                         "idempotency_key":"assemble","items":[{"asset_id":imported["data"]["assets"][0]["id"],"duration_seconds":120}],
                         "transition":{"kind":"cut"},"original_video_audio":"separate_linked","audio_tracks":[]})
        revision = assembled["revision"]
        report["project_id"] = pid
        report["project_path"] = str(work/"acceptance.storycut.json")
        # Start on MCP, then ping and issue a project read on the same adapter.
        mcp = subprocess.Popen([str(cli), "mcp", "--stdio", "--workspace", str(work)], stdin=subprocess.PIPE,
                               stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, encoding="utf-8", creationflags=subprocess.CREATE_NO_WINDOW)
        inbox = queue.Queue()
        def reader():
            for line in mcp.stdout: inbox.put(json.loads(line))
        threading.Thread(target=reader, daemon=True).start()
        def rpc(message):
            mcp.stdin.write(json.dumps(message)+"\n"); mcp.stdin.flush()
            return inbox.get(timeout=10)
        rpc({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"async-acceptance","version":"1"}}})
        mcp.stdin.write('{"jsonrpc":"2.0","method":"notifications/initialized"}\n'); mcp.stdin.flush()
        capabilities_rpc=rpc({"jsonrpc":"2.0","id":10,"method":"tools/call","params":{"name":"storycut_capabilities","arguments":{}}})
        assert not capabilities_rpc["result"].get("isError") and capabilities_rpc["result"]["structuredContent"]["data"]["background_render_jobs"],capabilities_rpc
        args = render(pid, revision, "cancelled")
        started = time.monotonic()
        submitted = rpc({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"storycut_render_start","arguments":args}})
        report["mcp_submit_seconds"] = round(time.monotonic()-started,4)
        assert not submitted["result"].get("isError"), submitted
        job = submitted["result"]["structuredContent"]["data"]["job"]
        assert job["state"] == "running" and report["mcp_submit_seconds"] < 3, job
        started = time.monotonic()
        assert rpc({"jsonrpc":"2.0","id":3,"method":"ping"})["result"] == {}
        report["mcp_ping_seconds"] = round(time.monotonic()-started,4)
        call("storycut_project_get", {"project_id":pid})
        replay = call("storycut_render_start", args)["data"]["job"]
        assert replay["job_id"] == job["job_id"] and replay["state"] == "running"
        call("storycut_render_start", {**args,"subtitle_mode":"burn"}, "IDEMPOTENCY_CONFLICT")
        deadline = time.monotonic()+30
        while time.monotonic()<deadline:
            job = call("storycut_job_get", {"job_id":job["job_id"],"include_preview":False})["data"]["job"]
            if job["progress"] is not None and 0 < job["progress"] < 1: break
            if job["state"] in {"failed", "cancelled", "interrupted"}: break
            time.sleep(0.2)
        assert job["state"] == "running" and 0 < job["progress"] < 1, job
        report["measured_progress"] = job["progress"]
        old = work / "existing.mp4"
        old.write_bytes(b"previous user output sentinel")
        old_sha = hashlib.sha256(old.read_bytes()).hexdigest()
        call("storycut_render_start", {**render(pid,revision,"existing"),"path":str(old)}, "OVERWRITE_DENIED")
        cancelling = call("storycut_job_cancel", {"job_id":job["job_id"],"idempotency_key":"cancel"})
        assert cancelling["data"]["job"]["state"] in {"cancelling","cancelled"}
        terminal = await_export(call,cancelling,30)["data"]["job"]
        assert terminal["state"] == "cancelled" and terminal["failure_code"] == "CANCELLED", terminal
        assert not (work/"cancelled.mp4").exists()
        assert call("storycut_job_cancel", {"job_id":job["job_id"],"idempotency_key":"cancel-again"})["data"]["job"] == terminal
        report["cancelled_job"] = terminal
        checked = rpc({"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"storycut_job_get","arguments":{"job_id":terminal["job_id"],"include_preview":False}}})
        assert not checked["result"].get("isError") and checked["result"]["structuredContent"]["data"]["job"]==terminal,checked
        checked = rpc({"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"storycut_job_cancel","arguments":{"job_id":terminal["job_id"],"idempotency_key":"mcp-repeat"}}})
        assert not checked["result"].get("isError") and checked["result"]["structuredContent"]["data"]["job"]==terminal,checked
        # Adapter EOF does not affect daemon or later exports.
        mcp.stdin.close(); assert mcp.wait(timeout=10)==0; mcp=None
        assert owner.poll() is None
        submitted_success = call("storycut_render_start",render(pid,revision,"success",2))
        track_id = call("storycut_project_get",{"project_id":pid})["data"]["project"]["tracks"][0]["id"]
        changed = call("storycut_track_update",{"project_id":pid,"expected_revision":revision,"idempotency_key":"edit-during-export","dry_run":False,"track_id":track_id,"changes":{"enabled":False}})
        completed = await_export(call,submitted_success,90)["data"]["job"]
        assert completed["state"] == "succeeded", completed
        assert completed["source_revision"]==revision and changed["revision"]>revision
        frame = subprocess.check_output(["ffmpeg","-v","error","-i",str(work/"success.mp4"),"-frames:v","1","-f","rawvideo","-pix_fmt","rgb24","-"])
        assert max(frame)-min(frame)>100, "The in-flight snapshot must keep the visual track enabled"
        restored=call("storycut_track_update",{"project_id":pid,"expected_revision":changed["revision"],"idempotency_key":"restore-track","dry_run":False,"track_id":track_id,"changes":{"enabled":True}})
        revision=restored["revision"]
        report["immutable_revision_verified"]=True
        output = work/"success.mp4"
        assert hashlib.sha256(output.read_bytes()).hexdigest()==completed["artifacts"][0]["sha256"]
        probe = json.loads(subprocess.check_output(["ffprobe","-v","error","-count_frames","-show_streams","-of","json",str(output)]))
        assert int(next(s for s in probe["streams"] if s["codec_type"]=="video")["nb_read_frames"]) == 60
        assert call("storycut_job_cancel",{"job_id":completed["job_id"],"idempotency_key":"too-late"})["data"]["job"]["state"]=="succeeded"
        report["succeeded_job"] = completed
        # Introduce a foreign target during encoding. Failure must preserve it.
        collision_args = render(pid,revision,"collision")
        collision = call("storycut_render_start",collision_args)
        collision_path = work/"collision.mp4"; collision_path.write_bytes(b"foreign output created after submit")
        # Short range to finish quickly while still testing the publication check.
        call("storycut_job_cancel",{"job_id":collision["data"]["job"]["job_id"],"idempotency_key":"collision-cancel"})
        await_export(call,collision,30)
        assert collision_path.read_bytes()==b"foreign output created after submit"
        failed_args = render(pid,revision,"failure",2)
        failed = call("storycut_render_start",failed_args)
        (work/"failure.mp4").write_bytes(b"foreign output created after submit")
        failed = await_export(call,failed,90)["data"]["job"]
        assert failed["state"]=="failed" and failed["failure_code"]=="OVERWRITE_DENIED",failed
        assert (work/"failure.mp4").read_bytes()==b"foreign output created after submit"
        report["failed_job"] = failed
        preserved_args=render(pid,revision,"preserve-stage",2)
        preserved_id="render-"+hashlib.sha256((pid+"\0preserve-stage").encode()).hexdigest()[:20]
        preserved_stage=work/".storycut/jobs"/(preserved_id+".stage.mp4")
        preserved_stage.write_bytes(b"pre-existing private stage sentinel")
        preserved=call("storycut_render_start",preserved_args)
        assert preserved["data"]["job"]["job_id"]==preserved_id
        assert await_export(call,preserved,30)["data"]["job"]["state"]=="failed"
        assert preserved_stage.read_bytes()==b"pre-existing private stage sentinel"
        report["pre_existing_private_stage_preserved"]=True
        first=call("storycut_render_start",render(pid,revision,"capacity-a"))
        second=call("storycut_render_start",render(pid,revision,"capacity-b"))
        call("storycut_render_start",render(pid,revision,"capacity-overflow"),"JOB_BUSY")
        deadline=time.monotonic()+30
        while time.monotonic()<deadline:
            pair=[call("storycut_job_get",{"job_id":s["data"]["job"]["job_id"],"include_preview":False})["data"]["job"] for s in [first,second]]
            if all(j["progress"] is not None and j["progress"]>0 for j in pair): break
            time.sleep(0.2)
        assert all(j["state"]=="running" and j["progress"]>0 for j in pair),pair
        for submitted in [first,second]:
            stopped=call("storycut_job_cancel",{"job_id":submitted["data"]["job"]["job_id"],"idempotency_key":"capacity-cancel"})
            assert await_export(call,stopped,30)["data"]["job"]["state"]=="cancelled"
            if submitted is first:
                assert call("storycut_job_get",{"job_id":second["data"]["job"]["job_id"],"include_preview":False})["data"]["job"]["state"]=="running", "Cancellation must not wait for or stop another export"
        report["bounded_capacity_verified"]=True
        # Stop only our explicitly created test owner; confirm interrupted job on restart.
        interrupted = call("storycut_render_start",render(pid,revision,"interrupted"))["data"]["job"]
        time.sleep(0.3)
        owner.terminate(); owner.wait(timeout=10); owner=start_owner()
        recovered=call("storycut_job_get",{"job_id":interrupted["job_id"],"include_preview":False})["data"]["job"]
        assert recovered["state"]=="interrupted" and not (work/"interrupted.mp4").exists(),recovered
        assert call("storycut_job_get",{"job_id":completed["job_id"],"include_preview":False})["data"]["job"]["state"]=="succeeded"
        report["recovered_job"] = recovered
        assert hashlib.sha256(image.read_bytes()).hexdigest()==source_sha
        assert hashlib.sha256(old.read_bytes()).hexdigest()==old_sha
        assert list((work/".storycut/jobs").glob("*.stage.mp4"))==[preserved_stage]
        report["source_and_old_output_unchanged"] = True
        report["status"]="PASS"
    finally:
        if mcp is not None:
            mcp.terminate(); mcp.wait(timeout=10)
        if owner is not None and owner.poll() is None:
            owner.terminate(); owner.wait(timeout=10)
        if owner is not None:
            report["daemon_stderr"] = owner.stderr.read()
        (work/"report.json").write_text(json.dumps(report,ensure_ascii=False,indent=2),"utf-8")
        print(json.dumps({"status":report["status"],"report":str(work/"report.json")},ensure_ascii=False))


if __name__=="__main__": main()
