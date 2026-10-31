---
title: "Astrophotography (FITS)"
---

Make faint images easy to see, then calibrate, select and finish them.

![A faint nebula FITS made visible with a color map and gauge](../../images/en/astro-nebula.png)

## See what you captured on a sky chart

![Areas captured by the FITS files of a folder on the sky chart](../../images/en/astro-skychart.png)

**View → Sky chart** opens the sky chart window. It outlines where on the sky each astronomy image (FITS) was taken, over stars (to magnitude 6) and constellation lines, borders and names. No network is used.

- The outline depends on what the header tells
  - **Solid yellow**: images with WCS in the header (`CTYPE` `CRVAL` `CRPIX` `CD`...). The exact area is known. TAN, SIN, ARC, STG and ZEA projections are read
  - **Dotted red**: images with the mount pointing (`RA` `DEC`, or `OBJCTRA` `OBJCTDEC`), focal length (`FOCALLEN`) and pixel size (`XPIXSZ` `YPIXSZ`). The area estimated from the field of view is drawn with north up (rotation unknown)
  - **Red cross**: images with only the mount pointing
- At the top right, switch between **This folder** and **All datasets** (FITS in the search catalogs)
- Drag to move, wheel to zoom, double-click to zoom 2x. Right ascension and declination under the cursor show at the bottom left
- Click an outline for its thumbnail, file name, `OBJECT`, `DATE-OBS` and center coordinates. **Open** goes to the image's folder, selects the image and loads it first. Where outlines overlap, such as a series of shots of the same target, pick from a list
- The outline of the active slot's image is drawn thicker
- Outline colors can be changed in the **Sky chart** settings. Turn off **Use the sky chart** to remove it from the menu

## Apply darks and flats

When you open an astronomy light frame (FITS), vw looks for matching dark, flat and bias masters and applies them before coloring and choosing black and white. You see the image without amp glow, hot pixels, vignetting and dust shadows.

- Where it looks: the open folder and its parent, and neighboring folders such as `darks` `flats` `biases` `darkflats` `calibration` `masters`. Folders in the **Calibration library** setting are searched too
- The kind comes from `IMAGETYP` (`DARK`, `Dark Frame`, `Master Flat`, IRAF `zero`...), or else from the folder name. Only masters are used (`IMAGETYP` `Master …`, `master` in the name, `NCOMBINE` 2 or more, or inside a `masters` folder)
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
