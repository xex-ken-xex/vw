---
title: "Getting started"
---

From installing vw to viewing photos, tagging them and building a catalog.

## Download and install

Download the file for your OS from the [Releases](https://github.com/xex-ken-xex/vw/releases) page.

| File | For |
| --- | --- |
| `vw-windows-x64.zip` | Windows 10 / 11 (64-bit) |
| `vw-linux-x64.tar.gz` | x86_64 Linux such as Ubuntu 22.04 or later |

The Microsoft Store version (under review) does not include the [vwbatch](check.html#apply-a-pixel-pipeline-to-many-images-vwbatch) command or the `check` folder for start-up problems. Use the zip version if you need them.

There are two kinds of releases.

- **vX.Y.Z** (for example v0.3.0): a regular release. Use this one normally
- **Latest build (main)**: the newest development build, replaced automatically on every change. Use it to try new features early

Both come with `SHA256SUMS.txt` (the SHA-256 of each file). To check that a download is neither broken nor replaced,
compare it with that file, or run `gh attestation verify vw-windows-x64.zip --repo xex-ken-xex/vw` with the GitHub CLI
(see [SECURITY.md](https://github.com/xex-ken-xex/vw/blob/main/SECURITY.md#verifying-a-download)).

### Windows

#### Install

There is no installer. Just extract the zip.

1. Download `vw-windows-x64.zip`
   - If Chrome stops it with "This file isn't commonly downloaded", open `chrome://downloads` and press **Keep** on the vw row (vw is not signed yet, so this appears while downloads are few)
2. Right-click the zip, choose **Extract All**, and extract it anywhere (for example `C:\Tools\vw`)
3. Double-click `vw.exe` in the extracted folder

On the first start Windows may show "Windows protected your PC".
This warning appears for new unsigned apps. Press **More info** → **Run anyway**.

On a PC with Windows 11 **Smart App Control** turned on, you get "Smart App Control blocked an app that may be unsafe" with no **Run** button, because vw is not signed. You can either:

- **Use the Microsoft Store version**: it is not blocked (it is under review; the link will be added here when it is published)
- **Turn Smart App Control off**: **Windows Security** → **App & browser control** → **Smart App Control settings** → **Off**. On some Windows versions you cannot turn it back on without reinstalling Windows, and it weakens protection for other apps too. Decide at your own judgment and risk (ask your administrator on a work or school PC). Prefer the Store version if you can

#### Ways to open images

- **After starting vw**: click a folder in the folder tree on the left; its folders and images are laid out on the right
- **Drag and drop**: drop images or folders onto the `vw.exe` icon. Dropping onto a running vw window also moves to that folder
- **From the right-click menu**: right-click an image → **Open with** → **Choose another app** → **Choose an app on your PC** and select `vw.exe`. With "Always use this app", double-clicking opens vw
- **From the command line**:

  ```bat
  vw.exe C:\Users\me\Pictures
  vw.exe a.png b.png c.png
  vw.exe photos.zip
  ```

  A folder starts vw in that folder.
  Several images are laid out in separate slots. A ZIP / 7z is extracted and its contents are shown.

#### Start menu and desktop

Right-click `vw.exe` → **Show more options** → **Create shortcut**, and put the shortcut on the desktop
or in `%APPDATA%\Microsoft\Windows\Start Menu\Programs`.

#### Uninstall

Delete the extracted folder and the settings and state folder (`%APPDATA%\vw`). vw does not use the registry.

### Ubuntu

#### Install

```sh
tar xzf vw-linux-x64.tar.gz
cd vw
./install.sh
```

`install.sh` does the following (without administrator rights):

- puts `vw` at `~/.local/bin/vw`
- adds "vw" with its icon to the app list, and to "Open With Other Application" for PNG, JPEG, WebP, GIF, BMP, TIFF, FITS and camera RAW

You can also run `./vw` in the extracted folder without installing.

#### Open images

- **From the app list**: start "vw" and click a folder in the folder tree
- **From the file manager**: right-click an image → **Open With Other Application** → **vw**
- **From a terminal**:

  ```sh
  vw ~/Pictures
  vw a.png b.png c.png
  vw photos.zip
  ```

#### Required libraries

Desktop Ubuntu needs nothing more.
If vw does not start on a server or minimal install, install these:

```sh
sudo apt install libxkbcommon-x11-0 libgl1 libegl1 libxcursor1 libxrandr2 libxi6 fonts-noto-cjk
```

`fonts-noto-cjk` is used to show Japanese file names and the Japanese UI.

#### Uninstall

```sh
./install.sh --uninstall
```

## The first five minutes

Here is a quick walk through a first session.

### Switch the display language

On the first start vw uses Japanese if the OS display language is Japanese, and English otherwise. To change it later:

1. Open the settings with the gear button at the bottom left (or Ctrl+,)
2. Set **Appearance** → **Language**. It switches immediately
3. Go back with **← Back** at the top left

This guide uses the names shown in the English UI.

### View photos

1. In the folder tree on the left, press ▶ next to **Home** and click a photo folder (for example `Pictures`)
2. The image window on the right shows the folder's subfolders and images one by one. Each frame is called a **slot**
3. Click an image slot; the metadata window on the right shows the shooting information (camera, ISO, shutter speed and so on)
4. Double-click a slot to show it across the whole image window. Double-click again to go back
5. Use → / ← to move to the next / previous image slot

### Add tags

1. In the settings, enter the tags you want in **Flags and tags** → **Tags**, separated by commas (for example `mountain, sea, flower`)
2. Back in the image window, press **T** for tagging mode. A tag bar appears under the image window
3. Select an image and press **↑** for a "good" flag or **↓** for a "bad" flag; vw moves on to the next image
4. Press **1** for the first tag (mountain), **2** for the second (sea)
5. Press the funnel button in the left column to show only the "good" images

### Build a catalog

1. In the settings, enter the parent folder of your photos (for example `C:\Users\me\Pictures`) in **Datasets** → **Dataset folders**
2. In the folder tree, right-click a folder that belongs together (for example `Trip2026`) → **Register as dataset...**
3. Enter a name and a memo and press **Register**; vw reads the images and builds a search catalog
4. Press **Ctrl+D** to open the datasets screen. Your datasets are listed and can be searched by name or shooting information

## The main window

![Parts of the window](../../images/screenshot.png)

vw divides its main window into four windows.

| Place | Contents |
| --- | --- |
| Menu bar | **File**, **View**, **Labels**, **AI tags**, **Image**, **Settings**, **Help**. Each item shows its key on the right. Once one menu is open, hovering over the next name switches to it |
| Left column (tools) | From the top: select (arrow) / hand / zoom / pixel probe / ROI / measure (greyed out, not ready yet), tagging mode, filter, add slot. The settings button is at the bottom |
| Folders (left) | A tree of folders and ZIPs. Clicking a folder moves there and lays out its contents in the image window. Start from Home, each drive on Windows, or `/` on Linux |
| Images (center) | The frames (slots) that show images |
| Metadata (right) | File information and in-image information (EXIF and so on) of the selected slot's image |
| Log (bottom) | Records such as folders opened and jobs finished; errors are red. Per-image records go to the log file only. With **Terminal** at the right of the heading, the log looks like a dark terminal: finished jobs (ZIP extraction, copies, datasets, sending, AI and so on) pile up as green "Extracted" or "Copied" lines, running jobs show one line each with a name, a progress bar and counts ("(2/5)"), and the last line keeps a running summary of loading (images, average time, failures, current file) |

- Show or hide each window from the **View** menu or with Ctrl+1 to 4. Hide the others to see images larger
- Drag the borders between windows to resize them. The main window position and size and the window layout come back on the next start
- Drag a window heading (Folders, Metadata, Log) to see docking guides. Drop on a guide to dock the window above, below, left or right of another window, or at an edge of the main window. Docking below a window splits that column in two
- Drag a heading outside the main window to make it a separate window, which can go on another monitor. Separate windows always stay in front of the main window. Hold Ctrl while dragging to ignore the guides; press Esc while dragging to cancel
- A separate window that holds only one window has no heading row, and its name is shown in the OS title bar (windows with buttons keep a row with just the buttons). Drag the title bar onto a guide in the main window to dock it back (not on Linux Wayland)
- To put a separate window back, drag its heading onto a guide in the main window, use **Dock in main window** from the heading's right-click menu or **View** → **Window layout**, or Ctrl+double-click the heading. Closing a separate window hides it; showing it again from the View menu puts it in the same place
- The image window has no heading, so use **View** → **Window layout** to float it or dock it back. **Reset layout** restores the first layout
- The pixel pipeline, analysis, zoom map, map, sky chart, relationship chart and datasets are windows too. They open as separate windows; drag the title bar to dock them in the main window (on Wayland, use **View** → **Window layout**). They reopen where they were closed. Except for the datasets and image comparison windows, a window left open when vw quits opens in the same place on the next start
- While a ZIP is extracted, a dataset is read or extracted, or images are exported, a thin line appears at the very bottom of the window. A red line grows over the grey one to show progress
