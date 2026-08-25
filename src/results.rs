use std::collections::{BTreeMap, BTreeSet};
use std::fmt::{self, Write as _};
use std::fs::{self, File, OpenOptions};
use std::io::{BufWriter, Write as _};
use std::path::Path;

use serde_json::{Map, Value, json};

mod manifest;
mod provenance;
mod writer;

pub(crate) use manifest::sha256_file;
pub use provenance::{Provenance, collect_provenance};
pub use writer::RunWriter;

#[derive(Debug)]
pub struct RunError(String);

impl fmt::Display for RunError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for RunError {}

fn summarize(observations: &[Value]) -> Value {
    let mut statuses = BTreeMap::<String, u64>::new();
    for observation in observations {
        *statuses
            .entry(
                observation
                    .get("status")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown")
                    .to_owned(),
            )
            .or_default() += 1;
    }
    json!({
        "observation_count": observations.len(),
        "status_counts": statuses,
        "coordinate_error_max_px": maximum(observations, "coordinate_error_max_px"),
        "dice_min": minimum(observations, "dice"),
        "runtime_total_ms": observations.iter().filter_map(|item| number(item, "runtime_ms")).sum::<f64>(),
        "peak_rss_max_bytes": maximum(observations, "peak_rss_bytes"),
    })
}

fn maximum(observations: &[Value], key: &str) -> Option<f64> {
    observations
        .iter()
        .filter_map(|item| number(item, key))
        .reduce(f64::max)
}

fn minimum(observations: &[Value], key: &str) -> Option<f64> {
    observations
        .iter()
        .filter_map(|item| number(item, key))
        .reduce(f64::min)
}

fn number(value: &Value, key: &str) -> Option<f64> {
    value.get(key).and_then(Value::as_f64)
}

fn write_jsonl_exclusive(path: &Path, values: &[Value]) -> Result<(), RunError> {
    let mut writer = BufWriter::new(create_exclusive(path)?);
    for value in values {
        serde_json::to_writer(&mut writer, value).map_err(|error| {
            RunError(format!("could not serialize {}: {error}", path.display()))
        })?;
        writer
            .write_all(b"\n")
            .map_err(|error| RunError(format!("could not write {}: {error}", path.display())))?;
    }
    writer
        .flush()
        .map_err(|error| RunError(format!("could not flush {}: {error}", path.display())))
}

fn write_json_exclusive(path: &Path, value: &Value) -> Result<(), RunError> {
    let mut writer = BufWriter::new(create_exclusive(path)?);
    serde_json::to_writer_pretty(&mut writer, value)
        .map_err(|error| RunError(format!("could not serialize {}: {error}", path.display())))?;
    writer
        .write_all(b"\n")
        .and_then(|()| writer.flush())
        .map_err(|error| RunError(format!("could not write {}: {error}", path.display())))
}

fn write_csv_exclusive(path: &Path, rows: &[Value]) -> Result<(), RunError> {
    let fields: BTreeSet<_> = rows
        .iter()
        .filter_map(Value::as_object)
        .flat_map(Map::keys)
        .cloned()
        .collect();
    let fields: Vec<_> = fields.into_iter().collect();
    let mut writer = BufWriter::new(create_exclusive(path)?);
    write_csv_row(&mut writer, fields.iter().map(String::as_str), path)?;
    for row in rows {
        let object = row.as_object().ok_or_else(|| {
            RunError("CSV rows must be JSON objects before serialization".to_owned())
        })?;
        let values = fields
            .iter()
            .map(|field| csv_value(object.get(field)))
            .collect::<Result<Vec<_>, RunError>>()?;
        write_csv_row(&mut writer, values.iter().map(String::as_str), path)?;
    }
    writer
        .flush()
        .map_err(|error| RunError(format!("could not flush {}: {error}", path.display())))
}

fn write_csv_row(
    writer: &mut impl std::io::Write,
    values: impl Iterator<Item = impl AsRef<str>>,
    path: &Path,
) -> Result<(), RunError> {
    let row = values
        .map(|value| csv_escape(value.as_ref()))
        .collect::<Vec<_>>()
        .join(",");
    writeln!(writer, "{row}")
        .map_err(|error| RunError(format!("could not write {}: {error}", path.display())))
}

fn csv_value(value: Option<&Value>) -> Result<String, RunError> {
    match value {
        None | Some(Value::Null) => Ok(String::new()),
        Some(Value::String(value)) => Ok(value.clone()),
        Some(value) => serde_json::to_string(value)
            .map_err(|error| RunError(format!("could not serialize CSV value: {error}"))),
    }
}

fn csv_escape(value: &str) -> String {
    if value.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_owned()
    }
}

fn write_figures(directory: &Path, observations: &[Value]) -> Result<(), RunError> {
    fs::create_dir(directory).map_err(|error| {
        RunError(format!(
            "could not create figure directory {}: {error}",
            directory.display()
        ))
    })?;
    for (filename, key, title) in [
        (
            "coordinate-error.svg",
            "coordinate_error_max_px",
            "Maximum coordinate error (level-0 pixels)",
        ),
        ("runtime.svg", "runtime_ms", "Runtime (milliseconds)"),
        ("memory.svg", "peak_rss_bytes", "Peak RSS (bytes)"),
    ] {
        write_bar_figure(&directory.join(filename), observations, key, title)?;
    }
    let mask_rows: Vec<_> = observations
        .iter()
        .filter_map(|observation| {
            observation.get("mask_metrics").and_then(Value::as_object)?;
            Some(observation)
        })
        .collect();
    write_mask_figure(&directory.join("mask-metrics.svg"), &mask_rows)
}

fn write_bar_figure(
    path: &Path,
    observations: &[Value],
    key: &str,
    title: &str,
) -> Result<(), RunError> {
    let values: Vec<_> = observations
        .iter()
        .map(|observation| number(observation, key).unwrap_or(0.0))
        .collect();
    let labels: Vec<_> = observations
        .iter()
        .enumerate()
        .map(|(index, observation)| {
            observation
                .get("case_id")
                .and_then(Value::as_str)
                .map_or_else(|| (index + 1).to_string(), str::to_owned)
        })
        .collect();
    write_svg_bars(path, title, &labels, &values)
}

fn write_mask_figure(path: &Path, observations: &[&Value]) -> Result<(), RunError> {
    const PANELS: [(&str, &str); 8] = [
        ("dice", "Dice"),
        ("area_difference_pixels", "Area difference (pixels)"),
        ("centroid_distance_pixels", "Centroid distance (pixels)"),
        ("hd95_pixels", "HD95 (pixels)"),
        ("assd_pixels", "ASSD (pixels)"),
        ("overlap_difference_pixels", "Overlap difference (pixels)"),
        ("expected_components", "Expected components"),
        ("expected_holes", "Expected holes"),
    ];
    let labels: Vec<_> = observations
        .iter()
        .enumerate()
        .map(|(index, observation)| {
            observation
                .get("case_id")
                .and_then(Value::as_str)
                .map_or_else(|| (index + 1).to_string(), str::to_owned)
        })
        .collect();
    let label_count = u32::try_from(labels.len())
        .map_err(|_| RunError("too many mask observations to render".to_owned()))?;
    let width = 1_200_u32;
    let height = 680_u32;
    let mut svg = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{width}\" height=\"{height}\" viewBox=\"0 0 {width} {height}\">\n<rect width=\"100%\" height=\"100%\" fill=\"white\"/>\n"
    );
    for (panel, (key, title)) in PANELS.into_iter().enumerate() {
        let panel = u32::try_from(panel)
            .map_err(|_| RunError("too many mask panels to render".to_owned()))?;
        let left = 20.0 + f64::from(panel % 4) * 295.0;
        let top = 20.0 + f64::from(panel / 4) * 325.0;
        let baseline = top + 235.0;
        let values: Vec<_> = observations
            .iter()
            .map(|observation| {
                observation
                    .pointer(&format!("/mask_metrics/{key}"))
                    .and_then(Value::as_f64)
                    .unwrap_or(0.0)
            })
            .collect();
        let maximum = values.iter().copied().reduce(f64::max).unwrap_or(0.0);
        let scale = if maximum > 0.0 { 190.0 / maximum } else { 0.0 };
        let slot = if label_count == 0 {
            1.0
        } else {
            250.0 / f64::from(label_count)
        };
        writeln!(
            svg,
            "<text x=\"{left:.2}\" y=\"{:.2}\" font-family=\"sans-serif\" font-size=\"15\">{}</text>",
            top + 18.0,
            xml_escape(title)
        )
        .map_err(|_| RunError("could not render mask SVG title".to_owned()))?;
        writeln!(
            svg,
            "<line x1=\"{left:.2}\" y1=\"{baseline:.2}\" x2=\"{:.2}\" y2=\"{baseline:.2}\" stroke=\"#222\"/>",
            left + 265.0
        )
        .map_err(|_| RunError("could not render mask SVG baseline".to_owned()))?;
        for (index, (label, value)) in labels.iter().zip(&values).enumerate() {
            let index = u32::try_from(index)
                .map_err(|_| RunError("too many mask observations to render".to_owned()))?;
            let x = left + 8.0 + f64::from(index) * slot;
            let bar_height = value.max(0.0) * scale;
            let y = baseline - bar_height;
            let bar_width = (slot * 0.65).max(2.0);
            writeln!(
                svg,
                "<rect x=\"{x:.2}\" y=\"{y:.2}\" width=\"{bar_width:.2}\" height=\"{bar_height:.2}\" fill=\"#28536b\"><title>{}: {value:.6}</title></rect>",
                xml_escape(label)
            )
            .map_err(|_| RunError("could not render mask SVG bar".to_owned()))?;
            writeln!(
                svg,
                "<text x=\"{x:.2}\" y=\"{:.2}\" transform=\"rotate(35 {x:.2} {:.2})\" font-family=\"sans-serif\" font-size=\"8\">{}</text>",
                baseline + 14.0,
                baseline + 14.0,
                xml_escape(label)
            )
            .map_err(|_| RunError("could not render mask SVG label".to_owned()))?;
        }
    }
    svg.push_str("</svg>\n");
    let mut file = create_exclusive(path)?;
    file.write_all(svg.as_bytes())
        .map_err(|error| RunError(format!("could not write {}: {error}", path.display())))
}

fn write_svg_bars(
    path: &Path,
    title: &str,
    labels: &[String],
    values: &[f64],
) -> Result<(), RunError> {
    let label_count = u32::try_from(labels.len())
        .map_err(|_| RunError("too many observations to render".to_owned()))?;
    let content_width = label_count
        .checked_mul(80)
        .and_then(|value| value.checked_add(120))
        .ok_or_else(|| RunError("figure width exceeds the SVG limit".to_owned()))?;
    let width = u32::max(720, content_width);
    let height = 420_u32;
    let chart_height = 280.0;
    let maximum = values.iter().copied().reduce(f64::max).unwrap_or(0.0);
    let scale = if maximum > 0.0 {
        chart_height / maximum
    } else {
        0.0
    };
    let slot = if labels.is_empty() {
        1.0
    } else {
        (f64::from(width) - 120.0) / f64::from(label_count)
    };
    let mut svg = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{width}\" height=\"{height}\" viewBox=\"0 0 {width} {height}\">\n<rect width=\"100%\" height=\"100%\" fill=\"white\"/>\n<text x=\"60\" y=\"32\" font-family=\"sans-serif\" font-size=\"20\">{}</text>\n<line x1=\"60\" y1=\"340\" x2=\"{}\" y2=\"340\" stroke=\"#222\"/>\n",
        xml_escape(title),
        width - 30
    );
    for (index, (label, value)) in labels.iter().zip(values).enumerate() {
        let index = u32::try_from(index)
            .map_err(|_| RunError("too many observations to render".to_owned()))?;
        let x = 70.0 + f64::from(index) * slot;
        let bar_height = value.max(0.0) * scale;
        let y = 340.0 - bar_height;
        let bar_width = (slot * 0.65).max(2.0);
        writeln!(
            svg,
            "<rect x=\"{x:.2}\" y=\"{y:.2}\" width=\"{bar_width:.2}\" height=\"{bar_height:.2}\" fill=\"#28536b\"><title>{}: {value:.6}</title></rect>",
            xml_escape(label)
        )
        .map_err(|_| RunError("could not render SVG bar".to_owned()))?;
        writeln!(
            svg,
            "<text x=\"{x:.2}\" y=\"360\" transform=\"rotate(35 {x:.2} 360)\" font-family=\"sans-serif\" font-size=\"10\">{}</text>",
            xml_escape(label)
        )
        .map_err(|_| RunError("could not render SVG label".to_owned()))?;
    }
    svg.push_str("</svg>\n");
    let mut file = create_exclusive(path)?;
    file.write_all(svg.as_bytes())
        .map_err(|error| RunError(format!("could not write {}: {error}", path.display())))
}

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn create_exclusive(path: &Path) -> Result<File, RunError> {
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| RunError(format!("could not create {}: {error}", path.display())))
}
