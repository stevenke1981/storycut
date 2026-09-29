"""Real CLI acceptance for the Night Lantern profile (docs/NIGHT_LANTERN_PROFILE.md).

Builds an isolated 夜燈說書-shaped story from generated media: a 10 s native intro,
six narration segments, stills, a 10 s native clip inside a short segment, an alpha
ProRes card, an outro still, music ducked by narration, a bilingual ASS document
placed from the returned segment offsets, and a render with master loudness.
Never reads or modifies production assets.

Run after building the CLI:
    python -X utf8 tests/product_acceptance/night_lantern.py --cli target/debug/storycut.exe
"""

import argparse
import hashlib
import json
import math
from pathlib import Path
import struct
import subprocess
import tempfile
import wave

ROOT = Path(__file__).resolve().parents[2]
TIMEBASE = 705_600_000
FPS = 30
FRAME = TIMEBASE // FPS
W, H = 320, 180


def run(args, *, binary=False, timeout=900):
    result = subprocess.run([str(a) for a in args], capture_output=True, text=not binary,
                            encoding=None if binary else "utf-8", timeout=timeout, check=False)
    if result.returncode:
        raise RuntimeError(f"exit={result.returncode}: {args[0]}\n{result.stderr!r}")
    return result


def sha(path):
    with Path(path).open("rb") as handle:
        return hashlib.file_digest(handle, "sha256").hexdigest()


def tone(path, frequency, seconds, amplitude):
    count = round(seconds * 48000)
    with wave.open(str(path), "wb") as sound:
        sound.setnchannels(1)
        sound.setsampwidth(2)
        sound.setframerate(48000)
        sound.writeframes(b"".join(
            struct.pack("<h", round(amplitude * math.sin(2 * math.pi * frequency * i / 48000)))
            for i in range(count)))


def goertzel(samples, frequency, start_s, end_s):
    values = samples[round(start_s * 48000):round(end_s * 48000)]
    k = 2 * math.cos(2 * math.pi * frequency / 48000)
    s1 = s2 = 0.0
    for x in values:
        s1, s2 = x + k * s1 - s2, s1
    return 2 * math.sqrt(max(s1 * s1 + s2 * s2 - k * s1 * s2, 0)) / len(values)


def frame_rgb(path, frame_index):
    raw = run(["ffmpeg", "-v", "error", "-i", path, "-vf", f"select=eq(n\\,{frame_index})", "-fps_mode", "vfr",
               "-frames:v", "1", "-f", "rawvideo", "-pix_fmt", "rgb24", "pipe:1"], binary=True).stdout
    assert len(raw) == W * H * 3, (frame_index, len(raw))
    return raw


def mean_rgb(raw, box=(0, 0, W, H)):
    x0, y0, x1, y1 = box
    total = [0, 0, 0]
    count = 0
    for y in range(y0, y1):
        for x in range(x0, x1):
            o = (y * W + x) * 3
            total[0] += raw[o]
            total[1] += raw[o + 1]
            total[2] += raw[o + 2]
            count += 1
    return tuple(round(t / count, 1) for t in total)


def integrated_lufs(path):
    text = run(["ffmpeg", "-hide_banner", "-nostats", "-i", path, "-map", "0:a:0", "-af", "ebur128",
                "-f", "null", "-"]).stderr
    summary = text[text.rfind("Summary:"):]
    line = next(l for l in summary.splitlines() if l.strip().startswith("I:"))
    return float(line.split()[1])


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--cli", type=Path, default=ROOT / "target/debug/storycut.exe")
    options = parser.parse_args()
    cli = options.cli.resolve(strict=True)
    parent = ROOT / "target/night-lantern-acceptance"
    parent.mkdir(parents=True, exist_ok=True)
    work = Path(tempfile.mkdtemp(prefix="run-", dir=parent)) / "夜燈說書"
    work.mkdir()
    report = {"workspace": str(work), "cli": str(cli), "cli_sha256": sha(cli), "status": "RUNNING"}
    counter = 0

    def call(tool, args):
        nonlocal counter
        counter += 1
        request = work / f"request-{counter:03}-{tool}.json"
        request.write_text(json.dumps(args, ensure_ascii=False), encoding="utf-8")
        result = subprocess.run([str(cli), "--json", "--workspace", str(work), "call", tool, "--args-file", str(request)],
                                capture_output=True, text=True, encoding="utf-8", timeout=1800, check=False)
        response = json.loads(result.stdout)
        (work / f"response-{counter:03}.json").write_text(json.dumps(response, ensure_ascii=False, indent=2), encoding="utf-8")
        assert result.returncode == 0 and response["ok"], (tool, response, result.stderr)
        return response

    try:
        colors = {"場景-1.png": "0xc03030", "場景-2.png": "0x3050c0", "場景-3.png": "0xc0a030",
                  "場景-4.png": "0x8030a0", "片尾.png": "0x202020"}
        for name, color in colors.items():
            run(["ffmpeg", "-v", "error", "-f", "lavfi", "-i", f"color=c={color}:s={W}x{H}:d=1", "-frames:v", "1", work / name])
        # Native clips: 10.2 s at 24 fps, solid colours so blending is measurable.
        for name, color in (("片頭拉近.mp4", "0x20a0a0"), ("斷繩原生.mp4", "0x20c020")):
            run(["ffmpeg", "-v", "error", "-f", "lavfi", "-i", f"color=c={color}:s={W}x{H}:r=24:d=10.2",
                 "-c:v", "libx264", "-pix_fmt", "yuv420p", work / name])
        # Alpha card: white box on transparent background, ProRes 4444.
        run(["ffmpeg", "-v", "error", "-f", "lavfi", "-i", f"color=c=black@0.0:s={W}x{H}:r=30:d=3,format=rgba",
             "-vf", "drawbox=x=20:y=20:w=80:h=40:color=white@1:t=fill", "-c:v", "prores_ks", "-profile:v", "4444",
             "-pix_fmt", "yuva444p10le", work / "章回卡.mov"])
        seg_lengths = [3.1, 6.4, 7.9, 4.6, 8.2, 5.4]
        for i, seconds in enumerate(seg_lengths):
            tone(work / f"旁白-{i + 1:02}.wav", 1000, seconds, 12000)
        tone(work / "配樂.wav", 220, 60, 6000)
        sources = sorted(p for p in work.iterdir() if p.suffix in {".png", ".mp4", ".mov", ".wav"})
        originals = {str(p): sha(p) for p in sources}

        project = call("storycut_project_create", {
            "path": str(work / "偷桃測試.storycut.json"), "name": "夜燈說書 · 驗收",
            "canvas": {"width": W, "height": H, "fps": {"num": FPS, "den": 1}, "background": "#000000", "color_mode": "sdr_bt709"},
            "audio_sample_rate": 48000, "idempotency_key": "nl-create"})["data"]["project"]
        pid = project["project_id"]
        imported = call("storycut_media_import", {"project_id": pid, "expected_revision": project["revision"],
                                                  "idempotency_key": "nl-import", "dry_run": False, "paths": [str(p) for p in sources]})
        asset = {Path(a["path"]).name: a["id"] for a in imported["data"]["assets"]}
        assemble = {
            "project_id": pid, "expected_revision": imported["revision"], "idempotency_key": "nl-assemble", "dry_run": True,
            "intro": {"asset_id": asset["片頭拉近.mp4"], "duration_seconds": 10},
            "narration": {"segments": [{"asset_id": asset[f"旁白-{i + 1:02}.wav"]} for i in range(6)]},
            "shots": [
                {"asset_id": asset["場景-1.png"], "segments": [0, 1]},
                {"asset_id": asset["場景-2.png"], "segments": [2, 2]},
                {"asset_id": asset["斷繩原生.mp4"], "segments": [3, 3], "duration_seconds": 10},
                {"asset_id": asset["場景-3.png"], "segments": [4, 4]},
                {"asset_id": asset["場景-4.png"], "segments": [5, 5]},
            ],
            "outro": {"asset_id": asset["片尾.png"], "duration_seconds": 5},
            "transition": {"mode": "centered_dissolve", "duration_seconds": 0.4},
            "music": {"cues": [{"asset_id": asset["配樂.wav"], "start_seconds": 0, "fade_in_seconds": 1, "fade_out_seconds": 2}]},
            "overlays": {"items": [{"asset_id": asset["章回卡.mov"], "segment": 1, "offset_seconds": 0.5}]},
        }
        planned = call("storycut_narration_assemble", assemble)
        assert planned["data"]["committed"] is False
        assemble["dry_run"] = False
        committed = call("storycut_narration_assemble", assemble)["data"]
        total = committed["total_duration_ticks"]
        offsets = committed["segment_offsets"]
        cores = committed["native_cores"]
        cuts = committed["cuts"]
        report["assembly"] = {"total_frames": total // FRAME, "lead_in_frames": committed["lead_in_ticks"] // FRAME,
                              "segment_start_frames": [o["start_tick"] / FRAME for o in offsets],
                              "native_cores_frames": [[c["core_start_tick"] // FRAME, c["core_end_tick"] // FRAME] for c in cores],
                              "cut_frames": [[c["cut_in_tick"] // FRAME, c["cut_out_tick"] // FRAME] for c in cuts]}
        assert committed["lead_in_ticks"] == 306 * FRAME

        # Bilingual ASS: one cue on segment 2 placed through the returned offset.
        seg2 = offsets[2]
        ass = ("[Script Info]\nScriptType: v4.00+\nPlayResX: 320\nPlayResY: 180\n\n[V4+ Styles]\n"
               "Format: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, OutlineColour, BackColour, Bold, Italic, Underline, StrikeOut, ScaleX, ScaleY, Spacing, Angle, BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, Encoding\n"
               "Style: ZH,Microsoft JhengHei,28,&H00FFFFFF,&H00FFFFFF,&H00000000,&H00000000,1,0,0,0,100,100,0,0,3,6,0,2,10,10,40,1\n"
               "Style: EN,Segoe UI,16,&H00E6E6E6,&H00E6E6E6,&H00000000,&H00000000,0,0,0,0,100,100,0,0,3,4,0,2,10,10,12,1\n\n"
               "[Events]\nFormat: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\n"
               "Dialogue: 0,0:00:00.00,0:00:01.50,ZH,,0,0,0,,八八兒，還不出來謝賞？\n"
               "Dialogue: 0,0:00:00.00,0:00:01.50,EN,,0,0,0,,Baba'er, come out and give thanks!\n")
        (work / "雙語.ass").write_text(ass, encoding="utf-8")
        project_now = call("storycut_project_get", {"project_id": pid})["data"]["project"]
        track = call("storycut_track_add", {"project_id": pid, "expected_revision": project_now["revision"], "idempotency_key": "nl-subs-track",
                                            "dry_run": False, "track": {"id": "subs", "name": "字幕", "kind": "subtitle", "locked": False,
                                                                        "enabled": True, "muted": False, "solo": False, "gain_db": 0}})
        subs = call("storycut_subtitle_import", {"project_id": pid, "expected_revision": track["revision"], "idempotency_key": "nl-subs",
                                                 "dry_run": False, "path": str(work / "雙語.ass"), "track_id": "subs",
                                                 "offset_tick": seg2["start_tick"]})
        output = work / "偷桃測試-成片.mp4"
        rendered = call("storycut_render_start", {"project_id": pid, "revision": subs["revision"], "idempotency_key": "nl-render",
                                                  "path": str(output), "range_start_tick": 0, "range_end_tick": None,
                                                  "subtitle_mode": "burn", "encoder": "h264_cpu", "overwrite": False,
                                                  "master_loudness": {"integrated_lufs": -16, "true_peak_db": -1.5}})
        assert rendered["data"]["job"]["state"] == "succeeded", rendered
        probe = json.loads(run(["ffprobe", "-v", "error", "-count_frames", "-show_streams", "-of", "json", output]).stdout)
        video = next(s for s in probe["streams"] if s["codec_type"] == "video")
        frames = int(video["nb_read_frames"])
        assert frames == total // FRAME, (frames, total // FRAME)
        report["frames"] = frames

        # Native cores: every sampled core frame is the pure source colour.
        core_checks = []
        for core, colour in zip(cores, ((0x20, 0xa0, 0xa0), (0x20, 0xc0, 0x20))):
            first, last = core["core_start_tick"] // FRAME, core["core_end_tick"] // FRAME - 1
            for index in (first, (first + last) // 2, last):
                rgb = mean_rgb(frame_rgb(output, index), (130, 10, 200, 50))
                core_checks.append({"frame": index, "rgb": rgb})
                assert all(abs(a - b) < 12 for a, b in zip(rgb, colour)), (index, rgb, colour)
        report["native_core_frames"] = core_checks
        # Centered dissolves: the frame at each cut is a mix of both neighbours.
        mixes = []
        for cut in cuts[1:]:
            index = cut["cut_in_tick"] // FRAME
            before = mean_rgb(frame_rgb(output, index - 7), (130, 10, 200, 50))
            at = mean_rgb(frame_rgb(output, index), (130, 10, 200, 50))
            after = mean_rgb(frame_rgb(output, index + 7), (130, 10, 200, 50))
            mixes.append({"cut_frame": index, "before": before, "at": at, "after": after})
            for channel in range(3):
                lo, hi = sorted((before[channel], after[channel]))
                assert lo - 6 <= at[channel] <= hi + 6, (index, before, at, after)
            assert any(abs(before[c] - after[c]) > 30 and abs(at[c] - before[c]) > 10 and abs(at[c] - after[c]) > 10 for c in range(3)), mixes[-1]
        report["dissolve_midpoints"] = mixes

        # Alpha card over segment 1 (white box at 20,20 80x40) and ASS text over segment 2.
        card_frame = round((offsets[1]["start_tick"] + TIMEBASE) / FRAME)
        card = mean_rgb(frame_rgb(output, card_frame), (30, 25, 90, 55))
        assert min(card) > 200, card
        sub_frame = round((seg2["start_tick"] + TIMEBASE * 0.75) / FRAME)
        no_sub_frame = round((seg2["start_tick"] + TIMEBASE * 1.7) / FRAME)
        lit = mean_rgb(frame_rgb(output, sub_frame), (0, 100, W, 170))
        dark = mean_rgb(frame_rgb(output, no_sub_frame), (0, 100, W, 170))
        assert sum(lit) - sum(dark) > 60, (lit, dark)
        report["overlay_card_rgb"] = card
        report["ass_band_rgb"] = {"with_cue": lit, "after_cue": dark}

        # Ducking: music (220 Hz) is lower under narration than during the intro.
        pcm = run(["ffmpeg", "-v", "error", "-i", output, "-vn", "-ac", "1", "-ar", "48000", "-f", "s16le", "pipe:1"], binary=True).stdout
        samples = struct.unpack(f"<{len(pcm) // 2}h", pcm)
        intro_music = goertzel(samples, 220, 3, 8)
        s0 = offsets[1]["start_tick"] / TIMEBASE
        under_voice = goertzel(samples, 220, s0 + 0.8, s0 + 2.4)
        voice = goertzel(samples, 1000, s0 + 0.8, s0 + 2.4)
        report["music_220hz"] = {"intro": round(intro_music, 1), "under_narration": round(under_voice, 1), "narration_1khz": round(voice, 1)}
        assert under_voice < intro_music * 0.5, report["music_220hz"]
        assert voice > under_voice * 3, report["music_220hz"]
        lufs = integrated_lufs(output)
        report["integrated_lufs"] = lufs
        assert abs(lufs + 16) <= 1.0, lufs
        report["render_master_loudness"] = rendered["data"]["job"].get("master_loudness")

        assert all(sha(Path(p)) == d for p, d in originals.items()), "source media changed"
        report["source_readonly"] = "PASS"
        report.update({"output": str(output), "output_sha256": sha(output), "status": "PASS"})
    except Exception as error:
        report.update({"status": "FAIL", "error": str(error)})
        raise
    finally:
        path = work / "report.json"
        path.write_text(json.dumps(report, ensure_ascii=False, indent=2), encoding="utf-8")
        print(json.dumps({"report": str(path), "status": report["status"]}, ensure_ascii=False))


if __name__ == "__main__":
    main()
