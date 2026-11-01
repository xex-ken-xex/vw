---
title: "Weather and ocean data (NetCDF)"
---

See and measure grids of weather, climate and ocean data with their values and units.

## Formats

NetCDF files (`.nc` `.nc4` `.cdf` `.netcdf`) open from a folder just like photos. vw reads NetCDF-3 (classic, 64-bit offset, 64-bit data) and NetCDF-4. HDF5 files that are not NetCDF, and HDF4 files, do not open yet.

Each numeric variable with two or more dimensions becomes an image. The variable with the largest plane is shown first.

## Switch variables

A file usually holds several variables, such as temperature, precipitation and wind.

- **N** shows the next variable and **Shift+N** the previous one
- Right-click the image → **Variable** to pick one from the list. The current variable is checked, and hovering an item shows its dimensions. Variables inside groups can be picked too
- Times and levels are planes in one slot; step them with the wheel. The label at the bottom right shows the time or level of the plane, such as "time = 2024-01-18 06:00". Switching variables keeps the same plane number

## Measure the real values

Pixels hold the file values with `scale_factor` and `add_offset` applied (with the `units`, such as K).

- The [pixel probe](check.html) (V), ROIs, the line profile graph and the color gauge use those values and units. For variables with a `scale_factor`, the line profile graph also shows the values stored in the file
- Pixels without a value (`_FillValue`, `missing_value` or outside `valid_range`, such as land in sea data) are drawn in the **Color of missing pixels** setting, which is kept when a color map is applied
- Data whose latitudes go from south to north are turned upside down so that north is at the top (the **North up** setting turns this off)

## Colors and black and white

- A color map (viridis by default) is applied from the start. Change it in the **NetCDF data** setting
- Black and white are chosen once per variable from all its times and levels, so planes keep the same coloring and can be compared. Variables such as temperature and precipitation have different units, so each has its own
- For data around zero, such as anomalies and winds, set **First black and white** to **Symmetric around zero** to put zero in the middle of the color map
- Black and white changed by hand on the gauge go back when you switch variables

## Metadata and search

The metadata pane shows the attributes of the current variable ("NetCDF variable"), all variables of the file ("NetCDF variables") and the file's global attributes ("NetCDF global attributes"). They are also stored in the [dataset](find.html) search catalog, so you can search by variable names and attributes.

## Settings

All settings are under **Earth science grids (NetCDF)**.

| Setting | What it does | Default |
| --- | --- | --- |
| NetCDF data | Whether to insert the color map stage from the start, and its color map | On, viridis |
| First black and white | Percentiles / Symmetric around zero and others | Percentiles |
| Black percentile, White percentile | Percents used with Percentiles | 0.5% / 99.5% |
| Same black and white for all planes | Choose one black and white from all planes of the variable. When off, each plane gets its own | On |
| North up | Turn data whose latitudes go from south to north upside down | On |
| Color of missing pixels | Color of pixels without a value | Gray |
