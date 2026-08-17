use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

const MAPPING: &[u8] = include_bytes!("../../examples/pathology-mapping-v1.json");
const DIRECT_GEOJSON: &[u8] = include_bytes!("../../examples/pathology-annotations.geojson");
const SEG_GEOJSON: &[u8] = include_bytes!("../../examples/pathology-seg-holes.geojson");
const RASTER_PROFILE: &[u8] = include_bytes!("../../examples/raster-profile-v1.json");
const CONCATENATION_PROFILE: &[u8] =
    include_bytes!("../../examples/raster-profile-concatenation-v1.json");

pub(super) struct ConversionInputs {
    pub mapping: PathBuf,
    pub direct_geojson: PathBuf,
    pub seg_geojson: PathBuf,
    pub raster_profile: PathBuf,
    pub concatenation_profile: PathBuf,
    pub raster: PathBuf,
    pub wide_raster: PathBuf,
}

pub(super) fn prepare(directory: &Path) -> Result<ConversionInputs, String> {
    fs::create_dir(directory).map_err(|error| {
        format!(
            "could not create conversion input directory {}: {error}",
            directory.display()
        )
    })?;
    let inputs = ConversionInputs {
        mapping: directory.join("pathology-mapping-v1.json"),
        direct_geojson: directory.join("pathology-annotations.geojson"),
        seg_geojson: directory.join("pathology-seg-holes.geojson"),
        raster_profile: directory.join("raster-profile-v1.json"),
        concatenation_profile: directory.join("raster-profile-concatenation-v1.json"),
        raster: directory.join("probability-float32.npy"),
        wide_raster: directory.join("probability-wide-float32.npy"),
    };
    for (path, bytes) in [
        (&inputs.mapping, MAPPING),
        (&inputs.direct_geojson, DIRECT_GEOJSON),
        (&inputs.seg_geojson, SEG_GEOJSON),
        (&inputs.raster_profile, RASTER_PROFILE),
        (&inputs.concatenation_profile, CONCATENATION_PROFILE),
    ] {
        write_new(path, bytes)?;
    }
    let mut values = (0_u16..256)
        .map(|value| f32::from(value) / 255.0)
        .collect::<Vec<_>>();
    values[5 * 16 + 5] = f32::NAN;
    write_npy_f32(&inputs.raster, &[16, 16], &values)?;
    let wide = (0_u16..513)
        .map(|value| f32::from(value) / 512.0)
        .collect::<Vec<_>>();
    write_npy_f32(&inputs.wide_raster, &[1, 513], &wide)?;
    Ok(inputs)
}

fn write_npy_f32(path: &Path, shape: &[usize], values: &[f32]) -> Result<(), String> {
    let expected = shape
        .iter()
        .try_fold(1_usize, |length, dimension| length.checked_mul(*dimension));
    if expected != Some(values.len()) {
        return Err("NPY fixture shape does not match its value count".to_owned());
    }
    let shape = shape
        .iter()
        .map(usize::to_string)
        .collect::<Vec<_>>()
        .join(", ");
    let mut header = format!("{{'descr': '<f4', 'fortran_order': False, 'shape': ({shape},), }}");
    while (10 + header.len() + 1) % 64 != 0 {
        header.push(' ');
    }
    header.push('\n');
    let header_length = u16::try_from(header.len())
        .map_err(|_| "NPY fixture header exceeds version 1 length".to_owned())?;
    let mut bytes = Vec::with_capacity(10 + header.len() + values.len() * 4);
    bytes.extend(b"\x93NUMPY\x01\x00");
    bytes.extend(header_length.to_le_bytes());
    bytes.extend(header.as_bytes());
    for value in values {
        bytes.extend(value.to_le_bytes());
    }
    write_new(path, &bytes)
}

fn write_new(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| {
            format!(
                "could not create conversion input {}: {error}",
                path.display()
            )
        })?;
    file.write_all(bytes).map_err(|error| {
        format!(
            "could not write conversion input {}: {error}",
            path.display()
        )
    })
}
