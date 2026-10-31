---
title: "Measure and check"
---

Pixel values, clipping, focus and color, the pixel pipeline and batch export.

![Brightness changed with the pixel pipeline and compared side by side with the original, with quality values (PSNR, SSIM...) below](../../images/pipeline-compare.png)

## Measure pixel values (probe, ROI, analysis)

![Pixel values with the probe cross, mean and standard deviation of ROIs, and the histogram in the analysis window](../../images/check-probe.png)

![One large image, with per-channel histograms and minimum, maximum and mean in the analysis window](../../images/single-analysis.png)

- **Pixel probe** (**V**, or the button in the tool column): click the image to place a cross; the value of that pixel shows beside it. The value under the mouse is shown too. Values are read from the original image (raw 16-bit values, for example), not the reduced display image. Right-click a cross to copy its value or delete it. **Delete** also removes the selected cross
- **ROI** (**R**, or the button in the tool column): drag to draw a target ROI, **Shift+drag** for a background ROI. The mean and standard deviation of the pixels inside show beside it. Right-click for **Copy ROI statistics** / **Delete ROI**
- **Analysis window** (**View → Analysis**, **Ctrl+H**): histogram and quality values (SNR, CNR...) of the active slot's image. The range can be the whole image, the target ROI or the background ROI. Besides display brightness, the horizontal axis can be the **Original value** for 16-bit and floating-point gray images, and the **RAW sensor value** for camera RAW ([Camera RAW](raw.html#camera-raw)). When the pixel pipeline changes values, before and after are overlaid

## Check clipping, focus and color perception

These lay marks over the slot's image to check it. They go in as **Check** nodes of the pixel pipeline, so you can change their values in the pipeline window or put them after other nodes. Press the same key again to remove.

- **Clipping warning** (**O**): paints blown highlights red (all channels) or yellow (some channels, such as a blue sky or red flower where one color is blown), and crushed shadows blue. Judged on display brightness. To see whether it was blown at capture, use **RAW sensor value** in the analysis window
- **Focus peaking** (**K**): colors edges where brightness changes sharply (in-focus areas) and darkens the rest slightly. Edges blur when reduced, so judge at 100%
- **Show colors outside sRGB** (**Shift+O**): for images with a wider gamut than sRGB, paints magenta the colors sRGB cannot show ([Metadata window](view.html#metadata-window))
- **Color vision simulation** (**Image** menu): simulates how colors look to people with protan (P), deutan (D), tritan (T) and monochromat vision (the method of Machado et al. 2009). Use it to check that charts and color maps can be told apart

The values used when adding with keys (clipping level, peaking threshold, kind of color vision) can be changed in the **Pixel pipeline** settings.

## Color gauge

Images with a color map applied (**C** key) show a **gauge** in the slot (the colors with a scale of black, white and the values between). **G** or the button in the tool column shows or hides it. Drag the ends of the gauge to change the black and white values of that slot. With the **Gauge on gray images** setting, it shows on gray images without a color map too.

## Read QR codes

**Image → Read QR codes** reads QR codes in the active slot's image and shows their contents in the log and in the **QR codes** group of the metadata window.

## Apply a pixel pipeline to many images (vwbatch)

The node chain made in the pixel pipeline window (**View** menu), together with the slot's look (black and white, curve, color map), can be applied to other images and exported in bulk.

1. In the pixel pipeline window, **File → Export to file...** saves it as a `.vwpipe` file. **Read from file...** adds a `.vwpipe` received from someone to the list of saved pipelines
2. Run `vwbatch`, found next to vw, on a folder. The subfolder structure is kept at the destination. Files with the same name are skipped, not overwritten

```
vwbatch -p finish.vwpipe -o export -f png16 shots/2026-10-25
```

- `-f` is `png` (8-bit, default), `png16`, `tiff` (16-bit) or `jpeg` (quality with `-q`)
- To pick from `pipelines.toml` in the settings folder: `-p pipelines.toml -n name`
- Camera RAW is developed first (`--no-develop` uses the embedded preview). To write every frame of animations and the like, `--all-frames`
- Astronomy images (FITS) are exported looking the same as with vw's settings (stretch, curve, darks, flats). Without calibration, `--no-calibrate`; to use another settings file, `--settings file`
- If the nodes only process colors pixel by pixel, **File → Export the colors as .cube...** (and CLF) makes a 3D LUT usable in other software
