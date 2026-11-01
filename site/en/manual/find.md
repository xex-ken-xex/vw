---
title: "Find and keep images"
---

Register folders as datasets and find them by content or by where photos were taken.

![Datasets screen](../../images/en/dataset-search.png)

## Datasets and search catalogs

With many images, it is easy to lose track of what is in which folder.
In vw, register a coherent folder as a **dataset**, and you can later find it by its content.

- When you register a dataset, vw reads every image under the folder (including inside ZIP / 7z files)
  and builds a **search catalog** of format, size, shooting information (EXIF) and more
- The search catalog is one file, `vw-search-catalog.json`, directly in the dataset folder.
  Move or copy the folder and the catalog goes with it. Datasets on network drives or external disks can be searched as they are
- A dataset can carry a **name, memo and tags**, plus **fields** you define (for example source, photographer)
- Nothing is written into image files

## Setup: choose where to look and which fields

![Dataset settings](../../images/en/settings-datasets.png)

Set these two in the **Datasets** settings.

| Setting | Meaning | Example |
| --- | --- | --- |
| **Dataset folders** | Folders to look for datasets in, one per line. All folders below are searched too | `C:\Users\me\Pictures`<br>`D:\Archive` |
| **Dataset fields** | Fields written per dataset besides name, memo and tags. Comma-separated | `Source, Photographer` |

**Dataset fields** become columns in the Datasets screen and can be used as search conditions.
Adding fields later does not erase values already written.

## Register a dataset

![Register or edit a dataset](../../images/en/dataset-edit.png)

1. In the folder tree, right-click the folder → **Register as dataset...**
2. In the dialog, enter
   - **Name**: the name shown in the list (the folder name at first)
   - **Memo**: free text (several lines)
   - **Tags**: comma-separated. ▼ picks from tags used in other datasets
   - **Fields** (defined in settings): ▼ picks from values used in other datasets
3. Press **Register** to read the files under the folder and build the search catalog. The red line at the bottom of the window shows progress

- For password-protected ZIP / 7z files, vw asks for the password (not saved). **Skip** leaves that archive out and records it in the catalog as "unreadable"
- Files that cannot be read (broken images, for example) are recorded as "unreadable" too, and their count appears in the dataset details
- Folders that cannot be written to (read-only media, for example) cannot be registered
- Registered folders change color in the folder tree (teal)
- While reading, the progress line in the Datasets screen offers **Pause reading** / **Cancel**. If you close vw while paused, reading continues from there next time. Cancel discards what was read so far
- Paused folders, and folders where vw stopped midway, turn orange in the folder tree. Opening one asks whether to continue reading

## Edit and update

![Right-click menu in the tree](../../images/en/tree-menu.png)

Right-clicking a registered folder shows a different menu.

| Menu | Action |
| --- | --- |
| **Edit dataset...** | Change name, memo, tags and fields. **Save** rewrites only the catalog information; **Save and update** also rereads the files |
| **Update dataset** | Reread every file under the folder and rebuild the search catalog. Name and memo stay. The log shows how many files were added, removed and changed |

After adding or deleting images, **Update**. Until then, the search catalog keeps the old content.

## Find datasets

**Ctrl+D** or **View → Datasets** opens the Datasets window. **Ctrl+D** again closes it.

- Each time it opens, vw looks again for search catalogs under the **Dataset folders**. Catalogs removed or added outside vw show up after reopening
- The **Reload** button looks again at any time

### Simple search

Type words in the search box at the top to narrow to datasets whose **name, memo, tags, fields or metadata** contain them.

- Separate words with spaces to require all of them (for example `trip VW-100`)
- Case does not matter

### Advanced conditions

![Metadata key suggestions](../../images/en/dataset-key-candidates.png)

**+ Condition** under the search box adds a condition (the **Advanced** button closes and opens the conditions).
Each line is one condition, and only datasets meeting all of them remain.

A condition is made of three choices.

**1. Target**

| Target | Meaning |
| --- | --- |
| Name / Memo / Tags | The name, memo and tags written for the dataset |
| Fields (Source...) | Fields defined in settings |
| Metadata | Information of the files in the dataset. **Kind** and **key** choose which |

Metadata is held as "kind, key, value".

| Kind | Example contents |
| --- | --- |
| `image` | `format` (JPEG...), `width`, `height`, bit depth |
| `exif` | `Make`, `Model`, `PhotographicSensitivity` (ISO), `ExposureTime`, `FNumber`, `FocalLength`, `DateTimeOriginal` and more |
| `jpeg`, `png`... | Format-specific information. The metadata window group name in lower case |
| `ai tags` | AI tags. `person` (named people), `tag` (general tags), `character`, `rating`, `model` (the model that tagged). Confidence is not included |
| `labels` | Flags and tags added by hand (as of the last registration or update). `flag` (`good` / `bad`), `tag` (tag name) |
| `location` | Where photos were taken. `Latitude`, `Longitude` (decimal degrees), and `Country`, `Region`, `Town` when place names were looked up |
| `fits` | FITS header. `OBJECT`, `FILTER`, `EXPTIME` (exposure seconds), `DATE-OBS` and more |

▼ in the key box picks from the keys actually in the search catalogs. Once the key is chosen, ▼ in the value box picks from the actual values.

**Group** ▼ → **AI tags** adds "equals" conditions for `person`, `character` and `tag` at once, so you only pick values. **Labels** adds conditions for the flags and tags added by hand.

**2. Comparison**

| Comparison | Meaning | Example |
| --- | --- | --- |
| contains | The value contains the text | `Model` contains `VW` |
| equals | The value is the text | `format` equals `JPEG` |
| regex | The value matches a regular expression | `Model` matches `^VW-(100\|200)$` |
| range | The leading number of the value between a lower and upper bound (either may be empty) | `PhotographicSensitivity` 800 or more. A value like `"50 mm"` compares as 50 |
| exists | The key is present | `GPSLatitude` exists (some photos have a location) |

**3. Value**: the text or number to compare.

**Metadata conditions count files that meet them all in the same single file.**
For example, adding "`Model` is `VW-200`" and "ISO 800 or more"
keeps datasets with at least one **photo taken with a VW-200 at ISO 800 or more**
(a dataset with only VW-200 photos and ISO 800 photos from another camera is not kept).

### Result list

| Column | Contents |
| --- | --- |
| Name / Tags / Fields | What was written for the dataset |
| Hits / Files | Files meeting the metadata conditions / files in the dataset |
| Location | The dataset folder |

- Click a column heading to sort by it; again to reverse
- When a dataset holds another dataset (nested), sorting by location shows the inner one indented below the outer one
- The count appears at the top (for example "2 datasets / 3"). Unreadable search catalogs are counted too

### Details and actions

Click a row to see details on the right: memo, tags, fields, number of files, update time, where it was extracted from, and unreadable files (if any).

![List of files found](../../images/en/dataset-hits.png)

When metadata conditions narrowed the files (including words in a simple search found in image information rather than names or memos),
the details also list the **files found**, as paths relative to the dataset folder.

| Button | Action |
| --- | --- |
| **Show** (or double-click the row) | Go to the dataset folder and show the images. Files in the catalog that cannot be found are logged with a hint to update |
| **Show the n files found** | Show only the files found, one per slot (only when files were narrowed). Files inside ZIP / 7z are not shown; their count goes to the log (extract them to see them) |
| **Extract...** | Copy the dataset's files to another folder (below) |
| **Edit...** / **Update** | Same as the right-click in the tree |
| **Update all** (top) | Update every dataset in the list in turn |

### Group views (events, nights, astronomy, filters)

![FITS dataset grouped by night](../../images/en/dataset-groups.png)

**Group by** in the details splits the dataset's files (or the files found when searching) into groups. Each group shows the count, dates, place and total exposure, and **Show n** lays out only that group's files, one per slot.

| Group by | How |
| --- | --- |
| **Events** | Sorted by time taken, split where shooting paused long (setting **Event: pause**, 18 hours by default) and where a photo was taken far from the previous one (setting **Event: distance**, 100 km by default, 0 for time only). Gives one group per trip or event |
| **Nights** | Noon to noon next day, local time, is one night. A night of observing that crosses midnight becomes one group |
| **Target object / Optical filter / Camera model** | Per value of `fits:OBJECT`, `fits:FILTER`, `exif:Model` |
| **Metadata key** | Per value of any key in the catalog |

- Time taken is the EXIF `DateTimeOriginal` (local time at the place). For FITS, `DATE-LOC` (local time) is used first; otherwise universal time `DATE-OBS` is converted to local time with the site longitude `SITELONG` (or left as universal time without `SITELONG`)
- Total exposure is the sum of FITS `EXPTIME` (or `EXPOSURE`)
- The place is the town with the most photos in the group (only photos with looked-up place names)
- Files without a date or value go into the last **(no date)** / **(no value)** group

### Save searches

**Save search...** names the search words, advanced conditions and grouping and keeps them in the settings. Pick one from **Saved** ▼ to run that search again.

- Conditions are saved, not results, so each run gives the latest results, including datasets added or updated since
- Saving with the same name replaces it. Rename and delete in the **Datasets → Saved searches** setting

## Extract from datasets

![Extract dialog](../../images/en/dataset-extract.png)

**Extract...** copies the dataset's files to another folder,
to bring them from a shared folder to your PC or to hand them to someone.

- Files are copied keeping the folder structure. Files with the same name are not overwritten
- Files inside ZIP / 7z are extracted under a folder named like the archive, such as `data.zip`
- The default destination is under your home folder, and from the second time under the last destination
- The search catalog is not copied. Only with **Register the destination as a dataset** checked is a new catalog made at the destination (name and so on can be changed in the dialog).
  That dataset's details show the original dataset under **Extracted from**
- When extracting into another dataset, vw asks whether to update that outer dataset
- If stopped midway, the files copied so far stay at the destination
- When files were narrowed, the dialog opens with **only the files found** checked. Uncheck it to copy all the dataset's files

## Nested datasets

A dataset can be registered inside another (for example `trip2026/flowers` inside `trip2026`).

- The outer dataset includes the inner files. Searching the outer one finds the inner files too
- In the list, sorting by location shows the inner one indented below the outer one

## Tips for managing catalogs

- **Datasets are folders**. Register groups that make sense later, such as "per shooting day" or "per source", to find them easily
- **Use fields**: add `Source, Photographer, License` and so on to the **Dataset fields** setting to get list columns and search conditions
- **Update after changes**: after adding images, press **Update** or **Update all**
- **Large values**: values over 2 KB (embedded thumbnails, for example) are recorded as their length instead of the value

## See photo locations on a map

Turn on **Map → Use the map** in the settings, and **View → Map** (**Ctrl+M**) opens the map window. Photos with GPS in EXIF show as red dots where they were taken.

- At the top right, switch between **This folder** (photos of the open folder) and **All datasets** (photos in the search catalogs of registered datasets). Dataset photos get their location when the catalog is **updated**
- Drag to move, wheel to zoom, double-click to zoom in one step. East-west scrolls endlessly and stays connected across the date line
- Nearby dots merge into numbered circles. Click a circle to zoom to it; photos taken at the same spot are listed
- Click a dot for its thumbnail, file name and time taken. **Open** goes to the photo's folder and selects it
- In **This folder**, NetCDF files with latitudes and longitudes are also outlined in red ([Weather and ocean data](earth.html#latitude-longitude-time-and-level))
- Type a town, state or country name (English or Japanese) in **Search places** at the top left to get suggestions; pick one to go there. The first time, press **Place names...** and **Download** **Place names (GeoNames)** in the download screen (about 4 MB, into `models/geonames` in the settings folder)
- With place name data, nearby town names appear in the dot information and in the metadata window. When a dataset's search catalog is **updated**, metadata `location` gets country (`Country`), state (`Region`) and city (`Town`) in English and Japanese, usable in advanced search
- By default the map pictures are the standard [OpenStreetMap](https://www.openstreetmap.org/copyright) tiles. Only while the map is open, the visible part is downloaded and kept for 7 days or more in `map-tiles` in the settings folder. Without a connection, only the dots on a grid are shown
- Change the picture source with **Map picture URL**. Follow the terms of the provider you use and change the credit (**Map credit**) to match
- Drag an image slot header onto the map and drop it to set that as where the photo was taken (with or without a location in EXIF). The placed position wins over EXIF, and the metadata window shows **Position from: map** under location. EXIF is not rewritten; the position is kept in the folder's `.vw-labels.json`, and goes into the dataset search catalog on **Update**
- Remove a placed position with right-click on the dot or **Remove the position** in its information (back to the EXIF position, if any). **Ctrl+Z** (**Labels → Undo the last position change or flag clearing**) undoes placing and removing one by one. The undo button in the map window undoes only position changes
- Positions cannot be placed for images inside ZIP / 7z
