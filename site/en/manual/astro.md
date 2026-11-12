---
title: "Astrophotography (FITS)"
---

Make faint images easy to see, then calibrate, select and finish them.

![A faint nebula FITS made visible with a color map and gauge](../../images/en/astro-nebula.png)

## See what you captured on a sky chart

![Areas captured by the FITS files of a folder on the sky chart](../../images/en/astro-skychart.png)

**View → Sky chart** opens the sky chart window. It outlines where on the sky each astronomy image (FITS) was taken, over stars (to magnitude 6) and constellation lines, borders and names. No network is used.

- The outline depends on what the header tells
  - **Solid yellow**: images with WCS in the header (`CTYPE` `CRVAL` `CRPIX` `CD`...). The exact area is known. TAN, SIN (including the slanted form and the older NCP), ARC, STG and ZEA projections and the all-sky CAR, AIT, MOL and SFL (including the older GLS) are read, with the optical distortion polynomials (`-SIP` and `TPV`). Coordinates may be right ascension and declination (`RA` `DEC`) or galactic (`GLON` `GLAT`)
  - **Dotted red**: images with the mount pointing (`RA` `DEC`, or `OBJCTRA` `OBJCTDEC`), focal length (`FOCALLEN`) and pixel size (`XPIXSZ` `YPIXSZ`). The area estimated from the field of view is drawn with north up (rotation unknown)
  - **Red cross**: images with only the mount pointing
  - **Yellow cross**: images wider than a hemisphere, such as all-sky maps or bands around the sky. No outline is drawn; the cross marks the centre
- When one file has two or more image HDUs with WCS, as from a mosaic camera, each detector is outlined thinly inside a thick outline around them all
- At the top right, switch between **This folder** and **All datasets** (FITS in the search catalogs)
- Drag to move, wheel to zoom, double-click to zoom 2x. Right ascension and declination under the cursor show at the bottom left
- Click an outline for its thumbnail, file name, `OBJECT`, `DATE-OBS` and center coordinates. **Open** goes to the image's folder, selects the image and loads it first. Where outlines overlap, such as a series of shots of the same target, pick from a list
- The outline of the active slot's image is drawn thicker
- Outline colors can be changed in the **Sky chart** settings. Turn off **Use the sky chart** to remove it from the menu

## See sky coordinates at the cross

For images with a WCS in the header, the cross of [pixel value check](check.html#measure-pixel-values-probe-roi-analysis) (**V**) and the cursor value at the bottom left add the right ascension and declination after the value and position, such as "α 05h35m16.46s  δ -05°23′22.8″". Cubes also add the plane label.

- The optical distortion polynomials (`-SIP`, common in plate solve results, and `TPV` in observatory data) are applied, so positions stay right at the edges of the image
- Choose between **Hours and degrees** and **Decimal degrees** with the **Sky coordinates at the cross** setting (`fits.sky_coordinates`)
- Images in galactic coordinates (such as all-sky maps) show the galactic longitude and latitude first, with the right ascension and declination in parentheses ("l 0.000000°  b +0.000000°  (α 17h45m37.20s  δ -28°56′10.2″)")
- **Ctrl+C** copies the right ascension and declination in decimal degrees (south declinations negative) after the value, separated by commas. Galactic images add the galactic longitude and latitude after them
- Images whose area is only estimated from the mount pointing and focal length (dotted red on the sky chart) get no coordinates, so a rough position is not shown as if it were exact for each pixel

## Switch extensions (HDUs)

JWST and HST images and mosaic camera images hold several image HDUs (extensions) in one file (SCI, ERR and DQ for JWST, one per detector for a mosaic camera). The first image with data (SCI for JWST and HST) is shown first.

- **N** / **Shift+N** switch to the next / previous HDU (the same keys as NetCDF variables). They can also be picked from right-click **HDU**, which lists their sizes and types
- An HDU is named by its number (the primary is 0) and `EXTNAME` (`1 SCI`, or `2 SCI,2` when names repeat)
- The chosen HDU is remembered per file until vw is closed
- Black and white are chosen again for each HDU. Black and white set by hand are not carried to the next HDU (errors and flags have different ranges from the image)
- Darks and flats are applied only to the image shown first
- The metadata shows the primary's cards, the cards of the HDU on show (**FITS HDU n**) and the list of HDUs to choose from (**FITS HDUs**)

## Apply darks and flats

When you open an astronomy light frame (FITS), vw looks for matching dark, flat and bias masters and applies them before coloring and choosing black and white. You see the image without amp glow, hot pixels, vignetting and dust shadows.

- Where it looks: the open folder and its parent, and neighboring folders such as `darks` `flats` `biases` `darkflats` `calibration` `masters`. Folders in the **Calibration library** setting are searched too
- The kind comes from `IMAGETYP` (`DARK`, `Dark Frame`, `Master Flat`, IRAF `zero`...), or else from the folder name. Masters are used (`IMAGETYP` `Master …`, `master` in the name, `NCOMBINE` 2 or more, or inside a `masters` folder)
- When there are only single calibration frames, frames taken under the same conditions (darks: exposure, temperature, gain, offset; flats: filter and night) are stacked into a master in the background (sigma-clipped mean, which drops cosmic rays and satellite trails). Built masters are kept in the `calibration-cache` folder next to the settings file and rebuilt when frames are added or changed. Lights are shown as they are until the master is ready, then calibrated. **Image → Clear built calibration masters** removes them
- With **Fill hot pixels** (off by default), pixels much brighter than their neighbours in the master dark are filled with the median of their neighbours (of the same color on a color camera). It can be switched per image from the right-click menu or the pixel pipeline
- Darks must match size, binning, `GAIN`, `OFFSET`, exposure (±0.5 s) and temperature (±2 °C); flats must have the same `FILTER`. Flats have the dark flat (or bias) subtracted, then are divided by their median
- Calibrated images get a mark in the slot header. **Apply dark and flat** in the slot's right-click menu switches to the uncalibrated image to compare (a key can be assigned too)
- The files used, and why calibration was not possible ("no 120 s dark", for example), appear under **Calibration** in the metadata. The log also says it once per folder
- How masters are chosen can be changed in the **Astronomy images (FITS)** settings (scale darks with other exposures, use only flats from the same night, equalize color flats per channel, constant bias, and more)

## Finish astrophotos (auto stretch, GHS, adding stars back)

![The pixel pipeline window with black and white, auto stretch and color map nodes](../../images/en/astro-pipeline.png)

Stretch unstretched (linear) astrophotos so they are easy to see, and finish stars and nebulae separately. All of these are **Astronomy** nodes in the pixel pipeline. Besides FITS, they work on 16-bit and 32-bit TIFF and PNG written by stacking software.

- **Auto stretch for viewing** (**A**): measures the sky background (median) and noise from the image, lifts the background to a set brightness (0.25 at first) and makes black the level 2.8 times the noise below the background (midtones transfer function, MTF). Press again to remove. At first R, G and B are stretched separately, so a sky tinted by light pollution looks gray. Values can be changed under **Auto stretch for viewing** in the **Pixel pipeline** settings
- **Hyperbolic stretch (GHS)**: five values shape the stretch curve: stretch amount, local intensity b, symmetry point SP, shadow protection LP and highlight protection HP. Set SP to the brightness of the sky background to stretch the faint nebula just above the background most, without overstretching bright stars. The node shows the curve. The formulas follow the description published by Cranfield and Shelley
- **Add stars back**: puts back the stars taken out by an earlier **Star removal** node into the starless image. Curve nodes in between (stretch, GHS, auto stretch, levels, tone) apply to the stars as well; noise reduction and sharpening apply only to the starless image. With **Star removal → Noise reduction → GHS → Add stars back**, you can stretch while calming only the grain of the background and nebula, without damaging the stars. Blend with **Add** (same brightness as without removal) or **Screen** (keeps stars on bright nebulae from blowing out, but fainter). **Amount** adjusts the star brightness, and **Show only the stars** shows what will be added back

Auto stretch and GHS stretch the values after black and white map them to 0 to 1. When you use the **Stretch** node, put them after it.

## Select subframes (evaluation table and blink)

![Evaluation table of a night's subframes. Images outside the thresholds are red, with the reason](../../images/en/astro-subframes.png)

Open the folder of a night's exposures (subframes) and open the subframes window with **Ctrl+B** or **View → Subframes**; vw evaluates each image of the folder and shows a table. It is for removing blurred, trailed or clouded frames before stacking.

- **Evaluation**: star width (FWHM, pixels), eccentricity (0 is round, near 1 is trailed), number of stars, background brightness and noise. Click a heading to sort. The top row is the folder median
- **Rejected images**: images whose FWHM is over 1.3 times the median, eccentricity over 0.7, fewer stars than half the median, or background brighter than the median by more than 10 times the noise are shown in red, with the reason under **Rejected because**. Thresholds can be changed under **Subframe rejection** in the **FITS** settings; the table follows at once
- **Flag rejected as bad**: flag all red images as bad at once, so filtering and export leave them out
- **Blink**: aligns all images on their stars to the reference (the image with the finest stars) and shows small versions in turn. Clouds, satellite and aircraft trails, and misaligned frames stand out. Frames turned 180° by a meridian flip are aligned too. ▷ plays, ‹ › step one by one. Turn off **Skip rejected** to include the red ones. The speed is the **Blink speed** setting (4 images per second at first)

Click a name in the table to select that image in the slots. Evaluation runs only while the window is open, and starts again when another folder is opened.

## Radio telescope images

Radio images (FITS) can have values crossing 0 and many planes (one per frequency). The **Astronomy images (FITS)** settings offer:

- Set **First black and white** to **Symmetric around zero** to put 0 in the middle. With the **Cool-warm (blue, white, red)** color map, positive and negative show in color
- **Same black and white for all planes** (on by default): one black and white from all planes of a spectral cube, so planes can be compared. Step planes with the wheel
- **Beam** (on by default): draws the BMAJ, BMIN and BPA (resolution) of the header as an ellipse at the bottom left of the image
- Pixels without a value (BLANK or NaN, outside the field, for example) are drawn in the **Color of missing pixels**. They keep that color even with a color map

### Spectra (data with one axis)

FITS files with only one axis (NAXIS=1, or every other axis of length 1) are drawn as a **line graph**. The horizontal axis follows CTYPE1 as **frequency, velocity or wavelength**, in units that fit the values such as GHz, km/s or mm. The vertical unit is BUNIT.

Axis values follow the FITS WCS standard (Paper III): channels spaced in frequency but written as wavelength or velocity (`WAVE-F2W`, `VOPT-F2W`, `FREQ-V2F` and the rest of the 12 forms), logarithmic axes (`-LOG`), grism axes (`-GRI`, `-GRA`), air wavelengths (`AWAV`), energy (`ENER`), wave number (`WAVN`), redshift (`ZOPT`), and the older `FELO-HEL` and `VELO-LSR`. Cube plane labels (frequency, velocity) use the same formulas.

- Hover over the graph to see the axis value, the value and the point number of the nearest point. With a rest frequency (RESTFRQ) or rest wavelength (RESTWAV), the line-of-sight velocity is shown too: the radio definition on a frequency axis, the optical definition on a wavelength axis. The velocity frame (SPECSYS, such as LSRK) is added in brackets
- The line breaks at points without a value (NaN)
- Turn off **Spectra as graphs** (`fits.spectrum_graph`, on by default) to show them as an image one pixel high

### Contours

Contour lines can be drawn over gray images (FITS, 16-bit PNG and so on; not color images). Toggle them per slot with the contours button in the tool column, the **Image** menu, or **Contours** in the slot's right-click menu (they are not kept across restarts). A key can be assigned under **Settings** → **Keys** (no key by default).

- The levels are set by **Contour levels** (`contours.levels`, default `-3, 3, 6, 12, 24, 48, 96`). When **Contour levels are** (`contours.mode`) is **Multiples of the noise** (the default), each level is the median of the shown plane plus that many times the noise (1.4826 × the median distance from the median). With **Pixel values**, each level is drawn as written
- Levels below the median (negative multiples of the noise) are drawn dashed, so negative bowls in radio images stand out
- Lines are traced from the original values, not the display image. On cubes they are traced again for each plane. Images with a long side over 2048 pixels are traced on a thinned grid
- The color is **Contour color** (`contours.color`, default `#00E5FF`) and the width **Contour line width** (`contours.width`, default 1)

### Spectrum at crosses

Place a cross with the [pixel probe](check.html) (**V**) on a FITS cube (two or more planes) and the **Spectrum at crosses** window opens, drawing the values of that point on every plane as a line (it can also be opened from the **View** menu).

- Each cross gets one line in the cross's own color. Move a cross and its line follows
- Crosses on a cube belong to the file, not the plane, so they stay in place as you step through planes
- The horizontal axis is the frequency, velocity or wavelength axis (chosen even when a polarization axis comes first); the vertical unit is BUNIT. A vertical line marks the shown plane
- Hover to see the axis value of that plane and the value of each cross. Click to move the slot to that plane
- Tile-compressed (`.fz`) cubes are read too; the first read takes longer while the planes are decompressed
- To stop the window from opening when a cross is placed, turn off **Open the spectrum after placing a cross on a cube** (`measure.open_spectrum`, on by default) under **Reports and graphs**
