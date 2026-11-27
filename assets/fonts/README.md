# Fonts

The fonts the TV's interface is drawn with (`src/paint/font.rs` compiles them in).

| File | Font | Source | Licence |
| --- | --- | --- | --- |
| `ArchivoNarrow-Bold.ttf` | Archivo Narrow, weight 700 | The 700 instance of the variable `ArchivoNarrow[wght].ttf` from [google/fonts](https://github.com/google/fonts/tree/main/ofl/archivonarrow) (SHA-256 `adbe027f…b23ef97f`), made with `fontTools.varLib.instancer` | SIL Open Font License 1.1, `ArchivoNarrow-OFL.txt` |
| `VCR_OSD_MONO_1.001.ttf` | VCR OSD Mono 1.001, by Riciery Leal | `vcr_osd_mono.zip` from the owner's research folder; the file carries no licence text | The owner stated on November 27, 2026 that the licence is granted |

To make the Archivo Narrow instance again:

```bash
uv run --with fonttools python -c "
from fontTools.ttLib import TTFont
from fontTools.varLib.instancer import instantiateVariableFont
instantiateVariableFont(TTFont('ArchivoNarrow[wght].ttf'), {'wght': 700}, updateFontNames=False).save('assets/fonts/ArchivoNarrow-Bold.ttf')"
```
