"""Exercise the actual update script with signed fixtures and an isolated HOME."""
import json
import os
from pathlib import Path
import plistlib
import subprocess
import tempfile
import time

SCRIPT = Path(__file__).resolve().parents[1] / "update.sh"


def bundle(path, version, register_status):
    executable = path / "Contents/MacOS/retype"
    executable.parent.mkdir(parents=True)
    with (path / "Contents/Info.plist").open("wb") as file:
        plistlib.dump({"CFBundleIdentifier": "io.github.retype.inputmethod.Retype",
                      "CFBundleExecutable": "retype", "CFBundlePackageType": "APPL",
                      "CFBundleShortVersionString": version}, file)
    subprocess.run(["xcrun", "clang", "-x", "c", "-", "-o", str(executable)],
                   input=f"int main(void) {{ return {register_status}; }}".encode(), check=True)
    subprocess.run(["codesign", "--force", "--sign", "-", str(path)],
                   check=True, capture_output=True)


def exercise(case):
    with tempfile.TemporaryDirectory(prefix="retype update helper ") as directory:
        home = Path(directory)
        root = home / "Library/Application Support/retype"
        destination = home / "Library/Input Methods/retype.app"
        source = root / "updates/fixture/retype.app"
        bundle(destination, "1.0.0", 0)
        bundle(source, "1.0.1", 7 if case == "register_failure" else 0)
        old_binary = (destination / "Contents/MacOS/retype").read_bytes()
        if case == "invalid_signature":
            with (source / "Contents/Info.plist").open("ab") as file:
                file.write(b"\n<!-- modified after signing -->\n")
        tools = home / "test-bin"
        tools.mkdir()
        # Do not register or launch the fixture with this machine's LaunchServices.
        launch = tools / "open"
        launch.write_text('#!/bin/sh\nprintf "opened\\n" >> "$HOME/opened.log"\n')
        launch.chmod(0o755)
        refresh = tools / "pkill"
        refresh.write_text("#!/bin/sh\nexit 0\n")
        refresh.chmod(0o755)
        env = dict(os.environ, HOME=str(home), PATH=str(tools) + ":" + os.environ["PATH"])
        old = subprocess.Popen(["sleep", "60"])
        helper = subprocess.Popen(["bash", str(SCRIPT), str(source), str(destination), str(old.pid)], env=env)
        state_path = root / "updates/install-state.json"
        try:
            deadline = time.monotonic() + 5
            while helper.poll() is None and time.monotonic() < deadline:
                state = json.loads(state_path.read_text()) if state_path.exists() else {}
                if state.get("step") == "waiting_for_exit":
                    assert state["stage"] == "running" and state["pid"] == helper.pid
                    old.terminate()
                    old.wait(timeout=2)
                    break
                time.sleep(.02)
            result = helper.wait(timeout=5)
            state = json.loads(state_path.read_text())
            assert state["stage"] == ("complete" if case == "success" else "failed"), state
            assert result == (0 if case == "success" else 7 if case == "register_failure" else 1), (case, result)
            with (destination / "Contents/Info.plist").open("rb") as file:
                assert plistlib.load(file)["CFBundleShortVersionString"] == ("1.0.1" if case == "success" else "1.0.0")
            if case != "success":
                assert (destination / "Contents/MacOS/retype").read_bytes() == old_binary
            if case == "invalid_signature":
                assert old.poll() is None, "Validation must fail before asking the current app to quit"
            else:
                assert (home / "opened.log").exists()
            assert (root / "logs/update-install.log").stat().st_mode & 0o777 == 0o600
            assert not list(destination.parent.glob(".retype-update.*"))
        finally:
            for process in (helper, old):
                if process.poll() is None:
                    process.terminate()
                    process.wait(timeout=3)


for scenario in ("success", "invalid_signature", "register_failure"):
    exercise(scenario)
print("Signed update helper: readiness handshake, successful install, signature rejection, rollback and persistent diagnostics passed.")
