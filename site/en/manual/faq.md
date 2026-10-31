---
title: "FAQ and troubleshooting"
---

| Question / symptom | Answer / fix |
| --- | --- |
| The menus are in Japanese | Set **外観 → 言語** (Appearance → Language) in the settings to **English** ([Switch the display language](start.html#switch-the-display-language)) |
| File names or Japanese text show as □ | This does not normally happen on Windows. On Linux, run `sudo apt install fonts-noto-cjk` and restart (only Japanese-style CJK fonts are used; with only Chinese fonts installed, you get □) |
| ↑ ↓ or number keys do not add labels | Check that tagging mode is on (**T**). Images inside a ZIP cannot be labeled |
| Number keys do not add tags | Check that the **Tags** setting has tags. Numbers follow the order of the tags |
| The Datasets screen shows nothing | Put the parent folder of your datasets in the **Dataset folders** setting |
| Added images do not appear in search | **Update** the dataset. The search catalog keeps the old content until updated |
| A deleted search catalog is still listed | Reopen the Datasets screen or press **Reload** |
| Too many small slots | Lower the **Auto layout maximum** setting, or fix a layout such as **Alt+4** (2x2) and turn pages |
| I flagged images to delete them but do not know how | **Labels → Move bad images to trash...** sends all "bad" images to the trash |
| I flagged many images by mistake | **Labels → Clear flags of the shown images...** (Ctrl+Shift+Backspace) or **Clear flags of the whole folder...** (Shift+Alt+Backspace) clears them at once. Ctrl+Z brings them back if you cleared too many |
| Windows 11 "Smart App Control" blocks vw with no Run button | vw is not signed. Use the Store version or turn off Smart App Control ([Install](start.html#install)) |
| Chrome blocks the download | Open `chrome://downloads` and press **Keep** on the vw row |
| On Linux, vw does not start with `libxkbcommon-x11.so could not be loaded` | `sudo apt install libxkbcommon-x11-0` |
| vw closes right after starting / the screen is black | vw draws with OpenGL 2.0 or later. If OpenGL is not enough, it restarts automatically with wgpu (DirectX 12 on Windows, or Windows' built-in software rendering without a GPU). The `Renderer:` line at the top of the log shows which is used. If neither works, a window explains why; update the graphics driver. On Linux remote desktops and virtual machines, `LIBGL_ALWAYS_SOFTWARE=1 vw` may work |
| vw does not start on Windows (no window at all) | Double-click `check-vw.bat` in the `check` folder of the zip. It checks the Windows version, VC++ runtime, OpenGL and the start result, and writes `vw-check-report.txt`. User names and folder locations are masked, so you can paste it into an [issue](https://github.com/xex-ken-xex/vw/issues/new/choose) as is. See the README.md in the `check` folder |
| Large images look coarse | Images over 8192 pixels on a side are shown reduced because of GPU limits (the log says so). Images beyond the full-resolution memory limit are shown as thumbnails (raise **Full-resolution memory limit** in the settings) |
| 16-bit images look dark | 16-bit images without a recorded number of significant bits are shown using the whole 16-bit range. In the pixel pipeline window (**View → Pixel pipeline**), fit **Black and white** to the pixel range, or lift the shadows with a curve or stretch |
| Something is wrong | Look at the log window at the bottom; errors are in red. Right-click a log line to copy it. If it does not get fixed, tell us in an [issue](https://github.com/xex-ken-xex/vw/issues/new/choose) |
