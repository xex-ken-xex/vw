---
title: "Sort photos"
---

Flag and tag with one key, keep the good photos and clear away the rest.

![Photos in a 3 x 3 layout in tagging mode, with flags and tags](../../images/tagging-grid.png)

## Flags and tags

vw can put two kinds of marks on images (together called **labels**).

| Kind | What it is | Keys |
| --- | --- | --- |
| **Flag** | Either "good" or "bad". Used to sort photos (keep / discard) | **↑** (good) / **↓** (bad) |
| **Tag** | Names you choose (for example mountain, sea, flower). Any number per image | **1** to **9** |

Labels are not written into image files. They are saved in a small file `.vw-labels.json` in each folder
([file contents](reference.html#vw-labelsjson-labels)). When you move or copy a folder, its labels go with it.

## Setup: choose your tags

![Tag settings](../../images/settings-tags.png)

Enter the tags you use, separated by commas, in the **Flags and tags** → **Tags** setting.

```
mountain, sea, flower, city, sky
```

- The first nine tags match the number keys **1** to **9** (from the tenth on, click them in the tagging bar)
- Two tags cannot have the same name
- Renaming a tag does not rename labels already given

## Tagging mode

![The tagging bar at the bottom](../../images/tagging.png)

Press **T**, or the tag button in the left column, to enter **tagging mode**. Press again to leave it.

In tagging mode the **tagging bar** appears below the image area, and the flag and tags of the selected image light up in color.
You can also click the buttons in the bar to add or remove them.

| Key | Action |
| --- | --- |
| **↑** | Flag as "good". Press again to remove |
| **↓** | Flag as "bad". Press again to remove |
| **Shift+↑** | In a group of similar images (a burst, for example), flag this one "good" and the others "bad" ([Align and compare similar images](#align-and-compare-similar-images)) |
| **1** to **9** | Add the 1st to 9th tag. Press again to remove |
| **→** / **←** | Next / previous image slot |

- After adding a flag or tag, vw moves to the next image, so you can sort just by pressing keys (turn off with the **Next image after labeling** setting)
- ↑ ↓ (and Shift+↑) and the number keys work only in tagging mode, so you will not label by accident otherwise
- Images inside a ZIP cannot be labeled

**A suggested way to sort**

1. Open the folder to sort and press **Alt+1** (1 x 1) to show one large image at a time
2. Press **T** to enter tagging mode
3. Look at each image and press **↑** to keep or **↓** to discard. vw moves to the next image
4. Add tags with the number keys if needed

## Filter

![Filtered to "good" only](../../images/filter-good.png)

Show only the images with given labels.

| How | Result |
| --- | --- |
| Funnel button in the left column | Each press cycles all → good → bad → unflagged → each tag → all. Its color shows the current filter |
| **Labels** menu | **Show all images** / **Show only good images** / **Show only bad images** / **Show only unflagged images** / **Show only tag** |
| **Ctrl+Shift+0** / **Ctrl+Shift+↑** / **Ctrl+Shift+↓** / **Ctrl+Shift+U** | All / good / bad / unflagged |

- The filter applies to the slots in the image area, stepping, folder loading and the slideshow
- While filtering, the heading of the folder window shows the current filter (for example "Filter: good")

## Export

![Export dialog](../../images/export-dialog.png)

**Labels → Export images...** (**Ctrl+E**) exports the filtered images of the current folder.

| Field | Meaning |
| --- | --- |
| Folder | The folder (the current folder). **Include subfolders** adds the folders below |
| Images shown | The current filter (for example "Filter: good") |
| Export as | **Copy files to a folder** / **CSV list** / **JSONL list** |
| Destination folder | Where to copy. Turn off **Keep folder structure** to put all files flat in one folder (same names get numbers) |
| Output file | The file for a list |

The **CSV list** has three columns, `path, flag, tags`. Tags are separated by `;`.

```csv
/home/me/Pictures/trip/IMG_0001.jpg,good,mountain
/home/me/Pictures/trip/IMG_0002.jpg,good,mountain;sky
/home/me/Pictures/trip/IMG_0003.jpg,bad,
```

The **JSONL list** has one JSON object per image per line. Without a flag, it is `null`.

```json
{"path":"/home/me/Pictures/trip/IMG_0001.jpg","flag":"good","tags":["mountain"]}
{"path":"/home/me/Pictures/trip/IMG_0006.jpg","flag":null,"tags":["flower"]}
```

- The original images are not moved (only copied). Files with the same name at the destination are not overwritten
- When copying with subfolders, images inside ZIP / 7z files on the way are written under a folder named like the archive, such as `data.zip`.
  For password-protected ones, open them in the tree and export from inside. While filtering, images inside a ZIP have no labels and are left out
- When exporting inside a ZIP, the default destination is next to the ZIP
- With **Remove the location (GPS and place names)** checked, the copied files lose the EXIF GPS, the XMP location and place names (city, state, country...),
  and the FITS observing site (`SITELAT`, `SITELONG`...). The values are blanked in place, so image quality and other metadata stay the same.
  This works for JPEG, PNG, WebP, TIFF, TIFF-style RAW (NEF, ARW, DNG...) and FITS (BMP and GIF have no location).
  Other formats (HEIC, CR3...) are not copied because vw cannot confirm the removal, and the log says so.
  The initial value comes from the **Remove the location when exporting** setting

**To build machine-learning training data**, filter by a tag and **Copy files to a folder**, and repeat:
you get one folder per tag such as `mountain/`, `sea/`, `flower/`. Training scripts can read the CSV / JSONL lists as they are.

## Clear many flags at once

If you flagged by mistake, you can clear flags in bulk instead of one by one. Both show the count and file names before clearing, and **Ctrl+Z** (**Labels → Undo the last position change or flag clearing**) brings the flags back.

| Menu (Labels) | Key | Flags cleared |
| --- | --- | --- |
| **Clear flags of the shown images...** | Ctrl+Shift+Backspace | Images of the current folder shown by the current filter. When showing only "bad", only "bad" flags are cleared |
| **Clear flags of the whole folder...** | Shift+Alt+Backspace | All images of the current folder (regardless of the filter) |

- Tags stay. Images in subfolders are not included
- Ctrl+Z shares its history with location changes made on the map, undoing the newest first

## Delete bad images at once

**Labels → Move bad images to trash...** sends the images of the current folder flagged "bad" to the OS trash.

- vw shows the count and file names first
- Images in subfolders are not included
- They only go to the trash, so you can restore them from there if needed

## Align and compare similar images

Line up images of the same scene (RAW and JPEG, retakes, other lenses or settings) so they overlap, and measure where and how much they differ.

1. Show the images in slots, right-click the reference image → **Align and compare...**
2. Check the images to compare and press **Compare**. The others are moved, rotated and scaled to overlap the reference (different sizes and orientations are handled)
3. Compare them in the **Compare images** window
   - **Side by side**: all images next to each other; zoom and pan move together
   - **Blink**: swap the reference and one image with Space
   - **Swipe**: drag the dividing line; reference on the left, image on the right
   - **Difference**: the size of differences in color (black → red → yellow → white). **Overlay the difference** lays it semi-transparent over the other views
   - Cyan boxes mark the places with the largest differences (5 places)

The right panel shows shift, rotation and scale, PSNR, SSIM, color difference (ΔE2000), brightness shift, ratios of sharpness, noise and vignetting, and brightness histograms. Values use the colors shown in the slot (after the pixel pipeline), measured inside the rectangle where the images overlap.

- **Match brightness** (on by default): before measuring, brightness is matched to the reference so tone differences such as RAW development vs. JPEG do not count as differences. Brightness shift, vignetting and histograms are values before matching
- Images that probably did not align (low correlation, small overlap) get ⚠
- **Copy view** copies the current view as an image, **Copy values** a table to paste into a spreadsheet, and **Write report...** writes an HTML with images and a CSV of the values

Among the images in slots, similar ones get a chain icon and a number in the header (same number, same group). Grouped together are files with the same name (without extension) in the same folder, shots in a row from the same camera (time taken within the **Burst interval** setting, 2 seconds by default), and images that look very alike. Click the icon to compare the group with that image as the reference at once. Long runs of similar images without burst records (frames of a video, for example) are not grouped by look alone.

In each group, the one image suggested to keep gets a star on its icon. vw picks the sharpest image, and among equally sharp ones, the one with more pixels. Hover the icon to see the image's rank in the group.

![Keeping one shot of a burst](../../images/similar-burst-keep.png)

To keep the chosen image and drop the others, select it in tagging mode (**T**) and press **Shift+↑**, or right-click the icon and choose **Keep this one, flag the rest bad**. The chosen image is flagged "good" and the rest of the group "bad", so you can later send only the bad ones to the trash.

If the automatic alignment is off, align by hand with **Fix alignment**. The image lies semi-transparent over the reference; drag to move and turn the wheel to rotate (Shift for 10 times, Ctrl+wheel to scale). **Done** measures again, and the fixed alignment is kept in the folder's label file for later comparisons. **Back to automatic** removes it. Images inside a ZIP can be fixed, but the fix is not kept.

## Move and copy files

| Action | Result |
| --- | --- |
| Drag a slot header onto a folder in the tree or a folder slot | **Move** the image to that folder (with confirmation). Flags and tags move with it. With the arrow tool, dragging the image itself does the same |
| The same with **Ctrl** held | **Copy** (no confirmation). Flags and tags are copied too |

- If a file with the same name exists at the destination, nothing is done (the log says so)
- Nothing can be moved into a ZIP (it is read only)

## Open in another app (Send to)

Right-click an image or a folder in the tree → **Send to** to open it in another app (a raw developer, an editor and so on).

- **Open with the default app**: open with the app the OS associates with that file type
- Below it are the apps listed in the **Send to apps** setting. At first these are the apps the OS uses for JPEG and camera RAW
- For each app, the settings hold a name, program, arguments, what it accepts (files / folders / both) and extensions (a regular expression matched against the whole extension, case-insensitive, such as `nef|arw|cr[23]`; empty means any)
- The arguments can contain the following. Without any of them, the files are added at the end

| Placeholder | Meaning |
| --- | --- |
| `{path}` | One file (or folder). With several, the app starts once per file |
| `{paths}` | All of them, starting the app only once |
| `{dir}` | The folder holding the file |
| `{name}` | The file name |

- Wrap an argument in `"` to include spaces. No shell is used, so paths with `&` and the like pass as they are
- If the app would start 6 or more times, vw asks first
- For images inside a ZIP, the file in the temporary extraction folder is passed. It is deleted when vw exits
- Assign keys to **Send to app 1** to **9** in the **Keyboard shortcuts** setting to send the shown image to the 1st to 9th app with one key

## AI tags and faces

AI models can tag what an image shows and recognize the people in it. The models run on the CPU,
and images are never sent out.

1. In **AI tags → AI models...**, **Download** all three models (they go into `models` in the settings folder).
   Until all are present, AI tagging and face finding are disabled
   - ONNX Runtime (about 60 MB, MIT): needed for both features
   - WD tagger v3 (about 380 MB, Apache-2.0): for AI tags
   - YuNet + SFace (about 39 MB, YuNet MIT, SFace Apache-2.0): for faces
   - Each is a fixed version from its publisher, checked against SHA-256 and other hashes built into vw. Files that do not match are not used
2. **AI tags**: right-click an image → **Add AI tags**. For a whole folder, **AI tags → Add AI tags to this folder**
   or right-click it in the folder tree. Tags appear with their confidence under "AI tags" in the metadata window.
   They are saved in `.vw-ai-tags.json`, apart from the tags you add by hand. The log shows one line at the start and one at the end,
   and progress shows in the line at the bottom of the main window and the bottom line of the terminal-style log
3. **Faces**: right-click an image → **Find faces**. Found faces get a box. Right-click a box →
   **Register as a new person...** and give a name; from then on the same person's face gets that name (green box; unknown faces get a yellow box.
   Colors can be changed in the **AI tags** settings)
4. **AI tags → People...** lists the registered people. Filter by name, sort by first registration or by the number of images,
   and switch between large, medium and small. Select a person to rename or delete them, or to set the face shown for them (right-click a face).
   After each registration, rename or deletion, earlier found faces are matched again.
   The **Unregistered faces** tab groups unnamed faces by likeness. Type a name for a group and
   press **Register** to register all those faces as that person. If someone else is mixed in, right-click the face →
   **Not in this group**; passers-by and the like can be removed from the list with **Hide**
5. **AI tags → Relationship charts...** makes a chart per work or project, where you place people and connect them. Right-click a person →
   **Draw a line from here** to draw a line, and select a line to name the relationship (parent and child, colleagues...).
   People who appear in the same images get a dotted line automatically, thicker the more images they share.
   Selected people and the lines between them can be copied to another chart with Ctrl+C / Ctrl+V.
   Choosing a chart in the box at the top of the people list shows only the people in that chart
6. AI tags and face records can be removed with right-click on an image → **Remove AI tags**, or **AI tags → Remove AI tags from this folder...** (registered people stay)
7. To remove only unwanted tags, right-click the tag in the metadata window (from this image / from all images in the folder / never add).
   **AI tags → Tidy AI tags in this folder...** lists the tags of the open folder with counts, and removes several at once.
   Tags set to "never add" go into **Settings → AI tags → Excluded tags** and are not added again
8. In folders registered as datasets, AI tags and people's names also go into the search catalog, so you can search them in the Datasets screen
   (metadata kind `ai tags` in [Advanced conditions](find.html#advanced-conditions)). After adding AI tags or names, **Update** the dataset
9. AI tags and faces record the name and version of the model that made them, shown as **AI model** and **Face model** in the metadata window.
   When a newer vw changes a model, they show "(older than the current model)", and **AI tags → Redo AI tags and faces made by older models**
   redoes only the old ones in the open folder. In the search catalog, filter with `model version` and `face model version` under `ai tags`

- Face features (the numbers compared for likeness) can identify people, so they are not written into image folders,
  only into the settings folder on this PC (`people.json` and `faces`)
- Made for real faces. Illustrated characters are recognized by the character tags of AI tags
- Thresholds can be changed in **Settings → AI tags**
