use std::fs::{self, OpenOptions};
use std::io::Write;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::time::Duration;

use serde::Serialize;
use serde_json::{Map, Value, json};

mod local;

pub use local::LocalOrthanc;

const MAX_DICOMWEB_RESPONSE_BYTES: u64 = 512 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct DicomwebObject {
    pub path: PathBuf,
    pub study_instance_uid: String,
    pub series_instance_uid: String,
    pub sop_instance_uid: String,
}

#[derive(Debug, Serialize)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "STOW, QIDO, WADO, and semantic equality are independent study outcomes"
)]
pub struct DicomwebObservation {
    pub sop_instance_uid: String,
    pub stow: bool,
    pub qido: bool,
    pub wado: bool,
    pub semantic_equal: bool,
    pub message: String,
}

#[derive(Debug, Serialize)]
pub struct DicomwebTransportResult {
    pub observations: Vec<DicomwebObservation>,
}

impl DicomwebTransportResult {
    #[must_use]
    pub fn is_ok(&self) -> bool {
        !self.observations.is_empty()
            && self.observations.iter().all(|observation| {
                observation.stow
                    && observation.qido
                    && observation.wado
                    && observation.semantic_equal
            })
    }
}

/// Reject every Orthanc endpoint that is not an explicit HTTP loopback URL.
///
/// # Errors
///
/// Returns an error for malformed, credential-bearing, or non-loopback URLs.
pub fn validate_loopback_url(url: &str) -> Result<(), String> {
    let authority = url
        .strip_prefix("http://")
        .and_then(|remainder| remainder.split('/').next())
        .filter(|authority| !authority.is_empty() && !authority.contains('@'))
        .ok_or_else(|| "Orthanc URL must be an HTTP loopback URL".to_owned())?;
    let host = if let Some(remainder) = authority.strip_prefix('[') {
        let (host, suffix) = remainder
            .split_once(']')
            .ok_or_else(|| "Orthanc URL has an invalid IPv6 address".to_owned())?;
        if !suffix.is_empty()
            && suffix
                .strip_prefix(':')
                .is_none_or(|port| port.parse::<u16>().is_err())
        {
            return Err("Orthanc URL has an invalid IPv6 authority".to_owned());
        }
        host
    } else {
        if authority.matches(':').count() > 1 {
            return Err("IPv6 loopback addresses must use URL brackets".to_owned());
        }
        authority
            .rsplit_once(':')
            .filter(|(_, port)| port.parse::<u16>().is_ok())
            .map_or(authority, |(host, _)| host)
    };
    if IpAddr::from_str(host).is_ok_and(|address| address.is_loopback()) {
        Ok(())
    } else {
        Err("Orthanc URL must resolve directly to a loopback address".to_owned())
    }
}

/// Build an isolated Orthanc configuration with no remotely reachable service.
#[must_use]
pub fn orthanc_configuration(
    storage_directory: &Path,
    http_port: u16,
    plugins: &[PathBuf],
) -> Map<String, Value> {
    let mut configuration = Map::from_iter([
        ("Name".to_owned(), json!("wsi-annotation-interop-local")),
        ("StorageDirectory".to_owned(), json!(storage_directory)),
        ("IndexDirectory".to_owned(), json!(storage_directory)),
        ("RemoteAccessAllowed".to_owned(), json!(false)),
        ("AuthenticationEnabled".to_owned(), json!(false)),
        ("HttpServerEnabled".to_owned(), json!(true)),
        ("HttpBindAddresses".to_owned(), json!(["127.0.0.1"])),
        ("HttpPort".to_owned(), json!(http_port)),
        ("DicomServerEnabled".to_owned(), json!(false)),
        (
            "DicomWeb".to_owned(),
            json!({"Enable": true, "Root": "/dicom-web/"}),
        ),
    ]);
    if !plugins.is_empty() {
        configuration.insert(
            "Plugins".to_owned(),
            Value::Array(
                plugins
                    .iter()
                    .map(|path| Value::String(path.to_string_lossy().into_owned()))
                    .collect(),
            ),
        );
    }
    configuration
}

/// Verify STOW, QIDO, WADO, and semantic equality through a loopback `DICOMweb` endpoint.
///
/// `normalize` receives the object index and either its original or retrieved file.
///
/// # Errors
///
/// Returns an error for unsafe endpoints, empty inputs, local I/O, or malformed
/// successful responses. Per-instance protocol failures are retained as observations.
pub fn verify_dicomweb_transport(
    base_url: &str,
    objects: &[DicomwebObject],
    retrieval_directory: &Path,
    mut normalize: impl FnMut(usize, &Path) -> Result<Value, String>,
) -> Result<DicomwebTransportResult, String> {
    validate_loopback_url(base_url)?;
    if objects.is_empty() {
        return Err("at least one DICOM object is required".to_owned());
    }
    fs::create_dir_all(retrieval_directory)
        .map_err(|error| format!("could not create WADO directory: {error}"))?;
    let agent = http_agent();
    let endpoint = base_url.trim_end_matches('/');
    let boundary = format!("wsi-interop-{}", std::process::id());
    let stow_body = stow_body(objects, &boundary)?;
    let stow = agent
        .post(&format!("{endpoint}/studies"))
        .header(
            "Content-Type",
            &format!("multipart/related; type=\"application/dicom\"; boundary={boundary}"),
        )
        .header("Accept", "application/dicom+json")
        .send(stow_body.as_slice());
    if let Err(error) = stow {
        return Ok(DicomwebTransportResult {
            observations: objects
                .iter()
                .map(|object| DicomwebObservation {
                    sop_instance_uid: object.sop_instance_uid.clone(),
                    stow: false,
                    qido: false,
                    wado: false,
                    semantic_equal: false,
                    message: format!("STOW failed: {error}"),
                })
                .collect(),
        });
    }

    let mut observations = Vec::with_capacity(objects.len());
    for (index, object) in objects.iter().enumerate() {
        observations.push(verify_instance(
            &agent,
            endpoint,
            object,
            index,
            retrieval_directory,
            &mut normalize,
        ));
    }
    Ok(DicomwebTransportResult { observations })
}

fn verify_instance(
    agent: &ureq::Agent,
    endpoint: &str,
    object: &DicomwebObject,
    index: usize,
    retrieval_directory: &Path,
    normalize: &mut impl FnMut(usize, &Path) -> Result<Value, String>,
) -> DicomwebObservation {
    let instance_root = format!(
        "{endpoint}/studies/{}/series/{}/instances",
        object.study_instance_uid, object.series_instance_uid
    );
    let qido_url = format!("{instance_root}?SOPInstanceUID={}", object.sop_instance_uid);
    let qido = agent
        .get(&qido_url)
        .header("Accept", "application/dicom+json")
        .call()
        .map_err(|error| error.to_string())
        .and_then(|mut response| {
            response
                .body_mut()
                .read_to_string()
                .map_err(|error| error.to_string())
        })
        .and_then(|body| qido_contains(&body, &object.sop_instance_uid));
    match qido {
        Err(error) => {
            return failed_observation(object, true, false, false, format!("QIDO failed: {error}"));
        }
        Ok(false) => {
            return failed_observation(
                object,
                true,
                false,
                false,
                "QIDO omitted instance".to_owned(),
            );
        }
        Ok(true) => {}
    }

    let wado_url = format!("{instance_root}/{}", object.sop_instance_uid);
    let retrieved = retrieval_directory.join(format!("{}-retrieved.dcm", object.sop_instance_uid));
    let wado = retrieve_dicom(agent, &wado_url, &retrieved);
    if let Err(error) = wado {
        return failed_observation(object, true, true, false, format!("WADO failed: {error}"));
    }
    let semantic = normalize(index, &object.path)
        .and_then(|original| normalize(index, &retrieved).map(|actual| original == actual));
    match semantic {
        Ok(true) => DicomwebObservation {
            sop_instance_uid: object.sop_instance_uid.clone(),
            stow: true,
            qido: true,
            wado: true,
            semantic_equal: true,
            message: String::new(),
        },
        Ok(false) => failed_observation(
            object,
            true,
            true,
            true,
            "semantic content changed after WADO retrieval".to_owned(),
        ),
        Err(error) => failed_observation(
            object,
            true,
            true,
            true,
            format!("semantic comparison failed: {error}"),
        ),
    }
}

fn http_agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(30)))
        .build()
        .into()
}

fn stow_body(objects: &[DicomwebObject], boundary: &str) -> Result<Vec<u8>, String> {
    let mut body = Vec::new();
    for object in objects {
        let bytes = fs::read(&object.path).map_err(|error| {
            format!(
                "could not read STOW object {}: {error}",
                object.path.display()
            )
        })?;
        write!(
            body,
            "--{boundary}\r\nContent-Type: application/dicom\r\n\r\n"
        )
        .map_err(|error| format!("could not construct STOW body: {error}"))?;
        body.extend_from_slice(&bytes);
        body.extend_from_slice(b"\r\n");
    }
    write!(body, "--{boundary}--\r\n")
        .map_err(|error| format!("could not finish STOW body: {error}"))?;
    Ok(body)
}

fn qido_contains(body: &str, sop_instance_uid: &str) -> Result<bool, String> {
    let results: Value = serde_json::from_str(body)
        .map_err(|error| format!("QIDO returned invalid JSON: {error}"))?;
    let Some(results) = results.as_array() else {
        return Err("QIDO response must be an array".to_owned());
    };
    Ok(results.iter().any(|result| {
        result
            .pointer("/00080018/Value")
            .and_then(Value::as_array)
            .is_some_and(|values| values.iter().any(|value| value == sop_instance_uid))
    }))
}

fn retrieve_dicom(agent: &ureq::Agent, url: &str, output: &Path) -> Result<(), String> {
    let mut response = agent
        .get(url)
        .header(
            "Accept",
            "multipart/related; type=\"application/dicom\", application/dicom",
        )
        .call()
        .map_err(|error| error.to_string())?;
    let content_type = response
        .headers()
        .get("Content-Type")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("application/dicom")
        .to_owned();
    let body = response
        .body_mut()
        .with_config()
        .limit(MAX_DICOMWEB_RESPONSE_BYTES)
        .read_to_vec()
        .map_err(|error| error.to_string())?;
    let dicom = if content_type
        .to_ascii_lowercase()
        .starts_with("multipart/related")
    {
        let boundary = multipart_boundary(&content_type)?;
        multipart_dicom(&body, boundary.as_bytes())?
    } else {
        body.as_slice()
    };
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(output)
        .map_err(|error| format!("could not create {}: {error}", output.display()))?;
    file.write_all(dicom)
        .map_err(|error| format!("could not write {}: {error}", output.display()))
}

fn multipart_boundary(content_type: &str) -> Result<String, String> {
    content_type
        .split(';')
        .map(str::trim)
        .find_map(|part| {
            let (name, value) = part.split_once('=')?;
            name.eq_ignore_ascii_case("boundary").then_some(value)
        })
        .map(|value| value.trim_matches('"').to_owned())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "multipart WADO response has no boundary".to_owned())
}

fn multipart_dicom<'a>(body: &'a [u8], boundary: &[u8]) -> Result<&'a [u8], String> {
    let header_end = find_bytes(body, b"\r\n\r\n")
        .map(|index| index + 4)
        .ok_or_else(|| "multipart WADO part has no header terminator".to_owned())?;
    let mut terminator = b"\r\n--".to_vec();
    terminator.extend_from_slice(boundary);
    let end = find_bytes(&body[header_end..], &terminator)
        .map(|index| header_end + index)
        .ok_or_else(|| "multipart WADO part has no closing boundary".to_owned())?;
    Ok(&body[header_end..end])
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    (!needle.is_empty())
        .then(|| {
            haystack
                .windows(needle.len())
                .position(|part| part == needle)
        })
        .flatten()
}

fn failed_observation(
    object: &DicomwebObject,
    stow: bool,
    qido: bool,
    wado: bool,
    message: String,
) -> DicomwebObservation {
    DicomwebObservation {
        sop_instance_uid: object.sop_instance_uid.clone(),
        stow,
        qido,
        wado,
        semantic_equal: false,
        message,
    }
}
