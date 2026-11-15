# Signal Bench TDD evidence

## Source

The user requested the complete 14-point Signal Bench redesign directly. No
external plan file was used.

## User journeys

- A listener can distinguish starting audio from bypassing correction.
- A listener can route input to output and understand stream health.
- A listener can find, favorite and reopen headphone profiles.
- A listener can browse profiles by brand or by the shape of their correction.
- A listener can save, import, export and automatically restore custom presets.
- A listener can undo experiments, reset a specific scope and compare A/B.
- A listener can see peaks, headroom, limiter activity and optional spectrum.
- A listener can recover from disconnected devices and inspect diagnostics.
- A listener can keep Rometric in the tray and optionally start it with Windows.

## RED/GREEN report

| Behavior | RED evidence | GREEN evidence |
|---|---|---|
| Product state, preset schema, favorites, history, device mapping, headroom, telemetry and reconnect policy | `cargo test --test product_features` failed with unresolved `product`, `telemetry` and `max_response_db` APIs | The same command passed after the APIs were implemented |
| Config migration and preset lifecycle | Focused suite failed because new config fields and preset methods did not exist | `cargo test --test product_features` passed with migration and lifecycle coverage |
| Limiter metering | Focused test failed because `limiter_gain` and `limiter_reduction_db` did not exist | `cargo test --test product_features limiter_exposes_gain_reduction_for_metering` passed |
| State-preserving DSP updates | Focused test failed because `EqChain::update_params` did not exist | `cargo test --test product_features live_parameter_updates_preserve_limiter_state` passed |
| Windows UTF-8 BOM settings | Focused test failed because `Config::from_json` did not exist | `cargo test --test product_features config_parser_accepts_windows_utf8_bom` passed |
| Profile categories | `cargo test --test profile_categories` failed because `wavelet_win::catalog` and its brand/signature APIs did not exist | The same command passed after `src/catalog.rs` was written |
| Category completeness on the real library | The real-library test failed until `AutoEq::catalog` existed | `real_library_is_fully_categorised` passed with 437 brand folders and proportionally sized sound folders |

## Final test specification

| Guarantee | Test or command | Type | Result |
|---|---|---|---|
| Imported sound values are finite and bounded | `imported_sound_settings_are_sanitized` | Unit | PASS |
| Preset files round-trip and reject incompatible schemas | `preset_files_round_trip_and_reject_unknown_schema` | Unit | PASS |
| Favorites toggle and recents remain deduplicated and bounded | `profile_library_tracks_favorites_and_bounded_recents` | Unit | PASS |
| Undo/redo supports branching and a bounded history | `edit_history_supports_undo_redo_and_branching` | Unit | PASS |
| Output mappings resolve and remove presets safely | `output_devices_can_restore_named_presets`; `config_upserts_and_removes_presets_without_stale_device_mappings` | Unit | PASS |
| Predicted response detects boost and safe preamp | `response_headroom_detects_boost_and_safe_preamp` | DSP integration | PASS |
| Telemetry exposes peak, limiter, xrun and spectrum state | `telemetry_reports_peaks_counters_and_latest_spectrum_samples` | Unit | PASS |
| Reconnect delay is bounded and resettable | `reconnect_backoff_is_bounded_and_resettable` | Unit | PASS |
| Live DSP updates preserve limiter state | `live_parameter_updates_preserve_limiter_state` | DSP integration | PASS |
| Legacy and BOM-prefixed settings migrate safely | `legacy_config_migrates_to_safe_product_defaults`; `config_parser_accepts_windows_utf8_bom` | Unit | PASS |
| Brands are derived from names and keep curated multi-word names whole | `brand_names_keep_multiword_brands_whole` | Unit | PASS |
| Correction curves are classified by their dominant region and direction | `signature_follows_the_shape_of_the_correction`; `signature_picks_the_dominant_region_when_several_move` | Unit | PASS |
| Every profile lands in exactly one brand folder and one sound folder | `catalog_places_every_profile_in_exactly_one_group`; `real_library_is_fully_categorised` | Integration (embedded DB) | PASS |
| Brand folders are alphabetical, single-profile brands are collated last | `brand_groups_are_alphabetical_and_singletons_are_collated_last` | Unit | PASS |
| Sound folders use a fixed order and omit empty buckets | `signature_groups_use_a_stable_order_and_omit_empty_buckets` | Unit | PASS |
| Search filters inside folders and drops empty ones | `search_filters_each_group_and_drops_empty_ones` | Unit | PASS |
| The browser opens on the flat list by default | `grouping_mode_defaults_to_the_flat_list` | Unit | PASS |
| Existing AutoEQ, DSP and resampling behavior remains intact | `cargo test --all-targets` | Regression | PASS: 44 tests |
| The application logo is square, transparent, visible and decodable | `tests/branding.rs` | Asset integration | PASS |
| Source is formatted and warning-free | `cargo fmt --all -- --check`; `cargo clippy --all-targets -- -D warnings` | Static | PASS |

## Visual verification

The native Windows application was launched with isolated `%APPDATA%` and
audio autostart disabled. The final light-neumorphic interface was reviewed at
1180×700 and 760×650 logical pixels (144 DPI). The wide layout keeps the profile
library, equalizer and independently scrolling inspector visible together. At
760 pixels the profile card remains beside the equalizer, the toolbar uses two
deliberate routing rows and the equalizer header moves history controls onto a
second row to avoid clipped text. Below that breakpoint, sections stack.

Runtime interaction checks exercised All/Brand/Sound grouping, profile search,
folder expansion, profile selection, correction bypass, the preamp control and
independent inspector scrolling. Selecting `Sony WH-1000XM2` updated the model
heading, all eleven bands, automatic preamp, response curve and headroom from
real application state. The lower inspector review covered Compare A/B, Custom
presets and every Application setting. No dummy display values were introduced.

Profile categories were reviewed in the running application at wide and compact
logical sizes: brand folders rendered alphabetically with counts,
folders expanded to starrable rows, searching `sony wh` reduced the tree to a
single auto-opened Sony folder, and the Sound view showed the seven correction
folders with its explanatory line. The five mode chips wrap onto two rows in the
narrow sidebar and stay on one row in the stacked layout.

## Known gaps

Automated UIA interaction was not added because this repository has no Python
desktop-test harness. Runtime launch and visual layout were verified directly;
native file-dialog, tray-menu and physical audio-device interactions still
require manual testing on the destination machine.
