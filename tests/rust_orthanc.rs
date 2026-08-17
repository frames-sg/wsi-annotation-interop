#![cfg(unix)]

use std::fs;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::thread;

use serde_json::json;
use tempfile::tempdir;
use wsi_annotation_interop::orthanc::{
    DicomwebObject, LocalOrthanc, orthanc_configuration, validate_loopback_url,
    verify_dicomweb_transport,
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
                    respond(&mut stream, "200 OK", "application/dicom+json", b"{}");
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
                        br#"[{"00080018":{"Value":["2.25.3"]}}]"#,
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

fn respond(stream: &mut TcpStream, status: &str, content_type: &str, body: &[u8]) {
    write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )
    .unwrap();
    stream.write_all(body).unwrap();
}
