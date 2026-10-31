---
title: "Settings and keys"
---

Settings, keyboard shortcuts, supported formats and the files vw makes.

## Settings

The gear at the bottom left, or **Settings → Settings** (or **Ctrl+,**), switches to the settings.
**Settings → Open the settings folder** / **Open the AI models folder** opens the folder holding the settings file, logs, people records and AI models.

- Type in the search box at the top to narrow the settings (by English name, Japanese name or setting key)
- Changes apply at once and are saved automatically. There is no save button
- **Open settings file** opens the settings file (TOML) in the default editor. When you save it in the editor, vw rereads it automatically

![Settings narrowed to "slideshow". Each setting shows its description and its key in the settings file](../../images/settings-search.png)

### Main settings

The main settings. Find all of them with the search box of the settings screen.

| Group | Setting (key) | Meaning | Default |
| --- | --- | --- | --- |
| Appearance | Theme (`appearance.theme`) | System / Dark / Light | System |
| | Language (`appearance.language`) | English / 日本語 | The OS language at first start (English unless Japanese) |
| | UI scale (`appearance.ui_scale`) | Size of text and icons | 1.00 |
| | Compact windows (`appearance.compact`) | Less padding to show more information | On |
| Viewer | Background color (`viewer.background`) | Color behind images (`#RRGGBB`). Empty for the theme default | Empty |
| | Full-resolution memory limit (`viewer.full_image_limit_mb`) | Memory for full-resolution images. Beyond it, 512 px thumbnails are shown | 256 MB |
| | Slot layout (`viewer.layout`) | Auto / 1x1 / 1x2 / 2x1 / 2x2 / 3x3 / 4x4 / Custom | Auto |
| | Custom layout (`viewer.custom_layout`) | Columns x rows for "Custom" in the View menu | 6 x 6 |
| | Auto layout maximum (`viewer.auto_layout_max`) | Largest columns x rows for the auto layout. More slots go to the next page | 6 x 6 |
| | Wheel scroll amount (`viewer.page_wheel`) | How far the wheel over the page bar, and Shift+wheel over slots, scrolls: one slot, half a page, one page | One page |
| | Slideshow speed (`viewer.slideshow_fps`) | Frames per second (0.1 to 120) | 30 |
| | Slideshow end (`viewer.slideshow_end`) | Loop / Stop at end / Ping-pong | Loop |
| | Dropped frames period (`viewer.slideshow_drop_window_secs`) | Over how many recent seconds the play bar counts dropped frames | 5 s |
| | Keep zoom when stepping (`viewer.keep_view`) | Keep zoom and position when stepping through a loaded folder slot | On |
| | Tool at startup (`viewer.start_tool`) | Last used, Select, Hand, Zoom | Last used |
| | Develop RAW in the background (`viewer.develop_raw`) | Show camera RAW with its preview, then develop it in the background and swap | On |
| | Keep the RAW preview after developing (`viewer.keep_raw_preview`) | Keep the camera's embedded preview after developing so J can switch | On |
| | Burst interval (`similar.burst_seconds`) | Photos taken in a row with the same camera within this many seconds form one similar group. 0 for the same second only | 2 s |
| | Look-alike tolerance (`similar.similar_bits`) | Images whose visual fingerprints (64 bits) differ in this many bits or fewer form a similar group. 0 for near duplicates only | 10 |
| Folder tree | Sort order (`tree.sort`) | Name (numbers by value) / Name (character by character) / Modified date | Modified date |
| | Descending (`tree.sort_descending`) | Reverse the order (Z to A, newest first) | On |
| | Confirm large loads (`tree.confirm_every`) | Ask whether to continue every chunk | Off |
| | Chunk size (`tree.confirm_chunk`) | Images loaded before asking again | 500 |
| | Load folders into (`tree.load_into`) | One slot per image / One slot per folder | One slot per image |
| | Key for the other way (`tree.other_load_modifier`) | Loading a folder with this key held uses the other way | Alt |
| Behavior | Group heading click (`behavior.header_click`) | Clicking a metadata window heading: Select only / Select and expand/collapse | Select and expand/collapse |
| | Auto reload (`behavior.auto_reload`) | Watch the current folder and show new and rewritten images at once | On |
| | Archive extraction folder (`behavior.archive_dir`) | Where opened ZIP and 7z files are extracted; `vw-archives` is made inside. Applies to archives opened after the change; open ones stay where they are until closed | Empty (the OS temporary folder) |
| Flags and tags | Tags (`labels.tags`) | Tags for images (comma-separated). The first 9 match keys 1 to 9 | None |
| | Next image after labeling (`labels.advance`) | In tagging mode, go to the next image after labeling | On |
| | Remove the location when exporting (`labels.export_strip_location`) | Initial value of "Remove the location" in image export | Off |
| Datasets | Dataset folders (`dataset.roots`) | Folders to look for datasets in (one per line) | None |
| | Dataset fields (`dataset.fields`) | Fields written besides name, memo and tags (comma-separated) | None |
| | Saved searches (`dataset.saved_searches`) | Searches kept with [Save searches](find.html#save-searches). Rename and delete them here | None |
| | Event: pause (`dataset.event_gap_hours`) | When grouping by events, a pause in shooting longer than this starts a new event | 18 hours |
| | Event: distance (`dataset.event_distance_km`) | When grouping by events, a photo taken farther than this from the previous one starts a new event. 0 for time only | 100 km |
| Send to apps | Send to apps (`send.apps`) | Apps listed under **Send to** in the right-click menu ([Open in another app](sort.html#open-in-another-app-send-to)) | The OS apps for JPEG and RAW |
| Map | Use the map (`map.enabled`) | Use the map window. While it is open, map pictures are downloaded from an outside server | Off |
| | Map picture URL (`map.tile_url`) | Where map pictures (tiles) come from. `{z}`, `{x}`, `{y}` are replaced by the tile numbers | OpenStreetMap |
| | Map credit (`map.attribution`), link (`map.attribution_url`) | Credit shown at the bottom right of the map | © OpenStreetMap contributors |
| Display | Monitors (`display.monitors`) | Drawing depth per monitor (8-bit, 10-bit, 12-bit or more) and dithering ([Show more than 8 bits](view.html#show-more-than-8-bits-10-bit-12-bit-or-more)) | 8-bit, dithering off |
| Keyboard shortcuts | Per action (`keys.*`) | See [Keyboard shortcuts](#keyboard-shortcuts) | |

### Change key bindings

The **Keyboard shortcuts** settings change the keys of every action.

- Actions are listed in groups such as "Moving between images and folders", "Zoom and play" and "Tagging". The group names are listed under **Keyboard shortcuts** in the contents on the left; click one to jump there. Typing a group name in the search box narrows to it too
- Press **+** and then a key to add it (Esc cancels). One action can have several keys
- Click an assigned key to remove it. ↺ restores the default
- A key used by another action cannot be assigned (remove it there first)
- Keys the OS often uses (Alt+F4 and the like) can be assigned, with an orange warning. If the OS takes them first, vw never receives them
- Ctrl+C / Ctrl+X / Ctrl+V are kept for the clipboard and cannot be assigned

### Settings file

| OS | Location |
| --- | --- |
| Windows | `%APPDATA%\vw\settings.toml` |
| Linux | `~/.config/vw/settings.toml` (under `XDG_CONFIG_HOME` if set) |

For example, you can write the following. Settings not written use their defaults.

```toml
[appearance]
language = "en"

[viewer]
layout = "auto"
auto_layout_max = "6x6"

[tree]
sort = "name_natural"
sort_descending = false

[labels]
tags = ["mountain", "sea", "flower", "city", "sky"]

[dataset]
roots = ["C:\\Users\\me\\Pictures", "D:\\Archive"]
fields = ["Source", "Photographer"]
```

- A setting with a broken value falls back to its default alone, and the log shows the reason in red
- When a change from the settings overwrites a broken file, the original is kept as `settings.toml.bak`

## Keyboard shortcuts

All can be changed in the [settings](#change-key-bindings). Menus show the current key on the right.

### View

| Key | Action |
| --- | --- |
| → / Space | Next image slot |
| ← | Previous image slot |
| Ctrl+→ / Ctrl+← | Next / previous slot (including folder slots) |
| PageDown / PageUp | Next / previous page |
| Enter | Maximize / restore the slot (on a folder slot, go to that folder) |
| Backspace / mouse back button | Up one folder |
| 0 | Fit to slot |
| Ctrl+0 | Actual size (100%) |
| + / = | Zoom in |
| - | Zoom out |
| P | Play / pause the slideshow |
| F5 | Fullscreen slideshow |
| V | Toggle the pixel probe |
| R | Toggle the ROI tool |
| Delete | Delete the selected cross or ROI |
| G | Show / hide the color gauge |
| D | Develop camera RAW from sensor data |
| J | Switch between the embedded preview of camera RAW and the developed image |
| A | Auto stretch for viewing / undo |
| O | Show / hide the clipping warning |
| K | Show / hide focus peaking |
| Shift+O | Show / hide colors outside sRGB |
| Ctrl+H | Open / close the analysis window (histogram, quality values) |
| H | Switch the left drag between hand and arrow |
| Z | Switch the left drag between zoom and arrow |
| Ctrl+C | Copy the image to the clipboard |

### Layout and slots

| Key | Action |
| --- | --- |
| Alt+0 to Alt+7 | Layout: Auto / 1x1 / 1x2 / 2x1 / 2x2 / 3x3 / 4x4 / Custom |
| Ctrl+N | New slot |
| Ctrl+W | Close the slot |
| Ctrl+Shift+W | Close all slots |

### Tagging

| Key | Action |
| --- | --- |
| T | Toggle tagging mode |
| ↑ / ↓ | Good / bad flag (in tagging mode only) |
| 1 to 9 | Tags 1 to 9 (in tagging mode only) |
| Ctrl+Shift+0 | Show all images |
| Ctrl+Shift+↑ | Show only good images |
| Ctrl+Shift+↓ | Show only bad images |
| Ctrl+Shift+U | Show only unflagged images |
| Ctrl+E | Export images |
| Ctrl+Shift+Backspace | Clear flags of the shown images |
| Shift+Alt+Backspace | Clear flags of the whole folder |

### Windows

| Key | Action |
| --- | --- |
| Ctrl+1 / 2 / 3 / 4 | Show / hide the folder tree / metadata / images / log |
| Ctrl+D | Open / close the Datasets screen |
| Ctrl+B | Open / close the subframes window |
| Ctrl+M | Show / hide the map (only when the map is enabled in settings) |
| Ctrl+Z | Undo positions placed or removed on the map, and flags cleared in bulk |
| Ctrl+, | Open / close the settings |
| Ctrl+Q | Quit |

### Mouse

| Action | Result |
| --- | --- |
| Wheel | Step through images or slots ([Step through images](view.html#step-through-images)) |
| Left drag | With the hand, move the image / with the arrow, carry the image to a folder |
| Right drag (up/down) / Ctrl+wheel / pinch | Zoom |
| Double-click | Maximize the slot / on a folder slot, load its images |
| Drag the header | Move the file (Ctrl to copy) |
| Right-click | Menus of images, the tree and the log |

## Supported formats in detail

| Format | Details |
| --- | --- |
| PNG | 1 to 16 bits, gray / RGB / alpha / palette |
| JPEG | Baseline / progressive, gray / color / CMYK. Follows the EXIF orientation and shows shooting information |
| WebP | Lossy / lossless, transparency, EXIF orientation and information. Animations show all frames in one slot and play |
| GIF | Transparency. Animations show all frames in one slot and play |
| BMP | 1 to 32 bits, palette / RLE / bit fields, bottom-up BMP too |
| Camera RAW | Nikon NEF / NRW, Sony ARW / SR2 / SRF, Fujifilm RAF (Canon CR2 / CR3 and DNG can be read too). The camera's embedded preview shows first, then the sensor data is developed in the background and the detailed image swapped in |
| ZIP / 7z / tar | Open the images inside like a folder. Password-protected ones too (ZIP: ZipCrypto and AES; 7z: AES). tar uncompressed, gzip, bzip2, xz and zstd (`.tar.gz` `.tgz` `.tar.bz2` `.tar.xz` `.tar.zst`...), comic book `.cbz` `.cb7` `.cbt`, and single compressed files `.gz` `.bz2` `.xz` `.zst` |
| TIFF | 1 to 16-bit integers and 32-bit floats, gray / RGB / alpha / palette / CMYK, black-and-white fax compression (CCITT G3 / G4), LZW / Deflate / PackBits / JPEG compression, tiles, BigTIFF. Multi-page files show all pages in one slot, stepped with the wheel (reduced-size pages are left out). Follows the orientation tag and shows camera and software information |
| FITS (astronomy images) | `.fits` `.fit` `.fts`, tile-compressed `.fz` (RICE / GZIP), whole-file gzip `.fits.gz`. 8 to 64-bit integers and floats, `BSCALE` / `BZERO`, missing values (`BLANK` and NaN). Images after an empty primary HDU (JWST and others) open too. Cubes and 3-plane images go in one slot, stepped with the wheel, and Bayer mosaics of color cameras (`BAYERPAT`) become color. Astronomy images are dark, so the first black and white come from the 0.5% to 99.5% pixels (the **Astronomy images (FITS)** settings can switch to zscale and others, and lift the range between with asinh, log or square-root curves). Up and down follow `ROWORDER`, or else the file order (can be flipped in settings). All header entries show in the metadata and can be used in dataset search. The sky area shows on the [sky chart](astro.html#see-what-you-captured-on-a-sky-chart). If dark, flat and bias masters are nearby, they are [applied before display](astro.html#apply-darks-and-flats) |

## Files vw makes

vw does not rewrite the original image files. Instead it makes the following files.

| File | Location | Contents |
| --- | --- | --- |
| `settings.toml` | Settings folder (Windows: `%APPDATA%\vw`, Linux: `~/.config/vw`) | Settings |
| `state.toml` | Settings folder | State at exit (window position and size, expanded tree folders, the current folder, extraction destination...). vw rewrites it automatically |
| `.vw-labels.json` | Folders with labeled images | Flags and tags of the images in that folder |
| `labels/` | Settings folder | Labels of images in folders that cannot be written to (read-only media, for example) |
| `vw-search-catalog.json` | Directly in the dataset folder | Search catalog |
| `map-tiles/` | Settings folder | Map pictures (tiles). If deleted, they are downloaded again the next time the map opens |

### .vw-labels.json (labels)

```json
{
  "version": 1,
  "files": {
    "IMG_0001.jpg": { "flag": "good", "tags": ["mountain"] },
    "IMG_0003.jpg": { "flag": "bad" },
    "IMG_0006.jpg": { "tags": ["flower"] },
    "IMG_0007.png": { "position": { "latitude": 43.0621, "longitude": 141.3544 } }
  }
}
```

- The name starts with a dot, so Explorer and Linux file managers treat it as a hidden file
- When vw moves or copies an image, its labels move to the file in the destination folder
- `position` is a location placed on the map (the image's EXIF is not rewritten)

### .vw-ai-tags.json (AI tags and face boxes)

```json
{
  "version": 1,
  "files": {
    "IMG_0001.jpg": {
      "model": "wd-vit-tagger-v3",
      "general": [{ "name": "outdoors", "score": 0.81 }],
      "rating": { "name": "general", "score": 0.97 },
      "faces": [{ "rect": [0.35, 0.12, 0.18, 0.22], "score": 0.94, "person": "Eileen", "similarity": 0.96 }]
    }
  }
}
```

- Face boxes are fractions of the image width and height. Face features are not stored here

### vw-search-catalog.json (search catalog)

A JSON file holding mainly the following.

| Key | Contents |
| --- | --- |
| `name` / `memo` / `tags` / `fields` | Name, memo, tags and fields of the dataset |
| `created` / `updated` | When it was made and last updated |
| `source` | For a dataset made by extracting, the original dataset |
| `meta` | List of "kind, key, value" triples. Each triple is stored once |
| `files` | Per file: path (relative to the dataset folder; inside a ZIP separated with `!`, like `data.zip!/day1/Z001.jpg`), size, modified time, and which `meta` triples it has |
| `skipped` | Files that could not be read, with the reason |

There is no need to edit it by hand. Name, memo, tags and fields can be changed in vw's dialogs.
