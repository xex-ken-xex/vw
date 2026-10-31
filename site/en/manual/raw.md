---
title: "Photos shot in RAW"
---

See them at once, compare with the camera JPEG and check the exposure.

## Camera RAW

![Histogram of RAW sensor values per color, in stops below the white level, with the share of clipped and below-black values](../../images/raw-sensor.png)

- When opened, the preview image the camera embedded in the RAW file shows at once
- vw then develops the sensor data in the background and swaps in the detailed image. While developing, the header shows a film icon
- To stop background developing, turn off the **Develop RAW in the background** setting. Then develop one at a time with **D** or right-click on the image → **Develop RAW from sensor data**
- The brightness of the developed image is matched to the preview
- The embedded preview is kept after developing, so **J** or the **RAW** / **JPEG** mark in the header switches between the camera JPEG and vw's development. Keeping both uses memory; if you do not need it, turn off **Keep the RAW preview after developing**
- Choose **RAW sensor value** on the horizontal axis of the analysis window (**Ctrl+H**) for a histogram of sensor values before developing, per color, on a scale of stops below the white level. The table below shows the share of values that reached the white level (clipped) and of values at or below the black level, and the headroom to white (stops). You can check exposure without being misled by the camera JPEG or the brightness matching of development

## Features to use with RAW

- **Align and compare with the camera JPEG**: a RAW and a JPEG with the same name pair up automatically, with a chain icon in the header. Click it to align them and measure differences (PSNR, SSIM, color difference...) ([Align and compare similar images](sort.html#align-and-compare-similar-images))
- **Keep one shot of a burst**: shots taken close together are grouped, and the sharpest one gets a star ([Sort photos](sort.html))
- **Develop and export in bulk**: save a pixel pipeline and apply it to RAW files with `vwbatch` ([Apply a pixel pipeline to many images](check.html#apply-a-pixel-pipeline-to-many-images-vwbatch))
- Supported cameras and file types are in [Supported formats in detail](reference.html#supported-formats-in-detail)
