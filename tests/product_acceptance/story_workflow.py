"""Real CLI/MCP acceptance for a 72-image story, focal zoom and three audio roles.

Creates isolated media under target; never reads or modifies production assets.
Run after building the CLI: python -X utf8 tests/product_acceptance/story_workflow.py
"""

import argparse
import hashlib
import json
import math
from pathlib import Path
from queue import Queue
import struct
import subprocess
import sys
import tempfile
import time
from threading import Thread
import wave
import zlib


ROOT = Path(__file__).resolve().parents[2]
TIMEBASE = 705_600_000


def run(args, *, timeout=900, binary=False, input_data=None):
    result = subprocess.run(
        [str(arg) for arg in args], input=input_data, capture_output=True,
        text=not binary, encoding=None if binary else "utf-8", timeout=timeout,
        check=False,
    )
    if result.returncode:
        raise RuntimeError(f"exit={result.returncode}: {args[0]}\n{result.stderr!r}")
    return result


def png(path, index):
    width, height = 320, 180
    background = bytes((24 + index % 35, 65 + index % 45, 100 + index % 60))
    rows = []
    for y in range(height):
        row = bytearray(background * width)
        if 52 <= y < 74:
            row[207 * 3:227 * 3] = b"\xf0\x20\x20" * 20
        if 140 <= y < 148:
            row[20 * 3:(20 + index + 1) * 3] = b"\x30\xc0\xd0" * (index + 1)
        rows.append(b"\x00" + row)

    def chunk(tag, payload):
        return struct.pack(">I", len(payload)) + tag + payload + struct.pack(">I", zlib.crc32(tag + payload))

    path.write_bytes(
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0))
        + chunk(b"IDAT", zlib.compress(b"".join(rows))) + chunk(b"IEND", b"")
    )


def tone(path, frequency, seconds):
    period = b"".join(struct.pack("<h", round(10_000 * math.sin(2 * math.pi * frequency * i / 48000))) for i in range(48000))
    with wave.open(str(path), "wb") as sound:
        sound.setnchannels(1)
        sound.setsampwidth(2)
        sound.setframerate(48000)
        for _ in range(seconds):
            sound.writeframesraw(period)


def sha(path):
    with path.open("rb") as handle:
        return hashlib.file_digest(handle, "sha256").hexdigest()


def marker(frame):
    points = []
    for y in range(180):
        for x in range(320):
            offset = (y * 320 + x) * 3
            r, g, b = frame[offset:offset + 3]
            if r > 175 and g < 80 and b < 80:
                points.append((x, y))
    if not points:
        raise AssertionError("Rendered subject marker is missing")
    return {
        "x": sum(x for x, _ in points) / len(points),
        "y": sum(y for _, y in points) / len(points),
        "width": max(x for x, _ in points) - min(x for x, _ in points) + 1,
    }


def amplitude(samples, frequency, center):
    start = round((center - 0.1) * 48000)
    values = samples[start:start + 9600]
    real = sum(value * math.cos(2 * math.pi * frequency * i / 48000) for i, value in enumerate(values))
    imag = sum(value * math.sin(2 * math.pi * frequency * i / 48000) for i, value in enumerate(values))
    return 2 * math.hypot(real, imag) / len(values)


def mcp_exchange(cli, workspace, messages):
    """Wait for initialize before initialized/tools; bound every protocol wait."""
    process = subprocess.Popen(
        [str(cli), "mcp", "--stdio", "--workspace", str(workspace)],
        stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
        text=True, encoding="utf-8",
    )
    lines = Queue()

    def read_lines():
        try:
            for line in process.stdout:
                lines.put(line)
        finally:
            lines.put(None)

    reader = Thread(target=read_lines, daemon=True)
    reader.start()
    replies = {}
    try:
        for message in messages:
            process.stdin.write(json.dumps(message) + "\n")
            process.stdin.flush()
            if "id" in message:
                line = lines.get(timeout=120)
                assert line is not None, "MCP exited before its response"
                reply = json.loads(line)
                assert reply.get("id") == message["id"], reply
                assert "error" not in reply, reply
                replies[reply["id"]] = reply
        process.stdin.close()
        process.wait(timeout=30)
        reader.join(timeout=5)
        assert not reader.is_alive(), "MCP stdout did not close"
        assert lines.get(timeout=5) is None, "MCP emitted unexpected stdout"
        stderr = process.stderr.read()
        (workspace / "mcp-stderr.log").write_text(stderr, encoding="utf-8")
        assert process.returncode == 0, (process.returncode, stderr)
        return replies
    finally:
        if process.poll() is None:
            process.kill()
            process.wait(timeout=10)
        reader.join(timeout=5)
        for pipe in (process.stdin, process.stdout, process.stderr):
            pipe.close()


def main():
    if sys.flags.optimize:
        raise RuntimeError("Run acceptance without -O: Python assertions must remain enabled")
    parser = argparse.ArgumentParser()
    parser.add_argument("--cli", type=Path, default=ROOT / "target/release/storycut.exe")
    parser.add_argument("--skip-full-render", action="store_true", help="Only setup and short previews; not full acceptance")
    options = parser.parse_args()
    cli = options.cli.resolve(strict=True)
    parent = ROOT / "target/story-workflow-acceptance"
    parent.mkdir(parents=True, exist_ok=True)
    work = Path(tempfile.mkdtemp(prefix="run-", dir=parent)) / "夜燈說書"
    work.mkdir()
    report = {"workspace": str(work), "cli": str(cli), "cli_sha256": sha(cli), "status": "RUNNING"}
    counter = 0

    def call(tool, args, error=None):
        nonlocal counter
        counter += 1
        request = work / f"request-{counter:03}-{tool}.json"
        request.write_text(json.dumps(args, ensure_ascii=False), encoding="utf-8")
        result = subprocess.run(
            [str(cli), "--json", "--workspace", str(work), "call", tool, "--args-file", str(request)],
            capture_output=True, text=True, encoding="utf-8", timeout=900, check=False,
        )
        response = json.loads(result.stdout)
        (work / f"response-{counter:03}.json").write_text(json.dumps(response, ensure_ascii=False, indent=2), encoding="utf-8")
        if error:
            assert not response["ok"] and response["error"]["code"] == error, response
        else:
            assert result.returncode == 0 and response["ok"], (tool, response, result.stderr)
        return response

    try:
        images = [work / f"場景-{i + 1:03}.png" for i in range(72)]
        for i, image in enumerate(images):
            png(image, i)
        audio = [work / "配樂.wav", work / "旁白.wav", work / "小虎對話.wav"]
        for path, freq, duration in zip(audio, (220, 440, 880), (7, 2, 2)):
            tone(path, freq, duration)
        video = work / "影片原聲.mp4"
        run(["ffmpeg", "-v", "error", "-f", "lavfi", "-i", "testsrc2=size=320x180:rate=30:duration=2",
             "-f", "lavfi", "-i", "sine=frequency=660:sample_rate=48000:duration=2", "-c:v", "libx264",
             "-threads", "1", "-pix_fmt", "yuv420p", "-c:a", "aac", "-t", "2", video])
        sources = images + [video] + audio
        original_hashes = {str(path): sha(path) for path in sources}
        project_path = work / "夜燈故事.storycut.json"
        project = call("storycut_project_create", {
            "path": str(project_path), "name": "夜燈說書 · 72 張焦點故事",
            "canvas": {"width": 320, "height": 180, "fps": {"num": 30, "den": 1}, "background": "#000000", "color_mode": "sdr_bt709"},
            "audio_sample_rate": 48000, "idempotency_key": "night-create",
        })["data"]["project"]
        project_id = project["project_id"]
        imported = call("storycut_media_import", {"project_id": project_id, "expected_revision": project["revision"], "idempotency_key": "night-import", "dry_run": False, "paths": [str(p) for p in sources]})
        by_name = {Path(asset["path"]).name: asset["id"] for asset in imported["data"]["assets"]}
        assembly = {
            "project_id": project_id, "expected_revision": imported["revision"], "idempotency_key": "night-assemble", "dry_run": True,
            "items": [{"asset_id": by_name[path.name]} for path in images + [video]],
            "transition": {"kind": "cut"}, "original_video_audio": "separate_linked",
            "audio_tracks": [
                {"role": "music", "track_name": "配樂", "clips": [{"asset_id": by_name[audio[0].name], "start_seconds": 0, "gain_db": -22, "fade_in_seconds": 1, "fade_out_seconds": 1, "loop_to_visual_end": True}]},
                {"role": "narration", "track_name": "旁白", "clips": [{"asset_id": by_name[audio[1].name], "start_seconds": 1, "gain_db": -3}]},
                {"role": "dialogue", "track_name": "小虎", "clips": [{"asset_id": by_name[audio[2].name], "start_seconds": 4, "gain_db": -6}]},
            ],
        }
        planned = call("storycut_storyboard_assemble", assembly)
        assert planned["data"]["visual_duration_ticks"] == 722 * TIMEBASE, planned
        assert call("storycut_project_get", {"project_id": project_id})["revision"] == imported["revision"]
        assembly["dry_run"] = False
        committed = call("storycut_storyboard_assemble", assembly)
        snapshot = call("storycut_project_get", {"project_id": project_id})["data"]["project"]
        pictures = [c for c in snapshot["clips"] if c["kind"] == "image"]
        assert len(pictures) == 72 and all(c["duration_ticks"] == 10 * TIMEBASE for c in pictures)
        assert [c["asset_id"] for c in sorted(pictures, key=lambda c: c["start_tick"])] == [by_name[p.name] for p in images]
        assert {t["name"] for t in snapshot["tracks"]} >= {"配樂", "旁白", "小虎"}
        original_video = next(c for c in snapshot["clips"] if c["kind"] == "video")
        original_audio = next(c for c in snapshot["clips"] if c["kind"] == "audio" and c["asset_id"] == original_video["asset_id"])
        assert any(set(link["clip_ids"]) == {original_video["id"], original_audio["id"]} for link in snapshot["links"])
        assert original_audio["start_tick"] == 720 * TIMEBASE and original_audio["duration_ticks"] == 2 * TIMEBASE
        focal = {"project_id": project_id, "expected_revision": snapshot["revision"], "idempotency_key": "night-focal", "dry_run": False,
                 "targets": [{"clip_id": c["id"], "focus": {"x": 217 / 320, "y": 63 / 180}} for c in pictures], "from_scale": 1, "to_scale": 1.6}
        messages = [
            {"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2025-11-25", "capabilities": {}, "clientInfo": {"name": "story-acceptance", "version": "1"}}},
            {"jsonrpc": "2.0", "method": "notifications/initialized"},
            {"jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {}},
            {"jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": {"name": "storycut_focal_motion_apply", "arguments": focal}},
        ]
        replies = mcp_exchange(cli, work, messages)
        assert replies[1]["result"]["protocolVersion"] == "2025-11-25", replies[1]
        assert {"storycut_storyboard_assemble", "storycut_focal_motion_apply"} <= {t["name"] for t in replies[2]["result"]["tools"]}
        focal_response = replies[3]["result"]["structuredContent"]
        assert focal_response["ok"], focal_response
        (work / "mcp-responses.json").write_text(json.dumps(replies, ensure_ascii=False, indent=2), encoding="utf-8")
        replay = call("storycut_storyboard_assemble", assembly)
        assert replay["revision"] == committed["revision"], replay
        current = call("storycut_project_get", {"project_id": project_id})["data"]["project"]
        assert current["revision"] == focal_response["revision"] and len(current["clips"]) == len(snapshot["clips"])
        for picture in (c for c in current["clips"] if c["kind"] == "image"):
            motion = picture["motion"]
            assert motion["domain_duration_ticks"] == 10 * TIMEBASE and motion["sample_offset_tick"] == 0
            assert motion["fit"] == "cover" and motion["avoid_exposed_edges"]
            assert math.isclose(motion["keyframes"][0]["scale"], 1) and math.isclose(motion["keyframes"][-1]["scale"], 1.6)
        music_track = next(t["id"] for t in current["tracks"] if t["name"] == "配樂")
        music_clips = sorted((c for c in current["clips"] if c["track_id"] == music_track), key=lambda c: c["start_tick"])
        assert music_clips[0]["start_tick"] == 0
        assert music_clips[-1]["start_tick"] + music_clips[-1]["duration_ticks"] == 722 * TIMEBASE
        assert all(a["start_tick"] + a["duration_ticks"] == b["start_tick"] for a, b in zip(music_clips, music_clips[1:]))
        invalid = {**assembly, "items": assembly["items"][:-1]}
        call("storycut_storyboard_assemble", invalid, error="IDEMPOTENCY_CONFLICT")
        report.update({"project_id": project_id, "project_path": str(project_path), "revision": current["revision"],
                       "images": 72, "visual_duration_seconds": 722, "audio_lanes": [t["name"] for t in current["tracks"] if t["kind"] == "audio"],
                       "total_clips": len(current["clips"]), "cli_assembly": "PASS", "mcp_focal_motion": "PASS", "idempotency_after_edit": "PASS"})
        started = time.monotonic()
        preview = call("storycut_preview_range", {"project_id": project_id, "revision": current["revision"], "idempotency_key": "night-late-preview", "width": 320, "include_subtitles": False, "start_tick": 711 * TIMEBASE, "duration_ticks": 2 * TIMEBASE})
        report["late_preview_seconds"] = round(time.monotonic() - started, 3)
        report["late_preview_job"] = preview["data"]["job"]
        if not options.skip_full_render:
            output = work / "夜燈故事-完整驗收.mp4"
            started = time.monotonic()
            rendered = call("storycut_render_start", {"project_id": project_id, "revision": current["revision"], "idempotency_key": "night-render", "path": str(output), "range_start_tick": 0, "range_end_tick": None, "subtitle_mode": "none", "encoder": "h264_cpu", "overwrite": False})
            assert rendered["data"]["job"]["state"] == "succeeded", rendered
            report["full_render_wall_seconds"] = round(time.monotonic() - started, 3)
            probe = json.loads(run(["ffprobe", "-v", "error", "-count_frames", "-show_streams", "-show_format", "-of", "json", output]).stdout)
            video_stream = next(s for s in probe["streams"] if s["codec_type"] == "video")
            assert int(video_stream["nb_read_frames"]) == 722 * 30, video_stream
            assert abs(float(probe["format"]["duration"]) - 722) < 0.04, probe
            raw = run(["ffmpeg", "-v", "error", "-i", output, "-vf", "select=eq(n\\,0)+eq(n\\,150)+eq(n\\,299)", "-fps_mode", "vfr", "-frames:v", "3", "-f", "rawvideo", "-pix_fmt", "rgb24", "pipe:1"], binary=True).stdout
            assert len(raw) == 3 * 320 * 180 * 3, len(raw)
            markers = [marker(raw[i * 172800:(i + 1) * 172800]) for i in range(3)]
            assert markers[0]["x"] - markers[-1]["x"] > 40, markers
            assert markers[-1]["y"] - markers[0]["y"] > 15, markers
            assert 1.45 < markers[-1]["width"] / markers[0]["width"] < 1.8, markers
            report.update({"output": str(output), "output_sha256": sha(output), "frames": int(video_stream["nb_read_frames"]), "duration_seconds": float(probe["format"]["duration"]), "focal_markers": markers})
            pcm = run(["ffmpeg", "-v", "error", "-i", output, "-t", "8", "-vn", "-ac", "1", "-ar", "48000", "-f", "s16le", "pipe:1"], binary=True).stdout
            samples = struct.unpack(f"<{len(pcm) // 2}h", pcm)
            observed = {str(center): {str(freq): round(amplitude(samples, freq, center), 1) for freq in (220, 440, 880)} for center in (0.5, 1.5, 4.5, 7.5)}
            report["audio_frequency_amplitudes"] = observed
            assert observed["0.5"]["440"] < 100 and observed["0.5"]["880"] < 100, observed
            assert observed["1.5"]["440"] > 3000 and observed["4.5"]["880"] > 2500, observed
            assert observed["7.5"]["220"] > 200 and observed["7.5"]["440"] < 100, observed
            tail_pcm = run(["ffmpeg", "-v", "error", "-ss", "720", "-i", output, "-t", "2", "-vn", "-ac", "1", "-ar", "48000", "-f", "s16le", "pipe:1"], binary=True).stdout
            tail_samples = struct.unpack(f"<{len(tail_pcm) // 2}h", tail_pcm)
            tail_levels = {str(center + 720): {str(freq): round(amplitude(tail_samples, freq, center), 1) for freq in (220, 660)} for center in (0.5, 1.5)}
            report["tail_audio_frequency_amplitudes"] = tail_levels
            assert all(levels["220"] > 150 and levels["660"] > 1500 for levels in tail_levels.values()), tail_levels
            sheet = work / "焦點推近-首中尾.png"
            run(["ffmpeg", "-v", "error", "-i", output, "-vf", "select=eq(n\\,0)+eq(n\\,150)+eq(n\\,299),tile=3x1", "-frames:v", "1", sheet])
            report.update({"output": str(output), "output_sha256": sha(output), "frames": 21660, "duration_seconds": 722, "focal_markers": markers, "audio_frequency_amplitudes": observed, "tail_audio_frequency_amplitudes": tail_levels, "contact_sheet": str(sheet)})
        assert all(sha(Path(path)) == digest for path, digest in original_hashes.items()), "Original media changed"
        report["source_readonly"] = "PASS"
        report["status"] = "PARTIAL" if options.skip_full_render else "PASS"
    except Exception as error:
        report.update({"status": "FAIL", "error": str(error)})
        raise
    finally:
        path = work / "report.json"
        path.write_text(json.dumps(report, ensure_ascii=False, indent=2), encoding="utf-8")
        print(json.dumps({"report": str(path), "status": report["status"]}, ensure_ascii=False))


if __name__ == "__main__":
    main()
