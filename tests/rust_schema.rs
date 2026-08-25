use serde_json::Value;
use wsi_annotation_interop::schema::{
    RunManifestVersion, validate_compatible_run_manifest, validate_conversion_report,
    validate_matrix_observation, validate_pathology_mapping, validate_probe_report,
    validate_raster_profile, validate_run_manifest, validate_tiled_manifest,
};

fn reference_report() -> Value {
    serde_json::from_str(include_str!("data/ann-report.json")).unwrap()
}

#[test]
fn committed_v1_manifest_remains_readable_without_fabricated_provenance() {
    let legacy: Value = serde_json::from_str(include_str!(
        "../results/orthanc-pydcm-20260815-v1/manifest.json"
    ))
    .unwrap();

    assert_eq!(
        validate_compatible_run_manifest(&legacy).unwrap(),
        RunManifestVersion::V1Legacy
    );
    assert!(legacy.get("provenance").is_none());
}

#[test]
fn published_run_manifest_v2_validates_and_rejects_v1() {
    let directory = tempfile::tempdir().unwrap();
    let mut writer = wsi_annotation_interop::results::RunWriter::new(
        directory.path(),
        "schema-v2",
        serde_json::json!({"profile": "core"}),
    )
    .unwrap();
    writer
        .write_observations(&[serde_json::json!({"status": "passed"})])
        .unwrap();
    let path = writer.finalize().unwrap();
    let mut manifest: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    validate_run_manifest(&manifest).unwrap();

    manifest["schema_version"] = serde_json::json!(1);
    assert!(validate_run_manifest(&manifest).is_err());
}

#[test]
fn probe_schema_accepts_the_reference_report() {
    validate_probe_report(&reference_report()).unwrap();
}

#[test]
fn matrix_observation_v2_schema_preserves_phase_evidence() {
    let observation: Value =
        serde_json::from_str(include_str!("../examples/matrix-observation-v2.json")).unwrap();
    validate_matrix_observation(&observation).unwrap();

    let mut missing_phase_status = observation.clone();
    missing_phase_status["phases"][0]
        .as_object_mut()
        .unwrap()
        .remove("status");
    assert!(validate_matrix_observation(&missing_phase_status).is_err());

    let mut old_version = observation;
    old_version["schema_version"] = serde_json::json!(1);
    assert!(validate_matrix_observation(&old_version).is_err());
}

#[test]
fn probe_schema_rejects_runtime_inside_semantic_data() {
    let mut report = reference_report();
    report["semantic"]["runtime"] = serde_json::json!({"parse_ms": 1.0});

    let error = validate_probe_report(&report).unwrap_err();

    assert!(error.contains("probe report schema validation failed"));
    assert!(error.contains("semantic"));
}

#[test]
fn shipped_conversion_contract_examples_validate() {
    let cases = [
        (
            include_str!("../examples/conversion-report-v1.json"),
            validate_conversion_report as fn(&Value) -> Result<(), String>,
        ),
        (
            include_str!("../examples/pathology-mapping-v1.json"),
            validate_pathology_mapping,
        ),
        (
            include_str!("../examples/raster-profile-v1.json"),
            validate_raster_profile,
        ),
        (
            include_str!("../examples/raster-profile-concatenation-v1.json"),
            validate_raster_profile,
        ),
        (
            include_str!("../examples/tiled-manifest-v1.json"),
            validate_tiled_manifest,
        ),
    ];
    for (document, validate) in cases {
        let value = serde_json::from_str(document).unwrap();
        validate(&value).unwrap();
    }
}

#[test]
fn conversion_schema_rejects_untracked_output_fields() {
    let mut report: Value =
        serde_json::from_str(include_str!("../examples/conversion-report-v1.json")).unwrap();
    report["outputs"][0]["untracked"] = serde_json::json!(true);

    let error = validate_conversion_report(&report).unwrap_err();

    assert!(error.contains("conversion report schema validation failed"));
    assert!(error.contains("untracked"));
}

#[test]
fn profile_schemas_enforce_scheme_designator_conditions_for_urn_codes() {
    let mut raster: Value =
        serde_json::from_str(include_str!("../examples/raster-profile-v1.json")).unwrap();
    raster["channels"][0]["quantity"] = serde_json::json!({
        "urn_code_value": "urn:example:pathology:probability",
        "code_meaning": "Tumor probability"
    });
    validate_raster_profile(&raster).unwrap();

    raster["channels"][0]["quantity"]["coding_scheme_version"] = serde_json::json!("2026");
    assert!(validate_raster_profile(&raster).is_err());

    let mut mapping: Value =
        serde_json::from_str(include_str!("../examples/pathology-mapping-v1.json")).unwrap();
    mapping["labels"]["viable_tumor"]["category"] = serde_json::json!({
        "code_value": "MORPH",
        "code_meaning": "Morphologically abnormal structure"
    });
    assert!(validate_pathology_mapping(&mapping).is_err());
}

#[test]
fn profile_schemas_reject_incomplete_enhanced_code_qualifiers() {
    for (document, code_pointer, validate) in [
        (
            serde_json::from_str::<Value>(include_str!("../examples/pathology-mapping-v1.json"))
                .unwrap(),
            "/labels/viable_tumor/category",
            validate_pathology_mapping as fn(&Value) -> Result<(), String>,
        ),
        (
            serde_json::from_str::<Value>(include_str!("../examples/raster-profile-v1.json"))
                .unwrap(),
            "/channels/0/quantity",
            validate_raster_profile as fn(&Value) -> Result<(), String>,
        ),
    ] {
        let mut incomplete_context = document.clone();
        incomplete_context.pointer_mut(code_pointer).unwrap()["context_identifier"] =
            serde_json::json!("1234");
        assert!(validate(&incomplete_context).is_err());

        let mut invalid_uid = document.clone();
        invalid_uid.pointer_mut(code_pointer).unwrap()["context_uid"] =
            serde_json::json!("not-a-uid");
        assert!(validate(&invalid_uid).is_err());

        let mut incomplete_extension = document;
        incomplete_extension.pointer_mut(code_pointer).unwrap()["context_group_extension"] =
            serde_json::json!(true);
        assert!(validate(&incomplete_extension).is_err());
    }
}

#[test]
fn conversion_schema_keeps_success_fields_out_of_error_reports() {
    let report = serde_json::json!({
        "schema": "conversion-report-v1",
        "schema_version": 1,
        "status": "error",
        "operation": "convert-raster",
        "implementation": {"name": "dicom-viewer-rust", "version": "0.1.0"},
        "losses": [],
        "error": {"code": "CONVERSION_FAILED", "message": "invalid raster"}
    });

    assert!(validate_conversion_report(&report).is_err());
}

#[test]
fn conversion_schema_requires_complete_concatenation_identity() {
    let mut report: Value =
        serde_json::from_str(include_str!("../examples/conversion-report-v1.json")).unwrap();
    report["operation"] = serde_json::json!("convert-raster");
    report["outputs"][0]["target"] = serde_json::json!("pm");
    report["outputs"][0]["frame_offset"] = serde_json::json!(0);
    report["outputs"][0]["frame_count"] = serde_json::json!(1);
    report["outputs"][0]["pixel_value_bytes"] = serde_json::json!(64);
    report["outputs"][0]["pixel_sha256"] =
        serde_json::json!("5555555555555555555555555555555555555555555555555555555555555555");
    report["outputs"][0]["concatenation_uid"] = serde_json::json!("2.25.200");
    report["target_coverage"] = serde_json::json!([{
        "target": "pm",
        "frame_count": 1,
        "channel_count": 1
    }]);

    assert!(validate_conversion_report(&report).is_err());
}

#[test]
fn tiled_manifest_schema_rejects_escaping_paths_and_incomplete_valid_region_crop() {
    let mut manifest: Value =
        serde_json::from_str(include_str!("../examples/tiled-manifest-v1.json")).unwrap();
    manifest["tiles"][0]["path"] = serde_json::json!("../outside.npy");
    assert!(validate_tiled_manifest(&manifest).is_err());

    manifest = serde_json::from_str(include_str!("../examples/tiled-manifest-v1.json")).unwrap();
    manifest["overlap_policy"] = serde_json::json!("valid-region-crop");
    assert!(validate_tiled_manifest(&manifest).is_err());
}
