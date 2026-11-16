# Audio workstation additions — TDD and verification evidence

## Scope and approvals

The user requested four polish features, parametric EQ, headphone crossfeed,
level-matched A/B and the attached rainbow-spectrum style. They explicitly
approved targeted DSP, shared-parameter and persistence changes, and later a
brief low-level hardware test. Existing components were extended: the spectrum
is inside the centre equalizer card, crossfeed is in Sound shaping, matching is
in Compare A/B, and confirmation is part of the existing preset workflow.

No source plan was supplied. No Rust dependencies were introduced. Source
snapshots were saved outside the project before editing; the workspace has no
Git repository. All native tests used isolated AppData and disabled autostart.

## User journeys / acceptance mapping

| ID | Observable guarantee | Evidence |
|---|---|---|
| AC-001 | Existing config and schema-1 presets load with new audio effects off; defaults retain the graphic response | `tests/advanced_audio.rs`; original regression suite |
| AC-002 | The same eleven bands support frequency, gain, Q, shelf/peak type and per-band Enabled; graph and faders remain real controls | DSP tests; actual egui pointer/keyboard tests; native hardware UI actions |
| AC-003 | Crossfeed is frequency dependent, preserves mono, and is inactive at 0% | `crossfeed_*` tests; native Crossfeed action |
| AC-004 | A/B matching uses the same real stereo window, only attenuates, remains temporary, and waits for usable audio | `level_match_*` tests; live RMS trim readout |
| AC-005 | Meters show finite dBFS or −∞ at silence; the spectrum follows real frequency/amplitude and clears after Stop | `tests/audio_polish.rs`; real capture/render and pixel checks |
| AC-006 | Unsaved status follows current sound/profile; Save and Import cannot overwrite without confirmation | product tests; application tests; native Cancel/Replace checks |
| AC-007 | F1/Help works; large client areas show all eleven faders and integrated spectrum; small areas use bounded internal overflow | `src/app/tests.rs`; native framebuffer matrix |

## RED → GREEN evidence

| Behavior | RED | GREEN |
|---|---|---|
| Advanced DSP, analysis, persistence, overwrite contracts | `cargo test --test advanced_audio --test audio_polish` failed with missing analysis APIs, new fields and confirmation result | Same tests passed after implementation |
| UI integration, overwrite behavior and responsive geometry | `cargo test --bin rometric` failed with missing UI APIs; initial rendered geometry also caught a small spectrum overflow and narrow-column overflow | Actual remaining-height allocation and content-driven compact navigation made the same tests pass |
| Complete correction bypass versus level matching | Compiled focused test failed because calibration was still claimed for a fully bypassed slot | Helper now refuses that combination; UI explains using a flat enabled slot instead |
| Numeric keyboard entry | Added direct egui pointer/focus/incremental decimal test | `q_editor_accepts_incremental_keyboard_decimal_entry` passed |
| Stale analysis after route restart | Focused test failed with missing clearing operation | `clear_route_analysis` is used on successful Start/Restart and Stop; test passed without changing user sound/preferences |

## Final automated verification

- `cargo test --target-dir target/feature-validation`: **89 passed**, no failed,
  ignored or filtered tests in the complete run.
- `cargo build --release --bin rometric`: **PASS**, optimized executable at
  `target/release/rometric.exe`.
- `cargo run --release --bin selftest`: **PASS**, including the 6,029 profiles,
  original response checks, limiter ceiling and numerical stability.
- The optimized `target/release/rometric.exe` also launched successfully in the
  isolated native UI smoke test at the host's real 144 DPI.
- `cargo fmt --all -- --check`: **PASS**.
- `cargo clippy --all-targets --target-dir target/feature-validation -- -D warnings`:
  **PASS** (re-run after the final changes).
- Python QA helpers compile with `python -m py_compile`.
- Rust source grep found no credential literals, `todo!`, `unimplemented!`,
  TODO/FIXME markers or newly introduced unsafe blocks.
- Source review uses the pre-edit snapshot and a local `difflib` comparison;
  neither a Git repository nor a Git executable is available in this environment.

## Native GUI and screenshot evidence

`tests/desktop_smoke.py --exercise` exercised the running application, not mock
controls. It verified F1/Escape, Ctrl+F search against all 6,029 profiles,
parametric mode, an actual fader drag, persistence and Unsaved changes, Cancel
without preset mutation, explicit Replace, Crossfeed, matching's waiting state,
Reload devices, restore and maximize.

`examples/capture_ui.rs` reuses the actual application and captures its OpenGL
framebuffer at these physical client sizes and egui scale factors:

| Client pixels | 100% | 125% | 150% |
|---|---|---|---|
| 1280×720 | Captured | Captured | Captured |
| 1366×768 | Captured | Captured | Captured |
| 1600×900 | Captured | Captured | Captured |
| 1920×1080 | Captured | Captured | Captured |

All twelve PNG dimensions were checked against their requested client sizes.
The native host DPI was 144 (150%); 100% and 125% were **egui scaling simulations**,
not changes to Windows display settings. At 1600×900 and 1920×1080 logical areas,
headless rendered-widget tests verify all eleven fader rectangles, the spectrum,
Compare and Presets inside the viewport without centre scrolling. At smaller
effective areas, the toolbar remains outside scrolling; profile lists and
overflowing sections scroll internally, or use compact navigation.

Artifacts (not committed):
- `target/ui-validation/matrix/captures.json` and twelve size/scale PNGs.
- `target/ui-validation/functional/help.png`.
- `target/ui-validation/functional/overwrite-confirmation.png`.
- `target/ui-validation/functional/interactions.json`.
- `target/ui-validation/parameter-probe/parameter-probe.json`.
- `target/ui-validation/live/live-level-matched.png`.
- `target/ui-validation/live/hardware-report.json`.

## Real audio hardware check

The user explicitly approved this test. The owned, isolated application used
`CABLE Output (VB-Audio Virtual Cable)` as capture and
`Speakers / Headphones (Conexant SmartAudio HD)` as render. A named virtual
playback endpoint received generated, bounded stereo multi-tone PCM. The source
peak was bounded below 0.048, and the QA preamp added 12 dB attenuation.

Observed evidence:
- Actual UI selections were verified **before** opening a stream, preventing
  silent fallback to the microphone or a cable feedback route.
- Real capture/render operated at **48,000 Hz / 2 channels**.
- Example sample-peak readouts: **−29.5 dBFS input**, **−39.7 dBFS output**.
- Pixel inspection found **1,596 colored spectrum pixels** driven by real audio.
- Frequency = 1500 Hz, Q = 3.0, High shelf/Peak and band Enabled changes persisted
  through real native actions while audio was running.
- A/B calibration displayed **RMS trim −1.2 dB**.
- Restart continued real processing and refreshed calibration; Stop cleared
  peaks and colored spectrum pixels.
- The generated source and owned application were closed. No existing Rometric
  process was running at the successful check, so no user route needed pausing.
  Windows defaults and listener presets were never changed. No microphone was
  opened and no captured audio was written to disk.

## Contained automation recovery

The first raw-key Q-entry check did not retain the intended value. A subsequent
probe incorrectly waited for a UIA Edit element. Inspection of egui's DragValue
implementation showed that it intentionally keeps its **SpinButton** role and
supports numeric SetValue. The no-audio probe then used the actual UIA RangeValue
action and passed both frequency/Q persistence checks. A separate direct egui
decimal-keyboard test also passed. The production control was not replaced.

The screenshot harness needed a final-frame closing guard. A later batch was
too slow while release compilation saturated the host; file timestamps showed
progress rather than a stuck screenshot request. Running framebuffer capture
after the build finished completed successfully. These were QA-harness issues,
not permission to substitute placeholder components or invent passing evidence.

## Known limits

- Coverage percentage is **not measured**: a coverage tool is not installed.
  No 80% coverage claim is made.
- RMS matching is pre-limiter sample RMS, not LUFS, true peak or acoustic/
  perceptual loudness measurement. The 24 dB safety cap is reported if reached.
- Spectrum input is the existing mono output telemetry; it is not a separate
  stereo analyser or a spectrogram history.
- Other audio hardware and physical monitor/DPI transitions need destination-
  machine verification. Windows startup registry behavior and external native
  Import/Export file-picker interaction were not exercised against user data;
  preset schema and the shared overwrite/import guard have automated coverage.

## Visual refresh — bento modular dashboard

After the functional work, the user asked for a modern UI and chose a **bento /
modular dashboard** across the whole app, with a bundled font family. The
previous light neumorphic identity was deliberately replaced; no audio,
preset, routing or persistence behavior changed.

| Change | Implementation | Verification |
|---|---|---|
| Neutral bento surfaces | `theme.rs` palette now uses a neutral canvas, opaque white tiles, hairline borders and one soft downward shadow; `paint_inset_with` is contrast-aware for the dark display | Twelve framebuffer captures at four client sizes × three egui scales |
| Module hierarchy | Titled tiles gained a hairline header rule; uppercase eyebrows label the spectrum region; type scale and weights use Inter SemiBold | Native UIA tree and visual capture review |
| Bundled typography | `assets/fonts/` contains Inter Regular/SemiBold and JetBrains Mono (SIL OFL 1.1 with licences); `FontSet` is embedded via `include_bytes!` and installed before the first frame | 89 tests pass with production font metrics in headless geometry tests; strict Clippy clean |
| Spectrum display | The former near-black slab became a rounded navy instrument tile with decade grid lines, keeping the real FFT dots and absolute hover dBFS | Image review at 1600×900, 1280×720 and 150% scaling |

Focused searches through the `ui-ux-pro-max` catalog supported Minimalism/Swiss
(high-contrast typography, grid rhythm, visible focus), Bento grids (16–24 px
radii, subtle shadows, varied tile hierarchy) and dark instrument surfaces for
the spectrum. Its landing-page "Hero + CTA" pattern was deliberately **not**
applied — it does not describe a desktop audio tool. Responsive navigation and
the eleven-fader geometry checks still pass at all captured sizes.

## Delivery self-evaluation

Summary: **4.2/5** — requested implementation and verification are delivered;
remaining limits concern broader-device validation and measurement, not missing
or dummy controls.

| Axis | Score | Evidence / improvement |
|---|---|---|
| Accuracy | 4 | 89 tests, strict Clippy, release selftest and real PCM checks pass. Matching is intentionally sample RMS; validate perceptual listening separately rather than claiming LUFS equivalence. |
| Completeness | 4 | All selected features, overwrite/import guards and twelve captures are present. Other physical monitor/DPI transitions and device combinations remain untested. |
| Clarity | 4 | README and F1 help explain controls and RMS/visual-mirror caveats. The large existing `app.rs` could be divided into focused UI modules in a future maintenance task. |
| Actionability | 5 | The optimized executable exists at `target/release/rometric.exe`; feature locations, screenshots and reproducible commands are provided. |
| Conciseness | 4 | The UI reuses its existing cards and state. Development-only QA scripts still duplicate some setup that could be shared later. |

Optional follow-ups, by impact: test additional real monitors/audio endpoints;
measure coverage using a dedicated coverage tool; consolidate common QA setup.
No critical axis is below 3. Self-check: the user can run the requested features
immediately and inspect the evidence, with the verification limits stated.
