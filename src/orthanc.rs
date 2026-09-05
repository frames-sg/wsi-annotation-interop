use std::fs::{self, File};
use std::io::{Cursor, Read, Seek, SeekFrom, Write};
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::Serialize;
use serde_json::{Map, Value, json};

mod local;
mod multipart;

pub use local::LocalOrthanc;

use multipart::{extract_single_dicom_part, media_type, multipart_boundary};

static BOUNDARY_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Copy)]
pub struct DicomwebLimits {
    pub stow_response_bytes: u64,
    pub qido_response_bytes: u64,
    pub wado_response_bytes: u64,
}

impl Default for DicomwebLimits {
    fn default() -> Self {
        Self {
            stow_response_bytes: 4 * 1024 * 1024,
            qido_response_bytes: 4 * 1024 * 1024,
            wado_response_bytes: 512 * 1024 * 1024,
        }
    }
}

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
    pub stow_request_bytes: Option<u64>,
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
    normalize: impl FnMut(usize, &Path) -> Result<Value, String>,
) -> Result<DicomwebTransportResult, String> {
    verify_dicomweb_transport_with_limits(
        base_url,
        objects,
        retrieval_directory,
        DicomwebLimits::default(),
        normalize,
    )
}

/// Verify `DICOMweb` transport with explicit response-body limits.
///
/// # Errors
///
/// Returns an error for unsafe endpoints, empty inputs, invalid limits, or local setup I/O.
pub fn verify_dicomweb_transport_with_limits(
    base_url: &str,
    objects: &[DicomwebObject],
    retrieval_directory: &Path,
    limits: DicomwebLimits,
    mut normalize: impl FnMut(usize, &Path) -> Result<Value, String>,
) -> Result<DicomwebTransportResult, String> {
    validate_loopback_url(base_url)?;
    if objects.is_empty() {
        return Err("at least one DICOM object is required".to_owned());
    }
    fs::create_dir_all(retrieval_directory)
        .map_err(|error| format!("could not create WADO directory: {error}"))?;
    if limits.stow_response_bytes == 0
        || limits.qido_response_bytes == 0
        || limits.wado_response_bytes == 0
    {
        return Err("DICOMweb response limits must be positive".to_owned());
    }
    let agent = http_agent();
    let endpoint = base_url.trim_end_matches('/');
    let context = InstanceContext {
        agent: &agent,
        endpoint,
        retrieval_directory,
        limits,
    };
    let mut observations = Vec::with_capacity(objects.len());
    for (index, object) in objects.iter().enumerate() {
        match stow_instance(&agent, endpoint, object, limits.stow_response_bytes) {
            Ok(request_bytes) => observations.push(verify_instance(
                &context,
                object,
                index,
                request_bytes,
                &mut normalize,
            )),
            Err(error) => observations.push(DicomwebObservation {
                sop_instance_uid: object.sop_instance_uid.clone(),
                stow: false,
                qido: false,
                wado: false,
                semantic_equal: false,
                stow_request_bytes: None,
                message: format!("STOW failed: {error}"),
            }),
        }
    }
    Ok(DicomwebTransportResult { observations })
}

struct InstanceContext<'a> {
    agent: &'a ureq::Agent,
    endpoint: &'a str,
    retrieval_directory: &'a Path,
    limits: DicomwebLimits,
}

fn verify_instance(
    context: &InstanceContext<'_>,
    object: &DicomwebObject,
    index: usize,
    stow_request_bytes: u64,
    normalize: &mut impl FnMut(usize, &Path) -> Result<Value, String>,
) -> DicomwebObservation {
    let instance_root = format!(
        "{}/studies/{}/series/{}/instances",
        context.endpoint, object.study_instance_uid, object.series_instance_uid
    );
    let qido_url = format!("{instance_root}?SOPInstanceUID={}", object.sop_instance_uid);
    let qido = context
        .agent
        .get(&qido_url)
        .header("Accept", "application/dicom+json")
        .call()
        .map_err(|error| error.to_string())
        .and_then(|mut response| {
            require_content_type(&response, "application/dicom+json", "QIDO")?;
            response
                .body_mut()
                .with_config()
                .limit(context.limits.qido_response_bytes)
                .read_to_string()
                .map_err(|error| {
                    format!(
                        "QIDO response exceeded or could not be read under the {} byte limit: {error}",
                        context.limits.qido_response_bytes
                    )
                })
        })
        .and_then(|body| qido_contains(&body, &object.sop_instance_uid));
    match qido {
        Err(error) => {
            return failed_observation(
                object,
                Some(stow_request_bytes),
                true,
                false,
                false,
                format!("QIDO failed: {error}"),
            );
        }
        Ok(false) => {
            return failed_observation(
                object,
                Some(stow_request_bytes),
                true,
                false,
                false,
                "QIDO omitted instance".to_owned(),
            );
        }
        Ok(true) => {}
    }

    let wado_url = format!("{instance_root}/{}", object.sop_instance_uid);
    let retrieved = context
        .retrieval_directory
        .join(format!("{}-retrieved.dcm", object.sop_instance_uid));
    let wado = retrieve_dicom(
        context.agent,
        &wado_url,
        &retrieved,
        context.limits.wado_response_bytes,
    );
    if let Err(error) = wado {
        return failed_observation(
            object,
            Some(stow_request_bytes),
            true,
            true,
            false,
            format!("WADO failed: {error}"),
        );
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
            stow_request_bytes: Some(stow_request_bytes),
            message: String::new(),
        },
        Ok(false) => failed_observation(
            object,
            Some(stow_request_bytes),
            true,
            true,
            true,
            "semantic content changed after WADO retrieval".to_owned(),
        ),
        Err(error) => failed_observation(
            object,
            Some(stow_request_bytes),
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

fn stow_instance(
    agent: &ureq::Agent,
    endpoint: &str,
    object: &DicomwebObject,
    response_limit: u64,
) -> Result<u64, String> {
    let file = File::open(&object.path).map_err(|error| {
        format!(
            "could not open STOW object {}: {error}",
            object.path.display()
        )
    })?;
    let object_bytes = file
        .metadata()
        .map_err(|error| {
            format!(
                "could not stat STOW object {}: {error}",
                object.path.display()
            )
        })?
        .len();
    let boundary = multipart_request_boundary();
    let header = format!("--{boundary}\r\nContent-Type: application/dicom\r\n\r\n").into_bytes();
    let trailer = format!("\r\n--{boundary}--\r\n").into_bytes();
    let request_bytes = u64::try_from(header.len())
        .ok()
        .and_then(|value| value.checked_add(object_bytes))
        .and_then(|value| value.checked_add(u64::try_from(trailer.len()).ok()?))
        .ok_or_else(|| "STOW request size overflows u64".to_owned())?;
    let reader = Cursor::new(header).chain(file).chain(Cursor::new(trailer));
    let mut response = agent
        .post(&format!("{endpoint}/studies"))
        .header(
            "Content-Type",
            &format!("multipart/related; type=\"application/dicom\"; boundary={boundary}"),
        )
        .header("Accept", "application/dicom+json")
        .send(ureq::SendBody::from_owned_reader(reader))
        .map_err(|error| error.to_string())?;
    require_content_type(&response, "application/dicom+json", "STOW")?;
    let body = response
        .body_mut()
        .with_config()
        .limit(response_limit)
        .read_to_vec()
        .map_err(|error| {
            format!(
                "STOW response exceeded or could not be read under the {response_limit} byte limit: {error}"
            )
        })?;
    parse_stow_response(&body, &object.sop_instance_uid)?;
    Ok(request_bytes)
}

fn multipart_request_boundary() -> String {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    let counter = BOUNDARY_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("wsi-interop-{}-{timestamp}-{counter}", std::process::id())
}

fn parse_stow_response(body: &[u8], sop_instance_uid: &str) -> Result<(), String> {
    let response: Value = serde_json::from_slice(body)
        .map_err(|error| format!("STOW returned invalid DICOM JSON: {error}"))?;
    let object = response
        .as_object()
        .ok_or_else(|| "STOW response must be a DICOM JSON object".to_owned())?;
    let successes = sequence_sop_uids(object.get("00081199"), "Referenced SOP Sequence")?;
    let failures = sequence_sop_uids(object.get("00081198"), "Failed SOP Sequence")?;
    if failures.iter().any(|uid| uid == sop_instance_uid) {
        return Err(format!(
            "STOW reported failure for SOP Instance UID {sop_instance_uid}"
        ));
    }
    if successes.as_slice() != [sop_instance_uid] || !failures.is_empty() {
        return Err(format!(
            "STOW did not report only one success for SOP Instance UID {sop_instance_uid}"
        ));
    }
    Ok(())
}

fn sequence_sop_uids(attribute: Option<&Value>, label: &str) -> Result<Vec<String>, String> {
    let Some(attribute) = attribute else {
        return Ok(Vec::new());
    };
    if attribute.get("vr").and_then(Value::as_str) != Some("SQ") {
        return Err(format!("STOW {label} must have SQ value representation"));
    }
    let values = attribute
        .get("Value")
        .and_then(Value::as_array)
        .ok_or_else(|| format!("STOW {label} must contain an array Value"))?;
    values
        .iter()
        .map(|item| {
            let uid = item
                .get("00081155")
                .ok_or_else(|| format!("STOW {label} item lacks Referenced SOP Instance UID"))?;
            if uid.get("vr").and_then(Value::as_str) != Some("UI") {
                return Err(format!(
                    "STOW {label} Referenced SOP Instance UID must have UI value representation"
                ));
            }
            let values = uid
                .get("Value")
                .and_then(Value::as_array)
                .filter(|values| values.len() == 1)
                .ok_or_else(|| format!("STOW {label} item lacks Referenced SOP Instance UID"))?;
            values[0]
                .as_str()
                .map(str::to_owned)
                .ok_or_else(|| format!("STOW {label} item lacks Referenced SOP Instance UID"))
        })
        .collect()
}

fn qido_contains(body: &str, sop_instance_uid: &str) -> Result<bool, String> {
    let results: Value = serde_json::from_str(body)
        .map_err(|error| format!("QIDO returned invalid JSON: {error}"))?;
    let Some(results) = results.as_array() else {
        return Err("QIDO response must be an array".to_owned());
    };
    let mut found = 0_usize;
    for (index, result) in results.iter().enumerate() {
        let attribute = result
            .get("00080018")
            .ok_or_else(|| format!("QIDO result at index {index} lacks SOP Instance UID Value"))?;
        if attribute.get("vr").and_then(Value::as_str) != Some("UI") {
            return Err(format!(
                "QIDO result at index {index} SOP Instance UID must have UI value representation"
            ));
        }
        let values = attribute
            .get("Value")
            .and_then(Value::as_array)
            .filter(|values| values.len() == 1)
            .ok_or_else(|| format!("QIDO result at index {index} lacks SOP Instance UID Value"))?;
        let uid = values[0]
            .as_str()
            .ok_or_else(|| format!("QIDO result at index {index} lacks SOP Instance UID Value"))?;
        if uid == sop_instance_uid {
            found += 1;
        }
    }
    Ok(found == 1)
}

fn retrieve_dicom(
    agent: &ureq::Agent,
    url: &str,
    output: &Path,
    response_limit: u64,
) -> Result<(), String> {
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
        .ok_or_else(|| "WADO response has no Content-Type".to_owned())?
        .to_owned();
    let parent = output
        .parent()
        .ok_or_else(|| format!("WADO output {} has no parent", output.display()))?;
    let mut raw = tempfile::NamedTempFile::new_in(parent)
        .map_err(|error| format!("could not stage WADO response: {error}"))?;
    let read_limit = response_limit
        .checked_add(1)
        .ok_or_else(|| "WADO response limit overflows u64".to_owned())?;
    let mut reader = response.body_mut().with_config().limit(read_limit).reader();
    let copied = std::io::copy(&mut reader, &mut raw).map_err(|error| {
        format!(
            "WADO response exceeded or could not be read under the {response_limit} byte limit: {error}"
        )
    })?;
    if copied > response_limit {
        return Err(format!(
            "WADO response exceeded the {response_limit} byte limit ({copied} bytes observed)"
        ));
    }
    raw.flush()
        .and_then(|()| raw.as_file().sync_all())
        .map_err(|error| format!("could not sync staged WADO response: {error}"))?;
    if media_type(&content_type).eq_ignore_ascii_case("application/dicom") {
        persist_noclobber(raw, output)
    } else if media_type(&content_type).eq_ignore_ascii_case("multipart/related") {
        let boundary = multipart_boundary(&content_type)?;
        raw.as_file_mut()
            .seek(SeekFrom::Start(0))
            .map_err(|error| format!("could not rewind staged WADO response: {error}"))?;
        let mut dicom = tempfile::NamedTempFile::new_in(parent)
            .map_err(|error| format!("could not stage multipart DICOM part: {error}"))?;
        extract_single_dicom_part(raw.as_file_mut(), &mut dicom, boundary.as_bytes())?;
        dicom
            .flush()
            .and_then(|()| dicom.as_file().sync_all())
            .map_err(|error| format!("could not sync staged DICOM part: {error}"))?;
        persist_noclobber(dicom, output)
    } else {
        Err(format!(
            "WADO returned unsupported Content-Type {content_type}"
        ))
    }
}

fn persist_noclobber(file: tempfile::NamedTempFile, output: &Path) -> Result<(), String> {
    file.persist_noclobber(output)
        .map(|_| ())
        .map_err(|error| format!("could not publish {}: {}", output.display(), error.error))
}

fn require_content_type(
    response: &ureq::http::Response<ureq::Body>,
    expected: &str,
    operation: &str,
) -> Result<(), String> {
    let actual = response
        .headers()
        .get("Content-Type")
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| format!("{operation} response has no Content-Type"))?;
    if media_type(actual).eq_ignore_ascii_case(expected) {
        Ok(())
    } else {
        Err(format!(
            "{operation} returned Content-Type {actual}, expected {expected}"
        ))
    }
}

fn failed_observation(
    object: &DicomwebObject,
    stow_request_bytes: Option<u64>,
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
        stow_request_bytes,
        message,
    }
}
