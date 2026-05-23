//! Multipart and form-urlencoded body validation tests.

use super::*;
use chasm_engine::validation::parse_request_body_for_validation;

/// `parse_request_body_for_validation` does not panic on malformed multipart
/// input and falls back to an empty JSON object envelope.
#[test]
fn test_multipart_parser_is_crash_safe() {
    let empty_boundary =
        parse_request_body_for_validation("multipart/form-data; boundary=", b"").expect("parsed");
    let binary_noise = parse_request_body_for_validation(
        "multipart/form-data; boundary=zzz",
        &[0xff, 0xfe, 0x00, 0x01],
    )
    .expect("parsed");
    let no_boundary_attr = parse_request_body_for_validation(
        "multipart/form-data",
        b"--zzz\r\nContent-Disposition: form-data; name=\"x\"\r\n\r\nbody",
    )
    .expect("parsed");
    let parsed = parse_request_body_for_validation(
        "multipart/form-data; boundary=ZZZ",
        b"--ZZZ\r\nContent-Disposition: form-data\r\n\r\nno-name\r\n--ZZZ--\r\n",
    )
    .expect("parsed");

    assert!(
        empty_boundary.as_object().is_some_and(|o| o.is_empty()),
        "empty-boundary malformed body must resolve to an empty object envelope",
    );
    assert!(
        binary_noise.as_object().is_some_and(|o| o.is_empty()),
        "binary-noise multipart body must resolve to an empty object envelope",
    );
    assert!(
        no_boundary_attr.as_object().is_some_and(|o| o.is_empty()),
        "missing-boundary parameter must resolve to an empty object envelope",
    );
    assert!(parsed.as_object().is_some());
}

/// A multipart body with all required parts present validates cleanly.
#[test]
fn test_multipart_with_required_parts_validates() {
    let yaml = r#"
openapi: 3.0.0
info: { title: t, version: 1.0.0 }
paths:
  /upload:
    post:
      requestBody:
        required: true
        content:
          multipart/form-data:
            schema:
              type: object
              required: [file, caption]
              properties:
                file:
                  type: string
                  format: binary
                caption:
                  type: string
                  minLength: 1
      responses:
        '200': { description: ok }
"#;
    let spec = load_spec(yaml).expect("spec");
    let op = op_for(&spec, "/upload", "POST");
    let body_bytes = b"--xx\r\nContent-Disposition: form-data; name=\"file\"; filename=\"a.bin\"\r\n\r\nBINARYDATA\r\n--xx\r\nContent-Disposition: form-data; name=\"caption\"\r\n\r\nhello\r\n--xx--\r\n";
    let body = parse_request_body_for_validation("multipart/form-data; boundary=xx", body_bytes)
        .expect("parsed");
    let mut headers = HashMap::new();
    headers.insert(
        "content-type".to_string(),
        "multipart/form-data; boundary=xx".to_string(),
    );
    let request = {
        let mut __r = MockRequest::default();
        __r.method = "POST".to_string();
        __r.path = "/upload".to_string();
        __r.headers = headers;
        __r.query = HashMap::new();
        __r.body = Some(body);
        __r
    };

    let errors = validate_request_full(&spec, Some("/upload"), op, &HashMap::new(), &request);

    assert!(errors.is_empty(), "got {errors:?}");
}

/// A binary multipart upload extracts field names without attempting to JSON-parse part bodies.
#[test]
fn test_multipart_binary_upload_validates() {
    let yaml = r#"
openapi: 3.0.0
info: { title: t, version: 1.0.0 }
paths:
  /file-upload:
    post:
      requestBody:
        required: true
        content:
          multipart/form-data:
            schema:
              type: object
              properties:
                file:
                  type: string
                  format: binary
              required: [file]
      responses:
        '201': { description: ok }
"#;
    let spec = load_spec(yaml).expect("spec");
    let op = op_for(&spec, "/file-upload", "POST");

    let boundary = "----xboundary";
    let mut raw: Vec<u8> = Vec::new();
    raw.extend_from_slice(format!("--{}\r\n", boundary).as_bytes());
    raw.extend_from_slice(
        b"Content-Disposition: form-data; name=\"file\"; filename=\"binary.dat\"\r\n",
    );
    raw.extend_from_slice(b"Content-Type: application/octet-stream\r\n\r\n");
    raw.extend_from_slice(&[0x00u8, 0xFF, 0x10, 0x7F, 0x80, 0xC3, 0x28, 0xFE]);
    raw.extend_from_slice(format!("\r\n--{}--\r\n", boundary).as_bytes());

    let ct = format!("multipart/form-data; boundary={}", boundary);
    let parsed = parse_request_body_for_validation(&ct, &raw)
        .expect("multipart binary upload must parse without crashing");
    let mut headers = HashMap::new();
    headers.insert("content-type".to_string(), ct);
    let request = {
        let mut __r = MockRequest::default();
        __r.method = "POST".to_string();
        __r.path = "/file-upload".to_string();
        __r.headers = headers;
        __r.query = HashMap::new();
        __r.body = Some(parsed);
        __r
    };

    let errors = validate_request_full(&spec, Some("/file-upload"), op, &HashMap::new(), &request);

    assert!(errors.is_empty(), "got {errors:?}");
}

/// A form-urlencoded scalar field is shaped into a JSON string keyed by the field name.
#[test]
fn test_form_urlencoded_helper_shapes_scalar_field() {
    let parsed = parse_request_body_for_validation(
        "application/x-www-form-urlencoded",
        b"name=alice&age=30&tag=a&tag=b",
    )
    .expect("parsed");

    let obj = parsed.as_object().expect("object");

    assert_eq!(obj.get("name").and_then(|v| v.as_str()), Some("alice"));
}

/// A repeated form-urlencoded key is shaped into a JSON array preserving every occurrence.
#[test]
fn test_form_urlencoded_helper_repeated_key_becomes_array() {
    let parsed = parse_request_body_for_validation(
        "application/x-www-form-urlencoded",
        b"name=alice&age=30&tag=a&tag=b",
    )
    .expect("parsed");

    let obj = parsed.as_object().expect("object");

    assert_eq!(
        obj.get("tag").and_then(|v| v.as_array()).map(|a| a.len()),
        Some(2)
    );
}

/// `parse_request_body_for_validation` extracts a multipart `name="file"` directive as an object key.
#[test]
fn test_multipart_helper_extracts_file_field_name() {
    let body = b"--boundary\r\nContent-Disposition: form-data; name=\"file\"; filename=\"a.txt\"\r\n\r\nbinary-bytes\r\n--boundary\r\nContent-Disposition: form-data; name=\"caption\"\r\n\r\nhello\r\n--boundary--\r\n";

    let parsed = parse_request_body_for_validation("multipart/form-data; boundary=boundary", body)
        .expect("parsed");

    assert!(parsed.as_object().expect("object").contains_key("file"));
}

/// `parse_request_body_for_validation` extracts a multipart `name="caption"` directive as an object key.
#[test]
fn test_multipart_helper_extracts_caption_field_name() {
    let body = b"--boundary\r\nContent-Disposition: form-data; name=\"file\"; filename=\"a.txt\"\r\n\r\nbinary-bytes\r\n--boundary\r\nContent-Disposition: form-data; name=\"caption\"\r\n\r\nhello\r\n--boundary--\r\n";

    let parsed = parse_request_body_for_validation("multipart/form-data; boundary=boundary", body)
        .expect("parsed");

    assert!(parsed.as_object().expect("object").contains_key("caption"));
}
