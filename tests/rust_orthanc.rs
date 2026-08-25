#![cfg(unix)]

use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::os::unix::fs::PermissionsExt;
use std::thread;

use serde_json::json;
use tempfile::tempdir;
use wsi_annotation_interop::orthanc::{
    DicomwebLimits, DicomwebObject, LocalOrthanc, orthanc_configuration, validate_loopback_url,
    verify_dicomweb_transport, verify_dicomweb_transport_with_limits,
};

#[test]
fn orthanc_configuration_is_loopback_only_and_uses_isolated_storage() {
    let directory = tempdir().unwrap();
    let configuration = orthanc_configuration(directory.path(), 8042, &[]);

    assert_eq!(configuration["RemoteAccessAllowed"], false);
    assert_eq!(configuration["AuthenticationEnabled"], false);
    assert_eq!(configuration["DicomServerEnabled"], false);
    assert!(configuration.get("DicomPort").is_none());
    assert!(configuration.get("DicomAet").is_none());
    assert_eq!(
        configuration["StorageDirectory"],
        directory.path().to_string_lossy().as_ref()
    );
    assert!(configuration.get("Plugins").is_none());
    validate_loopback_url("http://127.0.0.1:8042/dicom-web").unwrap();
    validate_loopback_url("http://[::1]:8042/dicom-web").unwrap();
    for unsafe_url in [
        "https://127.0.0.1:8042/dicom-web",
        "http://localhost:8042/dicom-web",
        "https://archive.example.org/dicom-web",
        "http://localhost.example.org/dicom-web",
        "http://user@127.0.0.1:8042/dicom-web",
        "http://[::1]archive.example.org/dicom-web",
        "file://127.0.0.1/tmp/archive",
    ] {
        assert!(
            validate_loopback_url(unsafe_url).is_err(),
            "accepted {unsafe_url}"
        );
    }
}

#[test]
fn local_orthanc_rejects_a_missing_user_supplied_binary() {
    let directory = tempdir().unwrap();
    let mut orthanc = LocalOrthanc::new(
        directory.path().join("not-installed"),
        Vec::new(),
        std::time::Duration::from_secs(1),
    )
    .unwrap();

    assert!(orthanc.start().unwrap_err().contains("not found"));
}

#[test]
fn local_orthanc_caps_stdout_and_stderr_without_blocking_the_child() {
    let directory = tempdir().unwrap();
    let executable = directory.path().join("synthetic-orthanc");
    fs::write(
        &executable,
        r#"#!/bin/sh
if [ "$1" = "--version" ]; then
  printf 'synthetic Orthanc 1.0'
  exit 0
fi
dd if=/dev/zero bs=1048576 count=5 2>/dev/null
dd if=/dev/zero bs=1048576 count=5 >&2 2>/dev/null
exit 9
"#,
    )
    .unwrap();
    let mut permissions = fs::metadata(&executable).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&executable, permissions).unwrap();
    let mut orthanc =
        LocalOrthanc::new(executable, Vec::new(), std::time::Duration::from_secs(3)).unwrap();

    let error = orthanc.start().unwrap_err();
    assert!(error.contains("exited"), "{error}");
    assert_eq!(
        orthanc.stdout().len(),
        4 * 1024 * 1024,
        "stderr bytes={}, error={error}",
        orthanc.stderr().len()
    );
    assert_eq!(orthanc.stderr().len(), 4 * 1024 * 1024);
    assert!(orthanc.stdout_truncated());
    assert!(orthanc.stderr_truncated());
}

#[test]
fn local_orthanc_retries_a_reported_port_collision() {
    let directory = tempdir().unwrap();
    let executable = directory.path().join("synthetic-orthanc");
    let launches = directory.path().join("launches");
    fs::write(
        &executable,
        format!(
            r#"#!/bin/sh
if [ "$1" = "--version" ]; then
  printf 'synthetic Orthanc 1.0'
  exit 0
fi
printf 'launch\n' >> '{}'
printf 'Address already in use\n' >&2
exit 9
"#,
            launches.display()
        ),
    )
    .unwrap();
    let mut permissions = fs::metadata(&executable).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&executable, permissions).unwrap();
    let mut orthanc =
        LocalOrthanc::new(executable, Vec::new(), std::time::Duration::from_secs(1)).unwrap();

    let error = orthanc.start().unwrap_err();

    assert!(error.contains("exited"), "{error}");
    assert_eq!(fs::read_to_string(launches).unwrap().lines().count(), 3);
}

#[test]
fn local_orthanc_retries_a_transient_executable_write_lock() {
    let directory = tempdir().unwrap();
    let executable = directory.path().join("synthetic-orthanc");
    fs::write(
        &executable,
        r#"#!/bin/sh
if [ "$1" = "--version" ]; then
  printf 'synthetic Orthanc 1.0'
  exit 0
fi
printf 'synthetic startup failure\n' >&2
exit 9
"#,
    )
    .unwrap();
    let mut permissions = fs::metadata(&executable).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&executable, permissions).unwrap();
    let writable_executable = OpenOptions::new().write(true).open(&executable).unwrap();
    let release_write_lock = thread::spawn(move || {
        thread::sleep(std::time::Duration::from_millis(25));
        drop(writable_executable);
    });
    let mut orthanc =
        LocalOrthanc::new(executable, Vec::new(), std::time::Duration::from_secs(1)).unwrap();

    let error = orthanc.start().unwrap_err();
    release_write_lock.join().unwrap();

    assert!(error.contains("exited"), "{error}");
    assert!(!error.contains("Text file busy"), "{error}");
}

#[test]
fn dicomweb_transport_verifies_stow_qido_wado_and_semantics() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let expected = b"synthetic-dicom".to_vec();
    let server_expected = expected.clone();
    let server = thread::spawn(move || {
        for request_number in 0..3 {
            let (mut stream, _) = listener.accept().unwrap();
            let (request, body) = read_request(&mut stream);
            match request_number {
                0 => {
                    assert!(request.starts_with("POST /dicom-web/studies "));
                    assert!(
                        body.windows(server_expected.len())
                            .any(|part| part == server_expected)
                    );
                    respond(
                        &mut stream,
                        "200 OK",
                        "application/dicom+json",
                        br#"{"00081199":{"vr":"SQ","Value":[{"00081155":{"vr":"UI","Value":["2.25.3"]}}]}}"#,
                    );
                }
                1 => {
                    assert!(
                        request
                            .starts_with("GET /dicom-web/studies/2.25.1/series/2.25.2/instances?")
                    );
                    respond(
                        &mut stream,
                        "200 OK",
                        "application/dicom+json",
                        br#"[{"00080018":{"vr":"UI","Value":["2.25.3"]}}]"#,
                    );
                }
                _ => {
                    assert!(request.starts_with(
                        "GET /dicom-web/studies/2.25.1/series/2.25.2/instances/2.25.3 "
                    ));
                    respond(&mut stream, "200 OK", "application/dicom", &server_expected);
                }
            }
        }
    });
    let directory = tempdir().unwrap();
    let input = directory.path().join("input.dcm");
    fs::write(&input, expected).unwrap();
    let object = DicomwebObject {
        path: input,
        study_instance_uid: "2.25.1".to_owned(),
        series_instance_uid: "2.25.2".to_owned(),
        sop_instance_uid: "2.25.3".to_owned(),
    };

    let result = verify_dicomweb_transport(
        &format!("http://{address}/dicom-web"),
        &[object],
        &directory.path().join("retrieved"),
        |_, path| Ok(json!(fs::read(path).map_err(|error| error.to_string())?)),
    )
    .unwrap();

    server.join().unwrap();
    assert!(result.is_ok());
    assert!(result.observations[0].stow);
    assert!(result.observations[0].qido);
    assert!(result.observations[0].wado);
    assert!(result.observations[0].semantic_equal);
}

#[test]
fn oversized_wado_response_is_not_published() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = thread::spawn(move || {
        for request_number in 0..3 {
            let (mut stream, _) = listener.accept().unwrap();
            let _ = read_request(&mut stream);
            match request_number {
                0 => respond(
                    &mut stream,
                    "200 OK",
                    "application/dicom+json",
                    br#"{"00081199":{"vr":"SQ","Value":[{"00081155":{"vr":"UI","Value":["2.25.3"]}}]}}"#,
                ),
                1 => respond(
                    &mut stream,
                    "200 OK",
                    "application/dicom+json",
                    br#"[{"00080018":{"vr":"UI","Value":["2.25.3"]}}]"#,
                ),
                _ => respond(
                    &mut stream,
                    "200 OK",
                    "application/dicom",
                    &[42; 64],
                ),
            }
        }
    });
    let directory = tempdir().unwrap();
    let input = directory.path().join("input.dcm");
    fs::write(&input, b"dicom").unwrap();
    let object = DicomwebObject {
        path: input,
        study_instance_uid: "2.25.1".to_owned(),
        series_instance_uid: "2.25.2".to_owned(),
        sop_instance_uid: "2.25.3".to_owned(),
    };
    let retrieved = directory.path().join("retrieved");

    let result = verify_dicomweb_transport_with_limits(
        &format!("http://{address}/dicom-web"),
        &[object],
        &retrieved,
        DicomwebLimits {
            stow_response_bytes: 1024,
            qido_response_bytes: 1024,
            wado_response_bytes: 16,
        },
        |_, path| Ok(json!(fs::read(path).map_err(|error| error.to_string())?)),
    )
    .unwrap();

    server.join().unwrap();
    assert!(!result.is_ok());
    assert!(result.observations[0].message.contains("limit"));
    assert!(!retrieved.join("2.25.3-retrieved.dcm").exists());
}

#[test]
fn stow_success_and_failure_are_accounted_per_instance() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = thread::spawn(move || {
        for request_number in 0..4 {
            let (mut stream, _) = listener.accept().unwrap();
            let (request, body) = read_request(&mut stream);
            match request_number {
                0 => {
                    assert!(request.starts_with("POST /dicom-web/studies "));
                    assert!(body.windows(5).any(|part| part == b"first"));
                    respond(
                        &mut stream,
                        "200 OK",
                        "application/dicom+json",
                        &stow_success("2.25.3"),
                    );
                }
                1 => respond(
                    &mut stream,
                    "200 OK",
                    "application/dicom+json",
                    br#"[{"00080018":{"vr":"UI","Value":["2.25.3"]}}]"#,
                ),
                2 => respond(&mut stream, "200 OK", "application/dicom", b"first"),
                _ => {
                    assert!(request.starts_with("POST /dicom-web/studies "));
                    assert!(body.windows(6).any(|part| part == b"second"));
                    respond(
                        &mut stream,
                        "200 OK",
                        "application/dicom+json",
                        br#"{"00081198":{"vr":"SQ","Value":[{"00081155":{"vr":"UI","Value":["2.25.4"]}}]}}"#,
                    );
                }
            }
        }
    });
    let directory = tempdir().unwrap();
    let mut objects = Vec::new();
    for (name, bytes, sop) in [
        ("first", b"first".as_slice(), "2.25.3"),
        ("second", b"second".as_slice(), "2.25.4"),
    ] {
        let path = directory.path().join(format!("{name}.dcm"));
        fs::write(&path, bytes).unwrap();
        objects.push(DicomwebObject {
            path,
            study_instance_uid: "2.25.1".to_owned(),
            series_instance_uid: "2.25.2".to_owned(),
            sop_instance_uid: sop.to_owned(),
        });
    }

    let result = verify_dicomweb_transport(
        &format!("http://{address}/dicom-web"),
        &objects,
        &directory.path().join("retrieved"),
        |_, path| Ok(json!(fs::read(path).map_err(|error| error.to_string())?)),
    )
    .unwrap();

    server.join().unwrap();
    assert_eq!(result.observations.len(), 2);
    assert!(result.observations[0].semantic_equal);
    assert!(result.observations[0].stow_request_bytes.unwrap() > 5);
    assert!(!result.observations[1].stow);
    assert!(result.observations[1].message.contains("reported failure"));
}

#[test]
fn multipart_wado_extracts_exactly_one_dicom_part() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let expected = b"multipart-dicom".to_vec();
    let server_expected = expected.clone();
    let server = thread::spawn(move || {
        for request_number in 0..3 {
            let (mut stream, _) = listener.accept().unwrap();
            let _ = read_request(&mut stream);
            match request_number {
                0 => respond(
                    &mut stream,
                    "200 OK",
                    "application/dicom+json",
                    &stow_success("2.25.3"),
                ),
                1 => respond(
                    &mut stream,
                    "200 OK",
                    "application/dicom+json",
                    br#"[{"00080018":{"vr":"UI","Value":["2.25.3"]}}]"#,
                ),
                _ => {
                    let mut body =
                        b"--dicom-boundary\r\nContent-Type: application/dicom\r\n\r\n".to_vec();
                    body.extend_from_slice(&server_expected);
                    body.extend_from_slice(b"\r\n--dicom-boundary--\r\n");
                    respond(
                        &mut stream,
                        "200 OK",
                        "multipart/related; type=\"application/dicom\"; boundary=dicom-boundary",
                        &body,
                    );
                }
            }
        }
    });
    let directory = tempdir().unwrap();
    let input = directory.path().join("input.dcm");
    fs::write(&input, expected).unwrap();
    let object = DicomwebObject {
        path: input,
        study_instance_uid: "2.25.1".to_owned(),
        series_instance_uid: "2.25.2".to_owned(),
        sop_instance_uid: "2.25.3".to_owned(),
    };

    let result = verify_dicomweb_transport(
        &format!("http://{address}/dicom-web"),
        &[object],
        &directory.path().join("retrieved"),
        |_, path| Ok(json!(fs::read(path).map_err(|error| error.to_string())?)),
    )
    .unwrap();

    server.join().unwrap();
    assert!(result.is_ok(), "{}", result.observations[0].message);
}

#[test]
fn malformed_stow_response_fails_only_that_instance() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let _ = read_request(&mut stream);
        respond(
            &mut stream,
            "200 OK",
            "application/dicom+json",
            br#"{"00081199":{"vr":"SQ","Value":[{}]}}"#,
        );
    });
    let directory = tempdir().unwrap();
    let object = single_object(directory.path(), b"dicom");

    let result = verify_dicomweb_transport(
        &format!("http://{address}/dicom-web"),
        &[object],
        &directory.path().join("retrieved"),
        |_, _| Ok(json!(null)),
    )
    .unwrap();

    server.join().unwrap();
    assert!(!result.observations[0].stow);
    assert!(
        result.observations[0]
            .message
            .contains("lacks Referenced SOP Instance UID")
    );
}

#[test]
fn oversized_qido_response_stops_before_wado() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = thread::spawn(move || {
        for request_number in 0..2 {
            let (mut stream, _) = listener.accept().unwrap();
            let _ = read_request(&mut stream);
            if request_number == 0 {
                respond(
                    &mut stream,
                    "200 OK",
                    "application/dicom+json",
                    &stow_success("2.25.3"),
                );
            } else {
                respond(
                    &mut stream,
                    "200 OK",
                    "application/dicom+json",
                    &[b' '; 128],
                );
            }
        }
    });
    let directory = tempdir().unwrap();
    let object = single_object(directory.path(), b"dicom");

    let result = verify_dicomweb_transport_with_limits(
        &format!("http://{address}/dicom-web"),
        &[object],
        &directory.path().join("retrieved"),
        DicomwebLimits {
            stow_response_bytes: 1024,
            qido_response_bytes: 16,
            wado_response_bytes: 1024,
        },
        |_, _| Ok(json!(null)),
    )
    .unwrap();

    server.join().unwrap();
    assert!(result.observations[0].stow);
    assert!(!result.observations[0].qido);
    assert!(result.observations[0].message.contains("limit"));
}

#[test]
fn malformed_multipart_wado_is_not_published() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = thread::spawn(move || {
        for request_number in 0..3 {
            let (mut stream, _) = listener.accept().unwrap();
            let _ = read_request(&mut stream);
            match request_number {
                0 => respond(
                    &mut stream,
                    "200 OK",
                    "application/dicom+json",
                    &stow_success("2.25.3"),
                ),
                1 => respond(
                    &mut stream,
                    "200 OK",
                    "application/dicom+json",
                    br#"[{"00080018":{"vr":"UI","Value":["2.25.3"]}}]"#,
                ),
                _ => respond(
                    &mut stream,
                    "200 OK",
                    "multipart/related; boundary=declared-boundary",
                    b"--different-boundary\r\nContent-Type: application/dicom\r\n\r\ndicom\r\n--different-boundary--\r\n",
                ),
            }
        }
    });
    let directory = tempdir().unwrap();
    let object = single_object(directory.path(), b"dicom");
    let retrieved = directory.path().join("retrieved");

    let result = verify_dicomweb_transport(
        &format!("http://{address}/dicom-web"),
        &[object],
        &retrieved,
        |_, _| Ok(json!(null)),
    )
    .unwrap();

    server.join().unwrap();
    assert!(!result.observations[0].wado);
    assert!(result.observations[0].message.contains("initial boundary"));
    assert!(!retrieved.join("2.25.3-retrieved.dcm").exists());
}

#[test]
fn multipart_wado_without_a_dicom_part_is_not_published() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = thread::spawn(move || {
        for request_number in 0..3 {
            let (mut stream, _) = listener.accept().unwrap();
            let _ = read_request(&mut stream);
            match request_number {
                0 => respond(
                    &mut stream,
                    "200 OK",
                    "application/dicom+json",
                    &stow_success("2.25.3"),
                ),
                1 => respond(
                    &mut stream,
                    "200 OK",
                    "application/dicom+json",
                    br#"[{"00080018":{"vr":"UI","Value":["2.25.3"]}}]"#,
                ),
                _ => respond(
                    &mut stream,
                    "200 OK",
                    "multipart/related; boundary=dicom-boundary",
                    b"--dicom-boundary\r\nContent-Type: text/plain\r\n\r\nnot dicom\r\n--dicom-boundary--\r\n",
                ),
            }
        }
    });
    let directory = tempdir().unwrap();
    let object = single_object(directory.path(), b"dicom");
    let retrieved = directory.path().join("retrieved");

    let result = verify_dicomweb_transport(
        &format!("http://{address}/dicom-web"),
        &[object],
        &retrieved,
        |_, _| Ok(json!(null)),
    )
    .unwrap();

    server.join().unwrap();
    assert!(!result.observations[0].wado);
    assert!(
        result.observations[0]
            .message
            .contains("no application/dicom")
    );
    assert!(!retrieved.join("2.25.3-retrieved.dcm").exists());
}

#[test]
#[ignore = "streaming calibration; run in release mode with /usr/bin/time -l"]
fn dicomweb_streaming_calibration() {
    for size_mib in [1_u64, 8, 32] {
        let size = size_mib * 1024 * 1024;
        let mut samples = Vec::new();
        for repetition in 0..4 {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let address = listener.local_addr().unwrap();
            let server = thread::spawn(move || {
                for request_number in 0..3 {
                    let (mut stream, _) = listener.accept().unwrap();
                    if request_number == 0 {
                        assert!(discard_request_body(&mut stream) >= size);
                        respond(
                            &mut stream,
                            "200 OK",
                            "application/dicom+json",
                            &stow_success("2.25.3"),
                        );
                    } else {
                        let _ = read_request(&mut stream);
                        if request_number == 1 {
                            respond(
                                &mut stream,
                                "200 OK",
                                "application/dicom+json",
                                br#"[{"00080018":{"vr":"UI","Value":["2.25.3"]}}]"#,
                            );
                        } else {
                            respond_zero_dicom(&mut stream, size);
                        }
                    }
                }
            });
            let directory = tempdir().unwrap();
            let input = directory.path().join("input.dcm");
            fs::File::create(&input).unwrap().set_len(size).unwrap();
            let object = DicomwebObject {
                path: input,
                study_instance_uid: "2.25.1".to_owned(),
                series_instance_uid: "2.25.2".to_owned(),
                sop_instance_uid: "2.25.3".to_owned(),
            };
            let started = std::time::Instant::now();
            let result = verify_dicomweb_transport(
                &format!("http://{address}/dicom-web"),
                &[object],
                &directory.path().join("retrieved"),
                |_, path| {
                    Ok(json!(
                        fs::metadata(path).map_err(|error| error.to_string())?.len()
                    ))
                },
            )
            .unwrap();
            let elapsed = started.elapsed().as_secs_f64() * 1000.0;
            server.join().unwrap();
            assert!(result.is_ok(), "{}", result.observations[0].message);
            if repetition > 0 {
                samples.push(elapsed);
            }
        }
        samples.sort_by(f64::total_cmp);
        println!(
            "dicomweb size_mib={size_mib} median_ms={:.3} min_ms={:.3} max_ms={:.3}",
            samples[1], samples[0], samples[2]
        );
    }
}

fn single_object(directory: &std::path::Path, bytes: &[u8]) -> DicomwebObject {
    let input = directory.join("input.dcm");
    fs::write(&input, bytes).unwrap();
    DicomwebObject {
        path: input,
        study_instance_uid: "2.25.1".to_owned(),
        series_instance_uid: "2.25.2".to_owned(),
        sop_instance_uid: "2.25.3".to_owned(),
    }
}

fn stow_success(sop_instance_uid: &str) -> Vec<u8> {
    format!(
        r#"{{"00081199":{{"vr":"SQ","Value":[{{"00081155":{{"vr":"UI","Value":["{sop_instance_uid}"]}}}}]}}}}"#
    )
    .into_bytes()
}

fn read_request(stream: &mut TcpStream) -> (String, Vec<u8>) {
    let mut data = Vec::new();
    let mut buffer = [0_u8; 4096];
    let header_end = loop {
        let count = stream.read(&mut buffer).unwrap();
        assert!(count > 0);
        data.extend_from_slice(&buffer[..count]);
        if let Some(index) = data.windows(4).position(|part| part == b"\r\n\r\n") {
            break index + 4;
        }
    };
    let headers = String::from_utf8_lossy(&data[..header_end]).into_owned();
    let content_length = headers
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse::<usize>().unwrap())
        })
        .unwrap_or(0);
    if headers.lines().any(|line| {
        line.split_once(':').is_some_and(|(name, value)| {
            name.eq_ignore_ascii_case("transfer-encoding")
                && value.trim().eq_ignore_ascii_case("chunked")
        })
    }) {
        return (
            headers.lines().next().unwrap().to_owned(),
            read_chunked_body(stream, data[header_end..].to_vec()),
        );
    }
    while data.len() - header_end < content_length {
        let count = stream.read(&mut buffer).unwrap();
        assert!(count > 0);
        data.extend_from_slice(&buffer[..count]);
    }
    (
        headers.lines().next().unwrap().to_owned(),
        data[header_end..header_end + content_length].to_vec(),
    )
}

fn discard_request_body(stream: &mut TcpStream) -> u64 {
    let mut data = Vec::new();
    let mut buffer = [0_u8; 4096];
    let header_end = loop {
        let count = stream.read(&mut buffer).unwrap();
        assert!(count > 0);
        data.extend_from_slice(&buffer[..count]);
        if let Some(index) = data.windows(4).position(|part| part == b"\r\n\r\n") {
            break index + 4;
        }
    };
    let headers = String::from_utf8_lossy(&data[..header_end]);
    assert!(headers.lines().any(|line| {
        line.split_once(':').is_some_and(|(name, value)| {
            name.eq_ignore_ascii_case("transfer-encoding")
                && value.trim().eq_ignore_ascii_case("chunked")
        })
    }));
    discard_chunked_body(stream, data[header_end..].to_vec())
}

fn discard_chunked_body(stream: &mut TcpStream, mut pending: Vec<u8>) -> u64 {
    let mut total = 0_u64;
    loop {
        let line_end = loop {
            if let Some(index) = pending.windows(2).position(|part| part == b"\r\n") {
                break index;
            }
            read_more(stream, &mut pending);
        };
        let size = usize::from_str_radix(
            std::str::from_utf8(&pending[..line_end]).unwrap().trim(),
            16,
        )
        .unwrap();
        pending.drain(..line_end + 2);
        if size == 0 {
            while pending.len() < 2 {
                read_more(stream, &mut pending);
            }
            assert_eq!(&pending[..2], b"\r\n");
            return total;
        }
        while pending.len() < size + 2 {
            read_more(stream, &mut pending);
        }
        total += u64::try_from(size).unwrap();
        assert_eq!(&pending[size..size + 2], b"\r\n");
        pending.drain(..size + 2);
    }
}

fn read_chunked_body(stream: &mut TcpStream, mut pending: Vec<u8>) -> Vec<u8> {
    let mut body = Vec::new();
    loop {
        let line_end = loop {
            if let Some(index) = pending.windows(2).position(|part| part == b"\r\n") {
                break index;
            }
            read_more(stream, &mut pending);
        };
        let size = usize::from_str_radix(
            std::str::from_utf8(&pending[..line_end]).unwrap().trim(),
            16,
        )
        .unwrap();
        pending.drain(..line_end + 2);
        if size == 0 {
            while pending.len() < 2 {
                read_more(stream, &mut pending);
            }
            assert_eq!(&pending[..2], b"\r\n");
            return body;
        }
        while pending.len() < size + 2 {
            read_more(stream, &mut pending);
        }
        body.extend_from_slice(&pending[..size]);
        assert_eq!(&pending[size..size + 2], b"\r\n");
        pending.drain(..size + 2);
    }
}

fn read_more(stream: &mut TcpStream, pending: &mut Vec<u8>) {
    let mut buffer = [0_u8; 4096];
    let count = stream.read(&mut buffer).unwrap();
    assert!(count > 0);
    pending.extend_from_slice(&buffer[..count]);
}

fn respond(stream: &mut TcpStream, status: &str, content_type: &str, body: &[u8]) {
    write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )
    .unwrap();
    stream.write_all(body).unwrap();
}

fn respond_zero_dicom(stream: &mut TcpStream, bytes: u64) {
    write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: application/dicom\r\nContent-Length: {bytes}\r\nConnection: close\r\n\r\n"
    )
    .unwrap();
    let buffer = vec![0_u8; 64 * 1024].into_boxed_slice();
    let mut remaining = bytes;
    while remaining > 0 {
        let count = usize::try_from(remaining.min(buffer.len() as u64)).unwrap();
        stream.write_all(&buffer[..count]).unwrap();
        remaining -= u64::try_from(count).unwrap();
    }
}
