"""Wait for a real export's terminal state; a submitted job is not a video."""
import time


def await_export(call, response, timeout=1800):
    deadline = time.monotonic() + timeout
    while response["data"]["job"]["state"] in {"queued", "running", "cancelling"}:
        if time.monotonic() > deadline:
            raise TimeoutError(response["data"]["job"])
        time.sleep(0.2)
        response = call("storycut_job_get", {"job_id": response["data"]["job"]["job_id"], "include_preview": False})
    return response
