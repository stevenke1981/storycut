"""CLI auto-start must release stdout/stderr EOF while its daemon survives."""
import argparse
import json
from pathlib import Path
import subprocess
import tempfile
import time

ROOT=Path(__file__).resolve().parents[2]


def main():
    parser=argparse.ArgumentParser()
    parser.add_argument("--cli",type=Path,default=ROOT/"target/debug/storycut.exe")
    args=parser.parse_args()
    cli=args.cli.resolve(strict=True)
    base=ROOT/"target/auto-start-acceptance"
    base.mkdir(parents=True,exist_ok=True)
    work=Path(tempfile.mkdtemp(prefix="run-",dir=base))/"自動啟動 with spaces"
    work.mkdir()
    start=time.monotonic()
    result=subprocess.run([str(cli),"--json","--workspace",str(work)+"\\","capabilities"],capture_output=True,
                          text=True,encoding="utf-8",timeout=15)
    elapsed=time.monotonic()-start
    caps=json.loads(result.stdout)
    assert result.returncode==0 and caps["ok"] and caps["data"]["background_render_jobs"],(result,caps)
    assert result.stderr=="",result.stderr
    # This new helper must exit as a loser, proving the auto-started owner lives
    # independently of the already-exited CLI whose pipes reached EOF.
    duplicate=subprocess.run([str(cli.with_name("storycut-daemon.exe")),"serve","--workspace",str(work)],
                             capture_output=True,text=True,encoding="utf-8",timeout=10)
    assert duplicate.returncode==0 and json.loads(duplicate.stdout)["event"]=="already_running",duplicate
    report={"status":"PASS","workspace":str(work),"stdout_eof_seconds":round(elapsed,4),"stderr_empty":True,
            "owner_survives_cli":True,"unicode_spaces_and_trailing_separator":True}
    (work/"report.json").write_text(json.dumps(report,ensure_ascii=False,indent=2),"utf-8")
    print(json.dumps(report,ensure_ascii=False))


if __name__=="__main__": main()
