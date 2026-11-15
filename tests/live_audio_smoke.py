"""Opt-in, low-level hardware verification using an isolated Rometric instance.

Requires explicit permission to pause an existing route and produce sound.
Feeds generated PCM into a named virtual cable; never opens a microphone,
changes Windows defaults, records input audio, or edits the listener's presets.
"""

import argparse
from array import array
import ctypes
from ctypes import wintypes
import json
import math
import os
from pathlib import Path
import site
import subprocess
import sys
import uuid


class WaveFormat(ctypes.Structure):
    _pack_ = 1
    _fields_ = [("tag", wintypes.WORD), ("channels", wintypes.WORD),
                ("rate", wintypes.DWORD), ("bytes_per_second", wintypes.DWORD),
                ("block_align", wintypes.WORD), ("bits", wintypes.WORD), ("extra", wintypes.WORD)]


class WaveHeader(ctypes.Structure):
    _fields_ = [("data", ctypes.c_void_p), ("length", wintypes.DWORD),
                ("recorded", wintypes.DWORD), ("user", ctypes.c_size_t),
                ("flags", wintypes.DWORD), ("loops", wintypes.DWORD),
                ("next", ctypes.c_void_p), ("reserved", ctypes.c_size_t)]


class WaveCaps(ctypes.Structure):
    _fields_ = [("manufacturer", wintypes.WORD), ("product", wintypes.WORD),
                ("version", wintypes.DWORD), ("name", wintypes.WCHAR * 32),
                ("formats", wintypes.DWORD), ("channels", wintypes.WORD),
                ("reserved", wintypes.WORD), ("support", wintypes.DWORD)]


class CableSignal:
    def __init__(self, prefix):
        self.mm = ctypes.WinDLL("winmm")
        self.mm.waveOutGetDevCapsW.argtypes = [ctypes.c_size_t, ctypes.POINTER(WaveCaps), wintypes.UINT]
        self.mm.waveOutOpen.argtypes = [ctypes.POINTER(ctypes.c_void_p), wintypes.UINT,
                                       ctypes.POINTER(WaveFormat), ctypes.c_size_t, ctypes.c_size_t, wintypes.DWORD]
        for name in ("waveOutPrepareHeader", "waveOutWrite", "waveOutUnprepareHeader"):
            getattr(self.mm, name).argtypes = [ctypes.c_void_p, ctypes.POINTER(WaveHeader), wintypes.UINT]
        self.mm.waveOutReset.argtypes = [ctypes.c_void_p]
        self.mm.waveOutClose.argtypes = [ctypes.c_void_p]
        candidates = []
        for device in range(self.mm.waveOutGetNumDevs()):
            caps = WaveCaps()
            if self.mm.waveOutGetDevCapsW(device, ctypes.byref(caps), ctypes.sizeof(caps)) == 0:
                if caps.name.startswith(prefix):
                    candidates.append((device, caps.name))
        if len(candidates) != 1:
            raise RuntimeError(f"Expected one virtual playback endpoint starting with {prefix!r}, got {candidates}")
        device, self.endpoint = candidates[0]
        # 0.004 per tone, 12 tones: peak is bounded below 0.048 (-26.4 dBFS).
        # Rometric's QA preamp adds another 12 dB attenuation before playback.
        frequencies = [63, 125, 250, 500, 750, 1000, 1500, 2000, 4000, 8000, 12000, 16000]
        pcm = array("h")
        for index in range(48000):
            left = sum(0.004 * math.sin(math.tau * hz * index / 48000 + band * 0.71)
                       for band, hz in enumerate(frequencies))
            right = sum(0.003 * math.sin(math.tau * hz * index / 48000 + band * 0.71 + 0.3)
                        for band, hz in enumerate(frequencies))
            pcm.extend((round(left * 32767), round(right * 32767)))
        self.buffer = ctypes.create_string_buffer(pcm.tobytes())
        self.header = WaveHeader(data=ctypes.addressof(self.buffer), length=len(pcm) * 2,
                                 flags=0x4 | 0x8, loops=45)
        self.handle = ctypes.c_void_p()
        fmt = WaveFormat(1, 2, 48000, 192000, 4, 16, 0)
        result = self.mm.waveOutOpen(ctypes.byref(self.handle), device, ctypes.byref(fmt), 0, 0, 0)
        if result:
            raise RuntimeError(f"Could not open virtual PCM source: MMRESULT {result}")
        try:
            result = self.mm.waveOutPrepareHeader(self.handle, ctypes.byref(self.header), ctypes.sizeof(self.header))
            if result:
                raise RuntimeError(f"Could not prepare PCM: MMRESULT {result}")
            result = self.mm.waveOutWrite(self.handle, ctypes.byref(self.header), ctypes.sizeof(self.header))
            if result:
                raise RuntimeError(f"Could not send PCM: MMRESULT {result}")
        except Exception:
            self.close()
            raise

    def close(self):
        if self.handle:
            self.mm.waveOutReset(self.handle)
            self.mm.waveOutUnprepareHeader(self.handle, ctypes.byref(self.header), ctypes.sizeof(self.header))
            self.mm.waveOutClose(self.handle)
            self.handle = ctypes.c_void_p()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--exe", type=Path, required=True)
    parser.add_argument("--artifacts", type=Path, required=True)
    parser.add_argument("--tool-path", type=Path, required=True)
    parser.add_argument("--existing-pid", type=int)
    parser.add_argument("--allow-playback", action="store_true", required=True)
    args = parser.parse_args()
    tools = args.tool_path.resolve()
    site.addsitedir(str(tools))
    sys.path.extend(str(tools / sub) for sub in ("win32", "win32/lib", "Pythonwin"))
    dll_directory = os.add_dll_directory(str(tools / "pywin32_system32"))
    from pywinauto import Application, keyboard, timings

    artifacts = args.artifacts.resolve()
    artifacts.mkdir(parents=True, exist_ok=True)
    sandbox = artifacts / ("sandbox-" + uuid.uuid4().hex)
    config_path = sandbox / "Roaming" / "Rometric" / "config.json"
    config_path.parent.mkdir(parents=True, exist_ok=True)
    capture = "CABLE Output (VB-Audio Virtual Cable)"
    render = "Speakers / Headphones (Conexant SmartAudio HD)"
    gains = [0.0] * 11
    gains[5] = 6.0
    sound = {"gains": gains, "preamp_db": -12.0, "parametric_enabled": True,
             "crossfeed": 0.35, "auto_preamp": False}
    config = {**sound, "capture_device": capture, "render_device": render,
              "auto_start_audio": False, "auto_reconnect": False, "minimize_to_tray": False,
              "spectrum_enabled": True, "reduced_motion": False,
              "presets": [{"name": "QA Live", "profile": None, "sound": sound}],
              "selected_preset": "QA Live"}
    config_path.write_text(json.dumps(config), encoding="utf-8")
    env = os.environ.copy()
    env["APPDATA"] = str(sandbox / "Roaming")
    env["LOCALAPPDATA"] = str(sandbox / "Local")
    env["TEMP"] = env["TMP"] = str(sandbox / "Temp")
    for directory in (env["LOCALAPPDATA"], env["TEMP"]):
        Path(directory).mkdir(parents=True, exist_ok=True)

    old = None
    old_paused = False
    old_minimized = False
    process = None
    window = None
    signal = None
    report = {"capture": capture, "render": render, "input_is_microphone": False,
              "source": "generated, peak-bounded stereo multi-tone PCM into VB-CABLE",
              "windows_defaults_changed": False, "checks": []}
    try:
        if args.existing_pid:
            previous = Application(backend="uia").connect(process=args.existing_pid, timeout=5)
            old = previous.window(title="Rometric")
            stop = old.child_window(title="Stop audio", control_type="Button")
            if stop.exists():
                if not stop.is_enabled():
                    raise RuntimeError("The existing route is protected by a dialog; refusing to interrupt it")
                old_minimized = old.wrapper_object().is_minimized()
                if old_minimized:
                    old.restore()
                stop.click_input()
                old.child_window(title="Start audio", control_type="Button").wait("visible", timeout=10)
                old_paused = True
            elif any(control.window_text().startswith(("Processing ·", "Bypassed ·")) for control in old.descendants(control_type="Text")):
                raise RuntimeError("Cannot access the existing route's Stop control; refusing to risk a feedback loop")
        report["existing_route_temporarily_paused"] = old_paused
        process = subprocess.Popen([str(args.exe.resolve())], env=env)
        app = Application(backend="uia").connect(process=process.pid, timeout=30)
        spec = app.window(title="Rometric")
        spec.wait("visible", timeout=30)
        window = spec.wrapper_object()
        window.maximize()
        timings.wait_until(10, 0.1, window.is_maximized)
        window.set_focus()

        def read_config():
            try:
                return json.loads(config_path.read_text(encoding="utf-8"))
            except (OSError, ValueError):
                return {}

        def wait_config(predicate):
            timings.wait_until(12, 0.1, lambda: predicate(read_config()))

        # Force a normal UI save BEFORE opening audio, to check the selections
        # actually accepted by App::new (not just the requested config strings).
        mode = spec.child_window(title="Parametric EQ", control_type="CheckBox")
        mode.wait("visible", timeout=10)
        mode.click_input()
        wait_config(lambda cfg: cfg.get("parametric_enabled") is False)
        mode.click_input()
        wait_config(lambda cfg: cfg.get("parametric_enabled") is True)
        actual = read_config()
        if actual.get("capture_device") != capture or actual.get("render_device") != render:
            raise RuntimeError("Requested non-microphone/non-loopback route was unavailable; audio was not started")
        report["checks"].append("actual UI route verified before opening audio")
        signal = CableSignal("CABLE Input (")
        report["pcm_playback_endpoint"] = signal.endpoint
        spec.child_window(title="Start audio", control_type="Button").click_input()
        spec.child_window(title="Stop audio", control_type="Button").wait("visible", timeout=10)

        def finite_meter_labels():
            return [control.window_text() for control in window.descendants(control_type="Text")
                    if "dBFS" in control.window_text() and "∞" not in control.window_text()
                    and control.window_text().split(" ")[0].lstrip("-").replace(".", "", 1).isdigit()]

        timings.wait_until(12, 0.1, lambda: len(finite_meter_labels()) >= 2)
        report["peak_readouts"] = finite_meter_labels()
        report["checks"].append("real capture/render callbacks supply finite input/output dBFS")

        def colored_spectrum_pixels(image):
            marker = spec.child_window(title="Live spectrum", control_type="CheckBox").rectangle()
            shaping = spec.child_window(title="Sound shaping", control_type="Text").rectangle()
            root = window.rectangle()
            crop = image.crop((max(0, marker.left - root.left - 12), marker.bottom - root.top + 10,
                               shaping.left - root.left - 40, image.height - 20))
            return sum(max(pixel[:3]) > 90 and min(pixel[:3]) < 90 and max(pixel[:3]) - min(pixel[:3]) > 70
                       for pixel in crop.get_flattened_data())

        timings.wait_until(12, 0.1, lambda: colored_spectrum_pixels(window.capture_as_image()) > 40)
        live_image = window.capture_as_image()
        report["colored_spectrum_pixels"] = colored_spectrum_pixels(live_image)
        live_image.save(artifacts / "live-spectrum.png")
        report["checks"].append("real output audio renders colored rainbow dots in the existing centre card")

        def edit_parameter(label, value):
            label_rect = spec.child_window(title=label, control_type="Text").rectangle()
            candidates = [control for control in window.descendants()
                          if control.element_info.control_type in ("Spinner", "Custom", "Edit")
                          and control.rectangle().left >= label_rect.right
                          and control.rectangle().top <= label_rect.bottom
                          and control.rectangle().bottom >= label_rect.top]
            control = min(candidates, key=lambda item: item.rectangle().left)
            control.iface_range_value.SetValue(float(value))

        edit_parameter("Freq", "1500")
        wait_config(lambda cfg: cfg.get("band_settings", [{}] * 11)[5].get("frequency_hz") == 1500.0)
        edit_parameter("Q", "3.0")
        wait_config(lambda cfg: cfg.get("band_settings", [{}] * 11)[5].get("q") == 3.0)
        q_rect = spec.child_window(title="Q", control_type="Text").rectangle()
        types = [control for control in window.descendants(control_type="ComboBox")
                 if control.rectangle().left > q_rect.right
                 and control.rectangle().top <= q_rect.bottom
                 and control.rectangle().bottom >= q_rect.top]
        type_combo = min(types, key=lambda item: item.rectangle().left)
        type_combo.click_input()
        spec.child_window(title="High shelf").wait("visible", timeout=10)
        spec.child_window(title="High shelf").click_input()
        wait_config(lambda cfg: cfg.get("band_settings", [{}] * 11)[5].get("kind") == "HighShelf")
        spec.child_window(title="Enabled", control_type="CheckBox").click_input()
        wait_config(lambda cfg: cfg.get("band_settings", [{}] * 11)[5].get("enabled") is False)
        spec.child_window(title="Enabled", control_type="CheckBox").click_input()
        wait_config(lambda cfg: cfg.get("band_settings", [{}] * 11)[5].get("enabled") is True)
        type_combo.click_input()
        spec.child_window(title="Peak").wait("visible", timeout=10)
        spec.child_window(title="Peak").click_input()
        wait_config(lambda cfg: cfg.get("band_settings", [{}] * 11)[5].get("kind") == "Peak")
        report["checks"].append("live frequency, Q, filter type and band Enabled controls persist real settings")
        window.capture_as_image().save(artifacts / "live-parametric-controls.png")

        # The existing A/B controls, not a second comparison component.
        spec.child_window(title="B").click_input()
        spec.child_window(title="Flat bands", control_type="Button").click_input()
        wait_config(lambda cfg: cfg.get("gains", [1.0] * 11)[5] == 0.0)
        spec.child_window(title="A").click_input()
        wait_config(lambda cfg: cfg.get("gains", [0.0] * 11)[5] == 6.0)
        spec.child_window(title="Level match A/B", control_type="CheckBox").click_input()
        timings.wait_until(12, 0.1, lambda: any(control.window_text().startswith("RMS trim") for control in window.descendants(control_type="Text")))
        report["level_match_readout"] = next(control.window_text() for control in window.descendants(control_type="Text") if control.window_text().startswith("RMS trim"))
        report["checks"].append("real stereo-input RMS calibration reaches the existing A/B panel")
        window.capture_as_image().save(artifacts / "live-level-matched.png")

        spec.child_window(title="Restart", control_type="Button").click_input()
        timings.wait_until(12, 0.1, lambda: len(finite_meter_labels()) >= 2
                           and any(control.window_text().startswith("Processing ·") for control in window.descendants(control_type="Text"))
                           and any(control.window_text().startswith("RMS trim") for control in window.descendants(control_type="Text")))
        report["checks"].append("Restart reopens the real route and meters remain active")
        spec.child_window(title="Stop audio", control_type="Button").click_input()
        spec.child_window(title="Start audio", control_type="Button").wait("visible", timeout=10)
        timings.wait_until(10, 0.1, lambda: sum("∞" in control.window_text() and "dBFS" in control.window_text() for control in window.descendants(control_type="Text")) >= 2)
        timings.wait_until(10, 0.1, lambda: colored_spectrum_pixels(window.capture_as_image()) < 5)
        report["checks"].append("Stop audio clears levels and live spectrum data")
        window.capture_as_image().save(artifacts / "stopped-after-live-test.png")
    finally:
        if signal is not None:
            signal.close()
        if process is not None and process.poll() is None:
            try:
                if window is not None:
                    window.close()
                process.wait(timeout=10)
            except Exception:
                process.kill()  # This script's isolated process only.
                process.wait(timeout=10)
        if old_paused and old is not None and old.exists():
            try:
                start = old.child_window(title="Start audio", control_type="Button")
                if start.exists():
                    start.click_input()
                    old.child_window(title="Stop audio", control_type="Button").wait("visible", timeout=10)
                if old_minimized:
                    old.minimize()
                report["existing_running_state_restored"] = True
            except Exception as error:
                report["existing_running_state_restored"] = False
                report["restore_error"] = str(error)
        else:
            report["existing_running_state_restored"] = not old_paused
        (artifacts / "hardware-report.json").write_text(json.dumps(report, indent=2), encoding="utf-8")
        dll_directory.close()
    print(json.dumps(report, indent=2))
    if not report["existing_running_state_restored"]:
        raise RuntimeError("Could not restore the listener's running audio state; see hardware-report.json")


if __name__ == "__main__":
    main()
