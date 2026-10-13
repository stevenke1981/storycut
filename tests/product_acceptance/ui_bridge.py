"""Browser QA adapter for the built UI, backed by real CLI/daemon results.

Only file-picker choices and the Tauri transport are substituted. This does not
validate native WebView2, dialogs or installer behavior. Generated fixtures only.
"""
import argparse
import base64
import hashlib
from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer
import json
from pathlib import Path
import subprocess
import threading

ROOT = Path(__file__).resolve().parents[2]


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--report", type=Path, required=True)
    parser.add_argument("--port", type=int, default=14929)
    args = parser.parse_args()
    report=json.loads(args.report.read_text("utf-8"))
    assert report["status"]=="PASS"
    work=Path(report["workspace"])
    cli=ROOT/"target/debug/storycut.exe"
    dist=ROOT/"desktop/dist"
    evidence=ROOT/"target/ui-export-evidence"
    evidence.mkdir(parents=True,exist_ok=True)
    owner=subprocess.Popen([str(cli.with_name("storycut-daemon.exe")),"serve","--workspace",str(work)],
                           stdin=subprocess.DEVNULL,stdout=subprocess.PIPE,stderr=subprocess.PIPE,text=True,encoding="utf-8",
                           creationflags=subprocess.CREATE_NO_WINDOW)
    assert json.loads(owner.stdout.readline())["event"]=="ready", "UI QA requires its own fresh fixture daemon"
    def call(tool,payload):
        result=subprocess.run([str(cli),"--json","--workspace",str(work),"call",tool,"--args-file","-"],
                              input=json.dumps(payload),capture_output=True,text=True,encoding="utf-8",timeout=90)
        value=json.loads(result.stdout)
        with (evidence/"bridge-calls.jsonl").open("a",encoding="utf-8") as log: log.write(json.dumps({"tool":tool,"args":payload,"response":value},ensure_ascii=False)+"\n")
        return value

    class Handler(SimpleHTTPRequestHandler):
        def __init__(self,*values,**kwargs): super().__init__(*values,directory=str(dist),**kwargs)
        def do_GET(self):
            if self.path=="/" or self.path=="/index.html":
                content=(dist/"index.html").read_text("utf-8")
                shim='<script>window.__TAURI_INTERNALS__={invoke:async(cmd,args)=>{const r=await fetch("/invoke",{method:"POST",headers:{"Content-Type":"application/json"},body:JSON.stringify({cmd,args})});const v=await r.json();if(!r.ok)throw new Error(v.error);return v;}};</script>'
                data=content.replace('<head>','<head>'+shim).encode("utf-8")
                self.send_response(200); self.send_header("Content-Type","text/html; charset=utf-8"); self.send_header("Content-Length",str(len(data))); self.end_headers(); self.wfile.write(data)
            else: super().do_GET()
        def do_POST(self):
            try:
                if self.path=="/shutdown":
                    threading.Thread(target=self.server.shutdown,daemon=True).start()
                    self.respond({}); return
                assert self.path=="/invoke"
                length=int(self.headers.get("Content-Length",0)); assert 0<length<8*1024*1024
                request=json.loads(self.rfile.read(length)); cmd=request["cmd"]; params=request.get("args") or {}
                if cmd=="plugin:dialog|open": value=report["project_path"]
                elif cmd=="plugin:dialog|save": value=str(work/"gui-export.mp4")
                elif cmd=="storycut_set_workspace":
                    assert Path(params["path"]).resolve()==work.resolve(); value=None
                elif cmd=="storycut_dispatch": value=call(params["tool"],params["args"])
                elif cmd=="storycut_read_preview":
                    response=call("storycut_job_get",{"job_id":params["jobId"],"include_preview":False})
                    artifact=next(a for a in response["data"]["job"]["artifacts"] if a["artifact_id"]==params["artifactId"])
                    path=(work/artifact["relative_path"]).resolve(); assert path.is_relative_to(work.resolve()/".storycut/previews")
                    data=path.read_bytes(); assert hashlib.sha256(data).hexdigest()==artifact["sha256"]
                    value={"mime_type":artifact["mime_type"],"data_url":f'data:{artifact["mime_type"]};base64,'+base64.b64encode(data).decode()}
                elif cmd=="storycut_read_asset_thumbnail":
                    response=call("storycut_project_get",{"project_id":params["projectId"]})
                    asset=next(a for a in response["data"]["project"]["assets"] if a["id"]==params["assetId"])
                    path=Path(report["project_path"]).parent/asset["path"]; assert path.resolve().is_relative_to(work.resolve())
                    data=path.read_bytes(); assert hashlib.sha256(data).hexdigest()==asset["sha256"]
                    value={"asset_id":asset["id"],"mime_type":"image/png","data_url":"data:image/png;base64,"+base64.b64encode(data).decode(),"width":640,"height":360}
                else: raise ValueError("unsupported QA transport command: "+cmd)
                self.respond(value)
            except Exception as error: self.respond({"error":str(error)},500)
        def respond(self,value,status=200):
            data=json.dumps(value,ensure_ascii=False).encode("utf-8")
            self.send_response(status); self.send_header("Content-Type","application/json"); self.send_header("Content-Length",str(len(data))); self.end_headers(); self.wfile.write(data)

    try:
        server=ThreadingHTTPServer(("127.0.0.1",args.port),Handler)
        print(json.dumps({"url":f"http://127.0.0.1:{args.port}","daemon_pid":owner.pid,"evidence":str(evidence)}),flush=True)
        server.serve_forever()
    finally:
        if owner.poll() is None: owner.terminate(); owner.wait(timeout=10)


if __name__=="__main__": main()
