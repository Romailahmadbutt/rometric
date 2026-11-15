# Data provenance

`logo.svg` and the derived `logo.png` / `logo.ico` application assets were
provided by the project owner as the Rometric visual identity on 2026-11-14.

`autoeq.bin.z` is a repacked copy of the AutoEQ correction database that ships
inside Wavelet 26.05.

- Source: `assets/auto_eq.db` from `base.apk`
  (`com.pittvandewitt.wavelet`, versionCode 260508)
- Source SHA-256: `09980e4da409d503cbddf9b54bd3b2511b27ae280d8978ad0b74b34e80816524`
- Rows: 6,029 (`SELECT headphone, equalization FROM autoeq`)
- Packed SHA-256: `39ecaa01b94798d1629a96997e6495f1b650a9200ebfc6ee148d2c10b4cb0c8c`

The original table is `autoeq(headphone TEXT, equalization TEXT)` where
`equalization` is a `frequency gain; frequency gain; …` list with 127 points
from 20 Hz to 19,871 Hz. Every row shares the same frequency grid, so the packed
format stores the grid once and the gains as i16 tenths of a dB, zlib-compressed.

To regenerate from an extracted database:

```python
import sqlite3, struct, zlib
c = sqlite3.connect('auto_eq.db')
rows = list(c.execute("SELECT headphone, equalization FROM autoeq ORDER BY headphone COLLATE NOCASE"))
grid = None; profiles = []
for name, eq in rows:
    pts = [tuple(map(float, s.split())) for s in eq.split(';')]
    f = [p[0] for p in pts]; g = [p[1] for p in pts]
    grid = grid or f
    assert f == grid
    profiles.append((name.encode(), [int(round(x * 10)) for x in g]))
raw = bytearray(b'WTQ1') + struct.pack('<II', len(profiles), len(grid))
for fr in grid: raw += struct.pack('<f', fr)
for name, gains in profiles:
    raw += struct.pack('<B', len(name)) + name + struct.pack(f'<{len(gains)}h', *gains)
open('assets/autoeq.bin.z', 'wb').write(zlib.compress(bytes(raw), 9))
```
