---
title: "Weather and ocean data (NetCDF, HDF)"
---

See and measure grids of weather, climate and ocean data with their values and units.

## Formats

NetCDF files (`.nc` `.nc4` `.cdf` `.netcdf`) open from a folder just like photos. vw reads NetCDF-3 (classic, 64-bit offset, 64-bit data) and NetCDF-4. HDF5 files (`.h5` `.hdf5` `.he5`) and HDF4 files (`.hdf` `.hdf4` `.h4` `.he2`) open too (see [HDF5 files](#hdf5-files) and [HDF4 files](#hdf4-files)).

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
- Variables whose name (such as `sst_anom`), `standard_name` or `long_name` says anomaly (differences from normal) automatically get black and white symmetric around zero and a color map with zero in the middle (coolwarm). Turn this off with **Anomalies around zero**
- For other data around zero, such as winds, set **First black and white** to **Symmetric around zero** to put zero in the middle of the color map
- Black and white changed by hand on the gauge go back when you switch variables

## Latitude, longitude, time and level

When the file says where its pixels are, vw reads the latitude and longitude of each pixel. These forms are read:

- Grids whose latitudes and longitudes are 1-D axes (the most common form)
- Curvilinear grids with 2-D latitude and longitude tables (named in the variable's `coordinates` attribute, as in ocean models)
- Projected grids (named in `grid_mapping`, with x / y in m or km). Lambert conformal conic, polar stereographic, Mercator, transverse Mercator, Albers equal area and rotated poles are turned into latitude and longitude

The positions are shown here:

- Next to the [pixel probe](check.html) cross and in the cursor readout at the bottom left, after the value and the position, as "35.68°N 139.77°E  time = 2024-01-18 06:00" with the time or level of the plane. **Ctrl+C** adds the latitude and longitude (south and west as negative numbers) and the plane label after the value, separated by commas
- The length tag of a measurement line and the line profile measure distance in km along the line on the earth. The table adds the latitude and longitude of the start and end, and the CSV adds `distance_km`, `latitude` and `longitude` columns. If any line has no latitude and longitude, all lines are measured in pixels
- The ROI graph's **Mean per frame** shows the dates or levels of the planes on the horizontal axis instead of frame numbers. The CSV adds a `plane` column
- The [map](find.html#see-photo-locations-on-a-map) outlines the NetCDF files of the open folder in red. Click an outline or its center point to see the name and open it, as with photos. Where outlines overlap, the smaller one is picked
- The key action **Show the slot's data on the map** opens the map and fits it to the current slot's data. It has no key at first; assign one in the key settings

Grids with longitudes from 0 to 360 degrees (Japan on the left, America on the right) can be turned with **Longitudes from -180 to 180** so the left edge is 180 degrees west. Pixel positions in graphs and CSV follow the turned grid.

## HDF5 files

HDF5 files that are not NetCDF, such as satellite observations, are shown the same way as NetCDF. Files that are NetCDF-4 inside are read as NetCDF even with the `.h5` extension. **Format** in the metadata is "HDF5" for HDF5 that is not NetCDF, and "NetCDF-4" for NetCDF.

- Every numeric dataset with two or more dimensions, also inside groups, becomes an image, switched with **N** / **Shift+N** or right-click **Variable**. One-dimensional datasets, strings, compounds and references are not images; they appear only in the metadata
- Datasets with three or more dimensions are stepped with the wheel over the leading dimensions. Bands stored as separate datasets (such as `B01` to `B07`) are switched with N
- HDF5 has no dimension names, so dimensions are named by their size, like "dim_400"
- The attributes `scale_factor`, `add_offset`, `_FillValue`, `missing_value` and `valid_range` are applied as in NetCDF. Attributes with other names (such as `ScaleFactor`) are not applied; the stored values are shown
- Latitude and longitude datasets of the same shape (in the same group, or `Latitude` and `Longitude` in the HDF-EOS5 "Geolocation Fields", found by name or `units`) give each pixel its position, so the cross, kilometers along lines, the ROI graph and the map work as in NetCDF. Values out of range, like -999, leave the pixel without a position. The latitude and longitude datasets stay in the list but are not shown first
- LZF compression (used by h5py) is read
- Enumerations show their values only, not their names, for now
- Positions of HDF-EOS5 grids (projections written in StructMetadata) are not read yet

The extensions are set with **Extensions opened as HDF** (a regular expression). Narrow it in folders with many HDF5 files that are not images, such as machine learning weights.

## HDF4 files

HDF4 (including HDF-EOS2), the older format used by MODIS and other satellite data, is shown the same way as NetCDF. It is a different format from HDF5 inside; whether a `.hdf` file is HDF4 or HDF5 is decided from its first bytes. **Format** in the metadata is "HDF4".

- Scientific datasets (SDS) become images, switched with **N** / **Shift+N** or right-click **Variable**. Three or more dimensions are stepped with the wheel. Dimension names and attributes are shown as they are
- HDF4 calibration (value = `scale_factor` × (stored − `add_offset`), datasets with the `calibrated_nt` attribute) subtracts before scaling, following HDF4. The `add_offset` in the metadata is turned into its CF meaning (added after scaling), and the original value is shown as `hdf4_add_offset`. `scale_factor` and `add_offset` without `calibrated_nt` are applied as in CF
- `_FillValue` and values outside `valid_range` are missing
- Latitude and longitude datasets of the same shape (such as `Latitude` and `Longitude`) give each pixel its position
- deflate, RLE and Huffman compression and chunks are read. szip and N-bit compression are not read yet
- HDF-EOS2 grids (projections in StructMetadata) and coarser latitudes and longitudes that need interpolation are not read yet
- When items in different HDF-EOS2 grids share a name, the second and later get a number, like "name_2"
- Old images (GR, RIS) and tables (Vdata) are not read
- The whole file is unpacked when it opens, so large files take some time and memory

## Metadata and search

The metadata pane shows the attributes of the current variable ("NetCDF variable"), all variables of the file ("NetCDF variables") and the file's global attributes ("NetCDF global attributes"). They are also stored in the [dataset](find.html) search catalog, so you can search by variable names and attributes.

## Settings

All settings are under **Earth science data (NetCDF, HDF)**. HDF5 and HDF4 files use the same settings.

| Setting | What it does | Default |
| --- | --- | --- |
| NetCDF data | Whether to insert the color map stage from the start, and its color map | On, viridis |
| First black and white | Percentiles / Symmetric around zero and others | Percentiles |
| Black percentile, White percentile | Percents used with Percentiles | 0.5% / 99.5% |
| Same black and white for all planes | Choose one black and white from all planes of the variable. When off, each plane gets its own | On |
| North up | Turn data whose latitudes go from south to north upside down | On |
| Color of missing pixels | Color of pixels without a value | Gray |
| Show coordinates | Show latitude, longitude and the plane's time or level at the cross, the line profile, the ROI graph and the map | On |
| Longitudes from -180 to 180 | Make the left edge of 0 to 360 degree grids 180 degrees west | Off |
| Anomalies around zero | Give anomaly variables black and white symmetric around zero and coolwarm | On |
| Extensions opened as HDF | Regular expression for the extensions of files opened as HDF5 or HDF4. `.nc` and the like always open | `^(h5\|hdf5\|he5\|hdf\|hdf4\|h4\|he2)$` |
