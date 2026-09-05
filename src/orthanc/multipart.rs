use std::io::{BufRead, BufReader, Cursor, Read, Write};

const MAX_BOUNDARY_BYTES: usize = 70;
const MAX_MIME_HEADERS: usize = 64;
const MAX_MIME_LINE_BYTES: u64 = 8 * 1024;

pub(super) fn media_type(content_type: &str) -> &str {
    content_type.split(';').next().map_or("", str::trim)
}

pub(super) fn multipart_boundary(content_type: &str) -> Result<String, String> {
    let mut boundary = None;
    for part in mime_parts(content_type)? {
        let Some((name, value)) = part.split_once('=') else {
            continue;
        };
        if name.trim().eq_ignore_ascii_case("boundary") {
            if boundary.is_some() {
                return Err("multipart WADO response repeats its boundary parameter".to_owned());
            }
            boundary = Some(parameter_value(value.trim())?);
        }
    }
    let boundary = boundary
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "multipart WADO response has no boundary".to_owned())?;
    if boundary.len() > MAX_BOUNDARY_BYTES
        || boundary.ends_with(' ')
        || !boundary.bytes().all(is_boundary_byte)
    {
        return Err("multipart WADO boundary is invalid or exceeds the limit".to_owned());
    }
    Ok(boundary)
}

fn mime_parts(value: &str) -> Result<Vec<&str>, String> {
    let mut parts = Vec::new();
    let mut start = 0;
    let mut quoted = false;
    let mut escaped = false;
    for (index, byte) in value.bytes().enumerate() {
        if escaped {
            escaped = false;
        } else if quoted && byte == b'\\' {
            escaped = true;
        } else if byte == b'"' {
            quoted = !quoted;
        } else if byte == b';' && !quoted {
            parts.push(value[start..index].trim());
            start = index + 1;
        }
    }
    if quoted || escaped {
        return Err("multipart WADO Content-Type has an unterminated quoted parameter".to_owned());
    }
    parts.push(value[start..].trim());
    Ok(parts)
}

fn parameter_value(value: &str) -> Result<String, String> {
    if !value.starts_with('"') {
        return Ok(value.to_owned());
    }
    let inner = value
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
        .ok_or_else(|| "multipart WADO parameter has malformed quotes".to_owned())?;
    let mut result = String::with_capacity(inner.len());
    let mut escaped = false;
    for character in inner.chars() {
        if escaped {
            result.push(character);
            escaped = false;
        } else if character == '\\' {
            escaped = true;
        } else {
            result.push(character);
        }
    }
    if escaped {
        return Err("multipart WADO parameter ends with an escape".to_owned());
    }
    Ok(result)
}

const fn is_boundary_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric()
        || matches!(
            byte,
            b'\''
                | b'('
                | b')'
                | b'+'
                | b'_'
                | b','
                | b'-'
                | b'.'
                | b'/'
                | b':'
                | b'='
                | b'?'
                | b' '
        )
}

pub(super) fn extract_single_dicom_part(
    input: &mut impl Read,
    output: &mut impl Write,
    boundary: &[u8],
) -> Result<(), String> {
    let mut reader = BufReader::new(input);
    let mut expected_start = b"--".to_vec();
    expected_start.extend_from_slice(boundary);
    expected_start.extend_from_slice(b"\r\n");
    let start = read_mime_line(&mut reader)?;
    if start != expected_start {
        return Err("multipart WADO response has an invalid initial boundary".to_owned());
    }
    let mut content_type = None;
    for header_index in 0..=MAX_MIME_HEADERS {
        let line = read_mime_line(&mut reader)?;
        if line == b"\r\n" {
            break;
        }
        if header_index == MAX_MIME_HEADERS {
            return Err("multipart WADO part exceeds the header count limit".to_owned());
        }
        let text = std::str::from_utf8(line.strip_suffix(b"\r\n").unwrap_or(&line))
            .map_err(|_| "multipart WADO header is not UTF-8".to_owned())?;
        if text.starts_with([' ', '\t']) {
            return Err("multipart WADO folded headers are unsupported".to_owned());
        }
        let (name, value) = text
            .split_once(':')
            .ok_or_else(|| "multipart WADO header is malformed".to_owned())?;
        if name.is_empty()
            || !name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        {
            return Err("multipart WADO header name is invalid".to_owned());
        }
        if name.eq_ignore_ascii_case("Content-Type")
            && content_type.replace(value.trim().to_owned()).is_some()
        {
            return Err("multipart WADO part repeats Content-Type".to_owned());
        }
    }
    if !content_type
        .as_deref()
        .is_some_and(|value| media_type(value).eq_ignore_ascii_case("application/dicom"))
    {
        return Err("multipart WADO response contains no application/dicom part".to_owned());
    }
    let mut delimiter = b"\r\n--".to_vec();
    delimiter.extend_from_slice(boundary);
    let suffix = copy_until_delimiter(&mut reader, output, &delimiter)?;
    let mut suffix_reader = Cursor::new(suffix).chain(reader);
    let mut closing = [0_u8; 2];
    suffix_reader
        .read_exact(&mut closing)
        .map_err(|_| "multipart WADO closing boundary is truncated".to_owned())?;
    if closing != *b"--" {
        return Err("multipart WADO response contains multiple or ambiguous parts".to_owned());
    }
    let mut tail = [0_u8; 3];
    let count = suffix_reader
        .read(&mut tail)
        .map_err(|error| format!("could not read multipart WADO suffix: {error}"))?;
    if count != 0 && (count != 2 || tail[..2] != *b"\r\n") {
        return Err("multipart WADO closing boundary has invalid trailing data".to_owned());
    }
    if suffix_reader
        .read(&mut tail[..1])
        .map_err(|error| format!("could not finish multipart WADO suffix: {error}"))?
        != 0
    {
        return Err("multipart WADO response has data after its closing boundary".to_owned());
    }
    Ok(())
}

fn read_mime_line(reader: &mut impl BufRead) -> Result<Vec<u8>, String> {
    let mut line = Vec::new();
    let count = reader
        .take(MAX_MIME_LINE_BYTES + 1)
        .read_until(b'\n', &mut line)
        .map_err(|error| format!("could not read multipart WADO header: {error}"))?;
    if count == 0 {
        return Err("multipart WADO headers are truncated".to_owned());
    }
    if u64::try_from(count).unwrap_or(u64::MAX) > MAX_MIME_LINE_BYTES || !line.ends_with(b"\r\n") {
        return Err("multipart WADO header line exceeds the limit or lacks CRLF".to_owned());
    }
    Ok(line)
}

fn copy_until_delimiter(
    reader: &mut impl Read,
    output: &mut impl Write,
    delimiter: &[u8],
) -> Result<Vec<u8>, String> {
    let mut pending = Vec::new();
    let mut buffer = vec![0_u8; 64 * 1024].into_boxed_slice();
    loop {
        let mut incomplete_delimiter = None;
        let mut valid_delimiter = None;
        for (index, part) in pending.windows(delimiter.len()).enumerate() {
            if part != delimiter {
                continue;
            }
            let suffix = index + delimiter.len();
            if pending.len() < suffix + 2 {
                incomplete_delimiter = Some(index);
                break;
            }
            if matches!(&pending[suffix..suffix + 2], b"--" | b"\r\n") {
                valid_delimiter = Some(index);
                break;
            }
        }
        if let Some(index) = valid_delimiter {
            output
                .write_all(&pending[..index])
                .map_err(|error| format!("could not write multipart DICOM part: {error}"))?;
            return Ok(pending[index + delimiter.len()..].to_vec());
        }
        let retained = delimiter.len().saturating_add(1);
        let writable =
            incomplete_delimiter.unwrap_or_else(|| pending.len().saturating_sub(retained));
        if writable > 0 {
            output
                .write_all(&pending[..writable])
                .map_err(|error| format!("could not write multipart DICOM part: {error}"))?;
            pending.drain(..writable);
        }
        let count = reader
            .read(&mut buffer)
            .map_err(|error| format!("could not read multipart DICOM part: {error}"))?;
        if count == 0 {
            return Err("multipart WADO part has no closing boundary".to_owned());
        }
        pending.extend_from_slice(&buffer[..count]);
    }
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::{MAX_MIME_HEADERS, extract_single_dicom_part, multipart_boundary};

    #[test]
    fn quoted_parameters_do_not_confuse_boundary_selection() {
        let content_type = concat!(
            "multipart/related; type=\"application/dicom; transfer-syntax=1.2.3\"; ",
            "boundary=\"dicom-boundary\""
        );
        assert_eq!(multipart_boundary(content_type).unwrap(), "dicom-boundary");
    }

    #[test]
    fn boundary_parameters_reject_duplicates_controls_and_excessive_length() {
        for content_type in [
            "multipart/related; boundary=one; boundary=two".to_owned(),
            "multipart/related; boundary=\"bad\r\nboundary\"".to_owned(),
            format!("multipart/related; boundary={}", "a".repeat(71)),
            "multipart/related; boundary=\"unterminated".to_owned(),
        ] {
            assert!(multipart_boundary(&content_type).is_err(), "{content_type}");
        }
    }

    #[test]
    fn part_headers_reject_duplicates_folding_and_count_or_line_overflow() {
        let boundary = b"boundary";
        let duplicate = concat!(
            "--boundary\r\n",
            "Content-Type: application/dicom\r\n",
            "Content-Type: application/dicom\r\n\r\n",
            "dicom\r\n--boundary--\r\n"
        );
        let folded = concat!(
            "--boundary\r\n",
            " Content-Type: application/dicom\r\n\r\n",
            "dicom\r\n--boundary--\r\n"
        );
        for payload in [duplicate.as_bytes(), folded.as_bytes()] {
            assert!(
                extract_single_dicom_part(&mut Cursor::new(payload), &mut Vec::new(), boundary)
                    .is_err()
            );
        }

        let mut excessive_count = b"--boundary\r\n".to_vec();
        for _ in 0..=MAX_MIME_HEADERS {
            excessive_count.extend_from_slice(b"X-Test: value\r\n");
        }
        excessive_count.extend_from_slice(b"\r\ndicom\r\n--boundary--\r\n");
        assert!(
            extract_single_dicom_part(
                &mut Cursor::new(excessive_count),
                &mut Vec::new(),
                boundary,
            )
            .unwrap_err()
            .contains("header count")
        );

        let oversized_line = format!(
            "--boundary\r\nX-Test: {}\r\n\r\ndicom\r\n--boundary--\r\n",
            "x".repeat(8192)
        );
        assert!(
            extract_single_dicom_part(&mut Cursor::new(oversized_line), &mut Vec::new(), boundary,)
                .unwrap_err()
                .contains("line exceeds")
        );
    }

    #[test]
    fn extraction_preserves_binary_payload_with_delimiter_prefixes() {
        let payload = b"dicom\0\r\n--boundary-prefix";
        let mut body = b"--boundary\r\nContent-Type: application/dicom\r\n\r\n".to_vec();
        body.extend_from_slice(payload);
        body.extend_from_slice(b"\r\n--boundary--\r\n");
        let mut output = Vec::new();

        extract_single_dicom_part(&mut Cursor::new(body), &mut output, b"boundary").unwrap();

        assert_eq!(output, payload);
    }
}
