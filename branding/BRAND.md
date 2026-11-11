# Yas Browser

**A clearer view of the web.**

Yas Browser is a modern blue browser identity inspired by the UAE's tradition of falconry. Its original **Falcon Wing** symbol combines a white falcon silhouette, a closed hooked beak, a swept wing, and a sky-blue rear feather on a rounded royal-blue tile. The open feather detail, angled eye, and cheek marking give it a clear silhouette at small sizes. It contains no letter monogram, official insignia, or borrowed government or airline emblem.

## Visual identity

| Role | Color |
| --- | --- |
| Royal blue: icon tile | `#2563EB` |
| Interface blue: primary actions | `#2F6EE7` |
| White: falcon silhouette and button text | `#FFFFFF` |
| Sky blue: rear wing | `#9BC8FF` |
| Bright blue: feather detail | `#3478F6` |
| Deep blue: eye and cheek marking | `#154BD2` |
| Navy: regular headings and body text | `#192C49` |
| Ice: regular interface surfaces | `#F7F9FC` |
| Slate blue: secondary text | `#5F708A` |
| Soft blue: borders | `#DCE3EE` |

**Typeface:** Dubai for the wordmark, headings, controls, and body text. The browser uses the installed font and does not redistribute its font files. Segoe UI is the fallback. Websites control their own typography.

**Voice:** helpful, direct, quiet. Prefer “A clearer view of the web”, “Less noise”, and “Room to breathe”. Avoid unsupported speed, privacy, or memory superlatives.

**Stealth mode:** retain the dark navy theme, pale readable text, dark tiles, and mask-shaped tab badge. Keep the name **Stealth mode** in controls and help text. Local history and session restoration are excluded; saved bookmarks and downloaded files remain. Websites and the network can still see activity.

## Assets

- `yas-symbol.svg`: vector master, also embedded in the browser interface.
- `yas-logo.png`: 1024px raster rendered from the vector, with transparent corners.
- `yas-wordmark.svg`: Dubai wordmark and Falcon Wing symbol.
- `yas-wordmark.png`: transparent 700px lockup rendered with the installed Dubai Bold font.
- `yas.ico`: Windows app icon with 16, 20, 24, 32, 40, 48, 64, 128, and 256px renditions.
- `icon.rgba`: 64px window/taskbar icon pixels.
- `render-assets.py`: reproducible SVG-to-PNG/ICO/RGBA renderer using PyMuPDF and Pillow.
- `render-wordmark.py`: wordmark renderer using the local Dubai Bold font; font files are not bundled.
- `Yas-preview.png` and `Yas-stealth-preview.png`: live application previews.

Keep clear space of at least one eighth of the tile width around the mark. Preserve the falcon's silhouette, white and sky-blue wings, and royal-blue tile. Do not stretch it, add flag bands or official insignia, or replace it with a letter badge. The plain wordmark is **Yas**, with this capitalization. Use **YAS** only in uppercase interface labels such as **YAS SHIELD**.
