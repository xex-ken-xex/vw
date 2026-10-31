---
title: "View images"
---

Lay out the images of a folder and compare them quickly.

![Photos of a folder in a 3 x 3 layout](../../images/en/screenshot.png)

## Open a folder

The folder tree works like a file explorer.

| Action | Result |
| --- | --- |
| Click a folder in the tree | Go to that folder and show its subfolders, ZIPs and images one per slot |
| Click ▶ in the tree | Only expand or collapse (does not go to the folder). Folders without subfolders show no ▶ |
| **Backspace** / **File → Up one folder** | Go up one folder |
| Double-click a folder slot | Load the images of that folder into that one slot (see [Folder slots](#folder-slots)) |
| **Enter** on a folder slot, or the arrow at the right of its header | Go to that folder (the arrow appears when the mouse is over the header) |
| Drop files or folders from a file manager | Go to that folder (for a file, select that image) |
| Right-click a folder in the tree → **Open folder** | Open the folder in Explorer (on Linux, the default file manager) |
| Right-click a folder in the tree → **Copy** | Copy the full path of the folder to the clipboard |

- The default order is **newest modified date first**. The **Sort order** setting can switch to file names (in name order, `img2` comes before `img10`). **Descending** is on by default, so turn it off to sort names from A to Z
- Hidden folders starting with a dot and the Windows Recycle Bin (`$RECYCLE.BIN`) are not shown
- The last folder and the folders expanded in the tree come back at the next start

## Reading a slot

![Slots](../../images/en/folder-slots.png)

The strip at the top of a slot is called the **header**.

- The header shows the file name on the left, and on the right the zoom, two buttons (⛶ = fit to slot, 1:1 = actual size) and ×
- The selected slot has a brighter border
- Folder slots show a folder icon, ZIP slots a ZIP icon
- When a slot holds several images (a loaded folder, an animation), "current / total" appears at the top right of the image
- A flagged image has a green (good) or red (bad) line under its header, and the header shows its flag and tags
- While a camera RAW is being developed in the background, the header shows a film icon

## Folder slots

Double-click a folder slot to load the images of that folder **into one slot**.
The mouse wheel steps through the images one by one, which is handy for comparing folder by folder.

- If there are subfolders, vw asks whether to load their images too
- For very many images, vw can ask whether to continue every **Chunk size** images (500 by default; setting **Confirm large loads**)
- How a click in the tree loads a folder (one slot per image / one slot per folder) is chosen with the **Load folders into** setting. **Alt**+click loads the other way

## Move and zoom

What a left drag on the image does depends on the tool column. **Hand** (the default) moves the image, the **arrow** carries the image to folders or the map, and **Zoom** zooms in and out. **H** switches to the hand and **Z** to zoom (press again for the arrow). The chosen tool is kept for the next start. To always start with one tool, set **Tool at startup** (`viewer.start_tool`). Zooming with a right drag works with every tool.

| Action | Result |
| --- | --- |
| Left drag | Move the image (with the hand tool; with the arrow it drags the image to a folder) |
| Right drag (up/down) | Zoom around the point you pressed |
| Click / Alt+click | With the zoom tool, zoom in / out one step around the click (a left drag up/down also zooms) |
| Ctrl+wheel / touchpad pinch | Zoom around the mouse |
| **+** / **-** | Zoom in / out |
| **0** / ⛶ in the header | Fit to slot |
| **Ctrl+0** / 1:1 in the header | Actual size (100%) |
| **Home** | Reset the display of the selected slot to when it was loaded (zoom and position, black and white, color map, added nodes and check views; probe crosses and ROIs stay) |
| Right-click the image | **Fit image to slot**, **Copy**, **Develop RAW from sensor data** (for camera RAW) |
| **Ctrl+C** | Copy the image of the selected slot to the clipboard (paste it into other apps) |

- Images first fit the slot (small images are enlarged to fit)
- When you step through the images of a loaded folder, the zoom and position stay the same, so you can compare the same spot over many images (turn off with the **Keep zoom when stepping** setting)

## Step through images

| Action | Result |
| --- | --- |
| **→** / **Space**, **←** | Select the next / previous **image slot**. Folder slots are skipped and the selection wraps around. The contents of slots do not change |
| **Ctrl+→** / **Ctrl+←** | Select the next / previous slot (including folder slots) |
| Mouse wheel | Over a loaded folder slot, its next / previous image. With one image per slot, the next / previous slot. With one image shown (1x1 or maximized), the next / previous image |

## Maximize and slideshow

![Maximized slot and the play bar](../../images/en/maximize-play.png)

| Action | Result |
| --- | --- |
| Double-click a slot / **Enter** | Maximize the slot to fill the image area. Again to restore |
| **P** | Play / pause a slideshow. Maximize the slot and choose the speed in the play bar below |

When a slot is maximized, the **play bar** appears below the image area. GIF and WebP animations start playing without P when opened as a single image (1x1 or maximized).

- **▶**: play / pause
- **File speed**: play files that carry a speed, such as animations (GIF / WebP), at that speed. GIF and WebP use the length of each frame. Changing the FPS turns this off
- **FPS box and 1, 5, 10, 15, 30, 60**: frames per second. Drag or type in the box; the value is kept for next time
- **Loop / Stop at end / Ping-pong**: what happens at the last image
- **Dropped**: frames skipped because the display could not keep up (over the last 5 seconds)

## Fullscreen slideshow

![Slideshow setup](../../images/en/view-slideshow-setup.png)

**F5** (or **View → Fullscreen slideshow...**) opens the setup screen. Choose a start folder and press **Start** to show images one at a time on the whole screen. Images are not loaded into slots, so even folders with tens of thousands of images start at once.

- **Mode**: show the folders with images under the start folder in order, and the images in each in order / show the newest file under the start folder and switch when a new file arrives (for tethered shooting)
- **Sort order**: order and descending for folders and files
- **Interval**: seconds before moving to the next image (0 = no automatic step), and whether to go back to the first at the end
- **Path**: whether to show the file path at the bottom right, and its color, opacity and size

In fullscreen, **→ / ←** go to the next / previous image, **↓ / ↑** to the next / previous folder, **Space** pauses and resumes, and **Esc** ends.

## Layouts and pages

How slots are arranged in the image area is the **layout**. Choose it with **View → Layout** or **Alt+number**.

| Key | Layout | Arrangement |
| --- | --- | --- |
| **Alt+0** | Auto (default) | A grid close to square, chosen for the number of slots each time |
| **Alt+1** | 1 x 1 | One large image |
| **Alt+2** / **Alt+3** | 1 x 2 / 2 x 1 | Two images, stacked / side by side |
| **Alt+4** / **Alt+5** / **Alt+6** | 2 x 2 / 3 x 3 / 4 x 4 | 4 / 9 / 16 images |
| **Alt+7** | Custom | Columns x rows of the **Custom layout** setting (default 6 x 6, 1 to 8 each) |

![Page bar](../../images/en/pages.png)

- **Auto** puts every slot on one page up to the **Auto layout maximum** (default 6 x 6 = 36 slots). With more slots, it splits them into pages at the maximum grid so slots do not get too small
- With a fixed layout such as 2 x 2, slots that do not fit go to the next page. Empty cells are drawn as empty slots. Click one to select it; the next image opened from the tree goes there
- With several pages, a bar like **"48 slots < 1 / 2 >"** appears at the bottom right of the image area: the number of slots, the current page / pages, and buttons for the previous and next page
- **PageDown** / **PageUp** also go to the next / previous page
- Turn the wheel over the bar, or **Shift+wheel** over the slots, to scroll the slots. The amount is set by **Wheel scroll amount**: one slot, half a page or one page (default). Hover the numbers in the bar to see the current amount
- **Drag the page number left or right** in the bar to jump to a distant page

## Add and close slots

| Action | Result |
| --- | --- |
| **Ctrl+N** / **File → New slot** / ⊞ in the left column | Add an empty slot |
| **Ctrl+W** / × at the top right of a slot | Close the slot |
| **Ctrl+Shift+W** / **File → Close all slots** | Close all slots |

## Open ZIP, 7z and tar

ZIP, 7z and tar files open like folders. These kinds are supported:

| Kind | Extensions |
| --- | --- |
| ZIP, 7z | `.zip` `.7z` (password-protected too) |
| tar | `.tar`, and compressed `.tar.gz` `.tgz` `.tar.bz2` `.tbz2` `.tar.xz` `.txz` `.tar.zst` `.tzst` |
| Comic book archives | `.cbz` (a ZIP), `.cb7` (7z), `.cbt` (tar) |
| A single compressed file | `.gz` `.bz2` `.xz` `.zst`, opened as a folder holding that one file. `.fits.gz` opens directly as an image |

1. Click the ZIP in the tree or double-click it in a slot
2. vw extracts it to a temporary folder (setting **Archive extraction folder**), shows it as another root at the top of the tree, and goes into it
3. For a password-protected archive, vw asks for the password (it is not remembered)
4. When you are done, right-click the ZIP root in the tree → **Close ZIP**. The extracted files are deleted (also when vw exits)

- Images inside are read only. You cannot flag or tag them
- **Open folder** on a folder inside a ZIP opens the folder where the ZIP is
- Shift_JIS names in old ZIP and tar files made on Japanese Windows are shown correctly
- RAR, LZH, CAB, ISO and split archives (`.7z.001` etc.) cannot be opened. If a folder has one, the log shows it in yellow with its full path
- Links inside tar files (symbolic and hard links) are not extracted

## Show more than 8 bits (10-bit, 12-bit or more)

On a monitor that can show deep gradation, 16-bit images (PNG, TIFF, FITS, developed RAW...) can be drawn with 10-bit or 12-bit or more gradation.
Bands in smooth gradients (skies, faint parts of nebulae) become 4 times finer at 10 bits and 16 times finer at 12 bits.

- **Settings → Display → Monitors** lists the connected monitors. For each, choose the depth (**8-bit** / **10-bit** / **12-bit or more**) and **Dithering** (defaults: 8-bit, dithering off)
- **10-bit** draws on a 10-bit surface (Rgb10a2Unorm) and **12-bit or more** on a 16-bit floating-point surface (Rgba16Float). Windows composes the floating-point surface as is and sends it at the monitor's signal depth (12 bits, for example). Bright parts get about 12 bits per color, dark parts finer
- The depth takes effect from the next start. The surface used by this run is shown under the same setting. Where a 12-bit-or-more surface cannot be made, vw draws 10 bits, and if that fails, 8 bits
- **Dithering** mixes in a fine pattern when reducing to 8 bits on an 8-bit monitor so bands stand out less. It applies at once
- This applies when one image is shown large (1x1, maximized) and to the fullscreen slideshow. With many slots, vw draws 8 bits as before
- When the drawing changes, the log shows a line like `Display: monitor name - image drawn 10-bit` (`16-bit float` for 12 bits or more)

The monitor, cable, GPU and OS settings must all support the depth.

- **Help → System information** shows the OS, renderer, drawing surface, and each monitor's signal bits (Windows) or EDID bits (Linux). **Copy** it for a support request
- **Help → Display gradation test...** fills the chosen monitor with gray bands made inside vw (8, 10, 12 and 14-bit steps, smooth, dithered). No image file is needed. **← →** switch between dark, middle and bright, and **Esc** closes. When n bits reach the screen, the bands get finer down to the n-bit step and deeper bands look the same as the n-bit one. Each band also shows the expected look from vw's drawing depth and the monitor's signal bits. If a monitor set to 10-bit receives a deeper signal, vw suggests 12-bit or more
- If a deep surface cannot be made, vw restarts with 8 bits (OpenGL) as before and logs the reason
- Quality values (PSNR, SSIM...) are measured from the 16-bit display values without rounding to 8 bits (the scale stays 0 to 255)

## What vw does automatically

- **Watching the folder**: when images are added to the current folder, slots are added; when they are deleted, slots are closed.
  Slots showing the last image move to the new image (with one image shown, always to the new one).
  This lets you follow the newest shot while shooting to the PC.
  When the shown image is overwritten, it is reloaded keeping the zoom and position. Stop this with the **Auto reload** setting
- **Restoring the last state**: the window position and size, the expanded folders in the tree, and the current folder come back at the next start.
  If the monitor layout changed, the window appears at the OS default position (only the size on Wayland)

## Metadata window

- **File**: path, size, plugin used, pixels, color, sample (bits), size of the image used for display
- **Format groups** (JPEG, PNG...): information of that format
- **EXIF**: camera, lens, ISO, shutter speed, aperture, date taken and more
- Click a group heading to open or close it (the **Group heading click** setting can make it select only)
- Select a row and press **Ctrl+C**, or right-click → **Copy**, to copy the value
- **Location**: latitude, longitude and altitude (decimal degrees) of photos with GPS in EXIF. With place name data, the **place names** of a nearby town (country, prefecture, city) are shown too
- **Color profile**: description, version, color space, white point, tone curves and primaries of the embedded ICC profile (named for sRGB, Display P3, Adobe RGB (1998), ProPhoto RGB and BT.2020), and CICP (color numbers such as in the PNG cICP chunk). Read from JPEG, PNG, TIFF and WebP
- **XMP**: the IPTC **digital source type** (taken by a camera, made or edited by generative AI, and so on). Images involving generative AI also show **Generative AI: yes**. This goes into the dataset search catalog, so you can use it in advanced search

vw shows pixel values as sRGB without color conversion. For images with a wider gamut than sRGB, such as Display P3 or Adobe RGB, the metadata **Gamut** shows a note, and vivid colors look duller than they are. **Shift+O** (**Image → Show colors outside sRGB**) paints magenta the colors sRGB cannot show (the **Out of sRGB gamut** node under **Check** in the pixel pipeline). The gamut comes from the ICC profile or CICP of the image, and can be chosen again in the node, such as Display P3.
