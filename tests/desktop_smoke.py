"""Native, isolated Rometric UI inspection; never uses the listener's config.

Requires pywinauto and Pillow as development tools, not application dependencies.
Run --help for paths and options. Only the process launched here is ever closed.
"""

import argparse
import ctypes
import json
import os
from pathlib import Path
import site
import subprocess
import sys
import uuid


def exercise(spec, window, config_path, artifacts, timings):
    from pywinauto import keyboard, mouse

    def read_config():
        try:
            return json.loads(config_path.read_text(encoding="utf-8"))
        except (OSError, ValueError):
            return {}

    def wait_config(predicate):
        timings.wait_until(12, 0.1, lambda: predicate(read_config()))

    def button(name):
        control = spec.child_window(title=name, control_type="Button")
        control.wait("visible", timeout=10)
        return control.wrapper_object()

    def screenshot(name):
        window.capture_as_image().save(artifacts / (name + ".png"))

    results = []
    keyboard.send_keys("{F1}")
    spec.child_window(title="Keyboard shortcuts", control_type="Text").wait("visible", timeout=10)
    screenshot("help")
    keyboard.send_keys("{ESC}")
    timings.wait_until(10, 0.1, lambda: not spec.child_window(title="Keyboard shortcuts", control_type="Text").exists())
    results.append("F1 opens real help; Escape closes it")

    keyboard.send_keys("^f")
    keyboard.send_keys("sony", with_spaces=True)
    timings.wait_until(10, 0.1, lambda: any(child.window_text().lower().startswith("sony ") for child in window.descendants(control_type="Text")))
    results.append("Ctrl+F searches the real 6,029-profile library")

    spec.child_window(title="Parametric EQ", control_type="CheckBox").click_input()
    wait_config(lambda cfg: cfg.get("parametric_enabled") is True)
    results.append("existing EQ changes to parametric mode")

    slider = spec.child_window(title_re=r"Band 6 .*", control_type="Slider").wrapper_object()
    rect = slider.rectangle()
    start = ((rect.left + rect.right) // 2, (rect.top + rect.bottom) // 2)
    end = (start[0], start[1] - max(20, (rect.bottom - rect.top) // 4))
    mouse.press(button="left", coords=start)
    try:
        mouse.move(coords=end)
    finally:
        mouse.release(button="left", coords=end)
    wait_config(lambda cfg: len(cfg.get("gains", [])) == 11 and cfg["gains"][5] > 2.0)
    spec.child_window(title="Unsaved changes", control_type="Text").wait("visible", timeout=10)
    screenshot("parametric-unsaved")
    results.append("real fader drag persists gain and shows Unsaved changes")

    before = read_config()["presets"][0]
    keyboard.send_keys("^s")
    button("Cancel")
    screenshot("overwrite-confirmation")
    button("Cancel").click_input()
    timings.wait_until(10, 0.1, lambda: not spec.child_window(title="Replace preset", control_type="Button").exists())
    assert read_config()["presets"][0] == before
    results.append("Cancel leaves the saved preset unchanged")

    keyboard.send_keys("^s")
    button("Replace preset").click_input()
    wait_config(lambda cfg: cfg.get("presets", [{}])[0].get("sound", {}).get("parametric_enabled") is True)
    timings.wait_until(10, 0.1, lambda: not spec.child_window(title="Unsaved changes", control_type="Text").exists())
    results.append("Replace saves the real setup and clears Unsaved changes")

    crossfeed = spec.child_window(title="Crossfeed", control_type="Slider").wrapper_object()
    rect = crossfeed.rectangle()
    crossfeed.click_input(coords=((rect.right - rect.left) // 2, (rect.bottom - rect.top) // 2))
    wait_config(lambda cfg: cfg.get("crossfeed", 0.0) > 0.3)
    results.append("existing sound-shaping panel controls real crossfeed")

    spec.child_window(title="Level match A/B", control_type="CheckBox").click_input()
    spec.child_window(title="Start audio to calibrate levels", control_type="Text").wait("visible", timeout=10)
    wait_config(lambda cfg: cfg.get("level_match_ab") is True)
    results.append("level matching waits for audio instead of displaying fake calibration")

    button("Reload devices").click_input()
    results.append("Reload devices remains reachable")
    screenshot("features-maximized")
    window.restore()
    timings.wait_until(10, 0.1, lambda: not window.is_maximized())
    screenshot("restored")
    window.maximize()
    timings.wait_until(10, 0.1, window.is_maximized)
    screenshot("maximized-again")
    results.append("restore and maximize keep the app responsive")
    (artifacts / "interactions.json").write_text(json.dumps(results, indent=2), encoding="utf-8")
    return results


def inspect_devices(spec, window, artifacts, timings):
    """Read the existing routing menus; do not select or open an audio stream."""
    from pywinauto import keyboard

    devices = {}
    for label in ("Input", "Output"):
        label_rect = spec.child_window(title=label, control_type="Text").rectangle()
        candidates = [control for control in window.descendants(control_type="ComboBox")
                      if control.rectangle().left >= label_rect.right
                      and control.rectangle().top <= label_rect.bottom
                      and control.rectangle().bottom >= label_rect.top]
        combo = min(candidates, key=lambda control: control.rectangle().left)
        before = {control.window_text() for control in window.descendants()}
        combo.click_input()
        timings.wait_until(10, 0.1, lambda: any(control.window_text() and control.window_text() not in before for control in window.descendants()))
        devices[label.lower()] = sorted({control.window_text() for control in window.descendants() if control.window_text() and control.window_text() not in before})
        keyboard.send_keys("{ESC}")
    (artifacts / "devices.json").write_text(json.dumps(devices, indent=2), encoding="utf-8")
    return devices


def probe_parameters(spec, window, config_path, artifacts, timings):
    """Discriminate numeric-editor readiness from application binding errors."""
    from pywinauto import keyboard

    def cfg():
        try:
            return json.loads(config_path.read_text(encoding="utf-8"))
        except (OSError, ValueError):
            return {}

    spec.child_window(title="Parametric EQ", control_type="CheckBox").click_input()
    timings.wait_until(12, 0.1, lambda: cfg().get("parametric_enabled") is True)
    observations = []
    for label, value, field in [("Freq", "1500", "frequency_hz"), ("Q", "3.0", "q")]:
        label_rect = spec.child_window(title=label, control_type="Text").rectangle()
        def neighbors(control_type=None):
            return [control for control in window.descendants(control_type=control_type)
                    if control.rectangle().left >= label_rect.right
                    and control.rectangle().left < label_rect.right + 170
                    and control.rectangle().top <= label_rect.bottom
                    and control.rectangle().bottom >= label_rect.top]
        choices = [control for control in neighbors() if control.element_info.control_type in ("Spinner", "Custom", "Edit")]
        control = min(choices, key=lambda item: item.rectangle().left)
        observations.append({"label": label, "target_type": control.element_info.control_type,
                             "target_name": control.window_text()})
        # egui's DragValue deliberately keeps its SpinButton role even in text
        # mode. Use the numeric accessibility action it explicitly supports.
        control.iface_range_value.SetValue(float(value))
        timings.wait_until(12, 0.1, lambda: cfg().get("band_settings", [{}] * 11)[5].get(field) == float(value))
        observations[-1]["saved_value"] = cfg()["band_settings"][5][field]
    window.capture_as_image().save(artifacts / "parameter-probe.png")
    (artifacts / "parameter-probe.json").write_text(json.dumps(observations, indent=2), encoding="utf-8")
    return observations


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--exe", type=Path, required=True)
    parser.add_argument("--artifacts", type=Path, required=True)
    parser.add_argument("--tool-path", type=Path)
    parser.add_argument("--exercise", action="store_true", help="exercise real UI controls, using only isolated QA data")
    parser.add_argument("--list-devices", action="store_true", help="inspect routing menus without starting audio")
    parser.add_argument("--probe-parameters", action="store_true", help="verify numeric editor readiness with audio stopped")
    args = parser.parse_args()

    dll_directory = None
    if args.tool_path:
        tools = args.tool_path.resolve()
        site.addsitedir(str(tools))
        sys.path.extend(str(tools / sub) for sub in ("win32", "win32/lib", "Pythonwin"))
        dll_directory = os.add_dll_directory(str(tools / "pywin32_system32"))

    from pywinauto import Application, timings

    artifacts = args.artifacts.resolve()
    artifacts.mkdir(parents=True, exist_ok=True)
    sandbox = artifacts / ("sandbox-" + uuid.uuid4().hex)
    appdata = sandbox / "Roaming"
    local = sandbox / "Local"
    temp = sandbox / "Temp"
    for folder in (appdata / "Rometric", local, temp):
        folder.mkdir(parents=True, exist_ok=True)
    config = {
        "auto_start_audio": False,
        "auto_reconnect": False,
        "minimize_to_tray": False,
        "spectrum_enabled": True,
        "reduced_motion": True,
        "preamp_db": -3.0,
        "presets": [{"name": "QA Desk", "profile": None, "sound": {"preamp_db": -3.0}}],
        "selected_preset": "QA Desk",
    }
    (appdata / "Rometric" / "config.json").write_text(json.dumps(config), encoding="utf-8")
    env = os.environ.copy()
    env.update(APPDATA=str(appdata), LOCALAPPDATA=str(local), TEMP=str(temp), TMP=str(temp))

    process = subprocess.Popen([str(args.exe.resolve())], env=env)
    window = None
    try:
        app = Application(backend="uia").connect(process=process.pid, timeout=30)
        spec = app.window(title="Rometric")
        spec.wait("visible", timeout=30)
        window = spec.wrapper_object()
        window.maximize()
        timings.wait_until(10, 0.1, window.is_maximized)
        window.set_focus()
        # Wait for actual application labels rather than a fixed startup sleep.
        timings.wait_until(15, 0.1, lambda: len(window.descendants(control_type="Text")) > 10)
        window.capture_as_image().save(artifacts / "maximized.png")
        devices = inspect_devices(spec, window, artifacts, timings) if args.list_devices else {}
        results = exercise(spec, window, appdata / "Rometric" / "config.json", artifacts, timings) if args.exercise else []
        parameter_probe = probe_parameters(spec, window, appdata / "Rometric" / "config.json", artifacts, timings) if args.probe_parameters else []
        tree = []
        for child in window.descendants():
            rect = child.rectangle()
            tree.append({
                "name": child.window_text(),
                "type": child.element_info.control_type,
                "rect": [rect.left, rect.top, rect.right, rect.bottom],
                "enabled": child.is_enabled(),
            })
        (artifacts / "uia-tree.json").write_text(json.dumps(tree, indent=2), encoding="utf-8")
        rect = window.rectangle()
        metadata = {
            "pid": process.pid,
            "dpi": ctypes.windll.user32.GetDpiForWindow(window.handle),
            "screen_pixels": [ctypes.windll.user32.GetSystemMetrics(0), ctypes.windll.user32.GetSystemMetrics(1)],
            "window_rect": [rect.left, rect.top, rect.right, rect.bottom],
            "sandbox": str(sandbox),
            "uia_elements": len(tree),
            "interaction_checks": results,
            "device_choices": devices,
            "parameter_probe": parameter_probe,
        }
        (artifacts / "metadata.json").write_text(json.dumps(metadata, indent=2), encoding="utf-8")
        print(json.dumps(metadata, indent=2))
    finally:
        if process.poll() is None:
            try:
                if window is not None:
                    window.close()
                process.wait(timeout=10)
            except Exception:
                process.kill()
                process.wait(timeout=10)
        if dll_directory is not None:
            dll_directory.close()


if __name__ == "__main__":
    main()
