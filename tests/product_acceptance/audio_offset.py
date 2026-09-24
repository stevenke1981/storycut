"""CLI-to-MP4 acceptance: a one-second tone starts two seconds into the mix."""

import json
import math
from pathlib import Path
import struct
import subprocess
import sys
import tempfile
import wave
import zlib


ROOT = Path(__file__).resolve().parents[2]
BASE = ROOT / "target" / "audio-acceptance-stdlib"
BASE.mkdir(parents=True, exist_ok=True)
WORK = Path(tempfile.mkdtemp(prefix="run-", dir=BASE))
CLI = ROOT / "target" / "release" / "storycut.exe"
TIMEBASE = 705_600_000


def call(name, args):
    request = WORK / f"request-{name}.json"
    request.write_text(json.dumps(args, ensure_ascii=False), encoding="utf-8")
    result = subprocess.run(
        [str(CLI), "--json", "--workspace", str(WORK), "call", name, "--args-file", str(request)],
        capture_output=True,
        text=True,
        encoding="utf-8",
        check=False,
    )
    response = json.loads(result.stdout)
    if result.returncode or not response.get("ok"):
        raise RuntimeError(f"{name}: exit={result.returncode}; response={response}; stderr={result.stderr}")
    return response


def tone(path):
    with wave.open(str(path), "wb") as sound:
        sound.setnchannels(1)
        sound.setsampwidth(2)
        sound.setframerate(48000)
        sound.writeframes(
            b"".join(struct.pack("<h", round(16000 * math.sin(2 * math.pi * 1000 * index / 48000))) for index in range(48000))
        )


def solid_png(path, width=640, height=360):
    def chunk(tag, payload):
        return struct.pack(">I", len(payload)) + tag + payload + struct.pack(">I", zlib.crc32(tag + payload))

    image = b"\x89PNG\r\n\x1a\n"
    image += chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0))
    image += chunk(b"IDAT", zlib.compress((b"\x00" + b"\xff\x00\x00" * width) * height))
    image += chunk(b"IEND", b"")
    path.write_bytes(image)


def rms(samples, start, end):
    window = samples[round(start * 48000):round(end * 48000)]
    return math.sqrt(sum(value * value for value in window) / len(window))


def main():
    if not CLI.is_file():
        raise RuntimeError(f"Release CLI missing: {CLI}")
    image = WORK / "red.png"
    tone_path = WORK / "one-second-tone.wav"
    solid_png(image)
    tone(tone_path)
    project_path = WORK / "audio-offset.storycut.json"
    project = call("storycut_project_create", {
        "path": str(project_path), "name": "Audio offset acceptance",
        "canvas": {"width": 640, "height": 360, "fps": {"num": 30, "den": 1}, "background": "#000000", "color_mode": "sdr_bt709"},
        "audio_sample_rate": 48000, "idempotency_key": "audio-offset-create-v1",
    })["data"]["project"]
    imported = call("storycut_media_import", {
        "project_id": project["project_id"], "expected_revision": project["revision"],
        "idempotency_key": "audio-offset-import-v1", "dry_run": False,
        "paths": [str(image), str(tone_path)],
    })
    assets = {asset["kind"]: asset["id"] for asset in imported["data"]["assets"]}
    motion = {
        "domain_duration_ticks": 5 * TIMEBASE, "sample_offset_tick": 0, "fit": "cover",
        "avoid_exposed_edges": False, "anchor": {"x": 0.5, "y": 0.5},
        "interpolation": "linear",
        "keyframes": [
            {"tick": 0, "x": 0, "y": 0, "scale": 1, "opacity": 1},
            {"tick": 5 * TIMEBASE - TIMEBASE // 30, "x": 0, "y": 0, "scale": 1, "opacity": 1},
        ],
    }
    audio = {
        "domain_duration_ticks": TIMEBASE, "sample_offset_tick": 0, "gain_db": 0,
        "pan": 0, "muted": False, "fade_in_ticks": 0, "fade_out_ticks": 0,
        "fade_curve": "linear_amplitude",
    }
    track = lambda id, kind: {"id": id, "kind": kind, "name": id, "locked": False, "enabled": True, "muted": False, "solo": False, "gain_db": 0}
    applied = call("storycut_timeline_apply", {
        "project_id": project["project_id"], "expected_revision": imported["revision"],
        "idempotency_key": "audio-offset-timeline-v1", "dry_run": False,
        "operations": [
            {"op": "track.add", "track": track("video", "video")},
            {"op": "track.add", "track": track("audio", "audio")},
            {"op": "clip.add", "clip": {"kind": "image", "id": "red", "track_id": "video", "asset_id": assets["image"], "start_tick": 0, "duration_ticks": 5 * TIMEBASE, "source_in_tick": 0, "motion": motion}},
            {"op": "clip.add", "clip": {"kind": "audio", "id": "tone", "track_id": "audio", "asset_id": assets["audio"], "start_tick": 2 * TIMEBASE, "duration_ticks": TIMEBASE, "source_in_tick": 0, "stream_index": 0, "audio": audio}},
        ],
    })
    output = WORK / "audio-offset.mp4"
    if output.exists():
        raise RuntimeError(f"Fresh acceptance output unexpectedly exists: {output}")
    rendered = call("storycut_render_start", {
        "project_id": project["project_id"], "revision": applied["revision"],
        "path": str(output), "overwrite": False, "idempotency_key": "audio-offset-render-v1",
        "subtitle_mode": "none", "encoder": "h264_cpu", "range_start_tick": 0,
        "range_end_tick": 5 * TIMEBASE,
    })
    job = rendered["data"]["job"]
    if job["state"] != "succeeded":
        raise RuntimeError(f"Render did not succeed: {job}")
    pcm = subprocess.run(
        ["ffmpeg", "-v", "error", "-i", str(output), "-vn", "-ac", "1", "-ar", "48000", "-f", "s16le", "-"],
        capture_output=True, check=True,
    ).stdout
    samples = struct.unpack("<" + "h" * (len(pcm) // 2), pcm)
    levels = {name: rms(samples, start, end) for name, start, end in [
        ("before", 1.5, 1.9), ("start", 2.1, 2.5), ("late", 2.5, 2.9), ("after", 3.1, 3.5),
    ]}
    passed = levels["before"] < 100 and levels["start"] > 1000 and levels["late"] > 1000 and levels["after"] < 100
    report = {"pass": passed, "workspace": str(WORK), "levels_rms": levels, "decoded_samples": len(samples), "output": str(output), "job_id": job["job_id"]}
    (WORK / "report.json").write_text(json.dumps(report, ensure_ascii=False, indent=2), encoding="utf-8")
    print(json.dumps(report, ensure_ascii=False, indent=2))
    if not passed:
        sys.exit(1)


if __name__ == "__main__":
    main()
