# Bundled typefaces

| File | Family | Source | Licence |
|---|---|---|---|
| `Inter-Regular.ttf` | Inter Regular | [rsms/inter v4.1](https://github.com/rsms/inter/releases/tag/v4.1), `extras/ttf/` | SIL OFL 1.1 — `Inter-OFL.txt` |
| `Inter-SemiBold.ttf` | Inter SemiBold | same release | SIL OFL 1.1 — `Inter-OFL.txt` |
| `JetBrainsMono-Variable.ttf` | JetBrains Mono | [google/fonts `ofl/jetbrainsmono`](https://github.com/google/fonts/tree/main/ofl/jetbrainsmono) | SIL OFL 1.1 — `JetBrainsMono-OFL.txt` |

The files are embedded into the executable with `include_bytes!` in
`src/app/theme.rs`, so no font files are loaded at runtime.

Both licences permit bundling and redistribution. The fonts are used unmodified
(apart from the JetBrains Mono variable face being used at its default weight).
The full licence text is included next to each font.
