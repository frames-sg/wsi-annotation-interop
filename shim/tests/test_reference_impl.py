from __future__ import annotations

import hashlib
import json
from pathlib import Path
from types import SimpleNamespace

import highdicom as hd
import numpy as np
import pydicom
from wsi_reference.fixtures import build_scale_ann, generate_core_fixtures
from wsi_reference.highdicom_adapter import read_dataset
from wsi_reference.pm_normalize import normalize_pm, normalize_pm_dataset
from wsi_reference.pydcm_adapter import qualify_pydcm
from wsi_reference.sr_normalize import normalize_sr


def _digest(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def test_reference_fixtures_are_deterministic_non_phi_and_cover_required_forms(
    tmp_path: Path,
) -> None:
    first = generate_core_fixtures(tmp_path / "first")
    second = generate_core_fixtures(tmp_path / "second")

    assert set(first.ann) == {"2D_VOLUME", "2D_FRAME", "3D_COMMON_Z", "3D_XYZ"}
    assert set(first.seg) == {"BINARY", "LABELMAP", "FRACTIONAL"}
    first_paths = [
        first.source,
        first.pyramid_source,
        first.pyramid_ann,
        first.reordered_seg,
        first.pm,
        first.sr,
        first.sr_seg,
        *first.ann.values(),
        *first.seg.values(),
    ]
    second_paths = [
        second.source,
        second.pyramid_source,
        second.pyramid_ann,
        second.reordered_seg,
        second.pm,
        second.sr,
        second.sr_seg,
        *second.ann.values(),
        *second.seg.values(),
    ]
    assert [_digest(path) for path in first_paths] == [_digest(path) for path in second_paths]

    source = pydicom.dcmread(first.source, stop_before_pixels=True)
    assert source.PatientIdentityRemoved == "YES"
    assert source.PatientID == "SYNTHETIC-RESEARCH"
    assert source.PatientName == ""
    assert source.SpecimenLabelInImage == "NO"
    assert source.SpecimenDescriptionSequence[0].SpecimenIdentifier == "SYNTHETIC-SLIDE"

    for form, path in first.ann.items():
        annotation = read_dataset(pydicom.dcmread(path))
        assert isinstance(annotation, hd.ann.MicroscopyBulkSimpleAnnotations)
        assert {group.GraphicType for group in annotation.AnnotationGroupSequence} == {
            "POINT",
            "POLYLINE",
            "POLYGON",
            "ELLIPSE",
            "RECTANGLE",
        }
        expected_type = "3D" if form.startswith("3D") else "2D"
        assert annotation.AnnotationCoordinateType == expected_type

    for path in first.seg.values():
        assert isinstance(read_dataset(pydicom.dcmread(path)), hd.seg.Segmentation)

    assert isinstance(read_dataset(pydicom.dcmread(first.pm)), hd.pm.ParametricMap)
    assert isinstance(read_dataset(pydicom.dcmread(first.sr)), hd.sr.Comprehensive3DSR)
    assert isinstance(read_dataset(pydicom.dcmread(first.sr_seg)), hd.sr.Comprehensive3DSR)


def test_reference_normalizes_parametric_map_semantics(tmp_path: Path) -> None:
    fixtures = generate_core_fixtures(tmp_path / "fixtures")

    normalized = normalize_pm(fixtures.pm)

    assert normalized["dimension_organization_type"] == "TILED_FULL"
    assert normalized["matrix"] == {
        "columns": 4,
        "frames": 16,
        "rows": 4,
        "total_columns": 16,
        "total_rows": 16,
    }
    assert normalized["pixel"]["precision"] == "float32"
    assert normalized["pixel"]["finite_count"] == 256
    assert normalized["pixel"]["missing_count"] == 0
    assert normalized["pixel"]["padding_value"] is None
    assert normalized["mappings"][0]["quantity"]["value"] == "TUMOR"
    assert normalized["mappings"][0]["unit"]["value"] == "1"
    assert normalized["source_sop_instance_uids"] == ["2.25.100000000000000000000000000000003"]
    json.dumps(normalized, allow_nan=False)

    dataset = pydicom.dcmread(fixtures.pm)
    dataset.FloatPixelPaddingValue = float("nan")
    with_padding = normalize_pm_dataset(dataset)
    assert with_padding["pixel"]["padding_value"] == "NaN"
    json.dumps(with_padding, allow_nan=False)


def test_reference_normalizes_tid_1500_sr_semantics(tmp_path: Path) -> None:
    fixtures = generate_core_fixtures(tmp_path / "fixtures")

    normalized = normalize_sr(fixtures.sr)

    assert normalized["template_id"] == "1500"
    assert normalized["status"] == {
        "completion": "COMPLETE",
        "preliminary": "PRELIMINARY",
        "verification": "UNVERIFIED",
    }
    assert normalized["procedures_reported"][0]["value"] == "P5-09051"
    assert len(normalized["groups"]) == 1
    group = normalized["groups"][0]
    assert group["template_id"] == "1410"
    assert group["tracking"] == {
        "id": "2.25.71",
        "uid": "2.25.800000000000000000000000000000002",
    }
    assert group["reference"]["kind"] == "coordinates"
    assert group["reference"]["graphic_type"] == "POLYGON"
    assert group["reference"]["graphic_data"][0] == group["reference"]["graphic_data"][-1]
    assert group["measurements"][0]["value"] == 25.0
    assert group["qualitative_evaluations"][0]["value"]["value"] == "HIGH"

    segmented = normalize_sr(fixtures.sr_seg)
    reference = segmented["groups"][0]["reference"]
    assert reference["kind"] == "segmentation"
    assert reference["segment_numbers"] == [1]
    assert reference["frame_numbers"]


def test_reference_scale_fixture_has_the_requested_coordinate_count(tmp_path: Path) -> None:
    fixtures = generate_core_fixtures(tmp_path / "fixtures")
    path = build_scale_ann(fixtures.source, tmp_path / "scale.dcm", coordinate_values=1_000)

    group = pydicom.dcmread(path).AnnotationGroupSequence[0]
    assert len(group.PointCoordinatesData) // 4 == 1_000


def test_failed_pydcm_qualification_is_nonprimary() -> None:
    result = qualify_pydcm(module=SimpleNamespace(__version__="0.4.5"))

    assert not result.qualified
    assert result.primary_failure is False
    assert result.reasons


def test_pydcm_symbols_without_behavioral_fixtures_do_not_qualify() -> None:
    module = SimpleNamespace(
        __version__="0.4.5",
        read_ann=lambda path: path,
        write_ann=lambda value, path: None,
        read_seg=lambda path: path,
        write_seg=lambda value, path: None,
    )

    result = qualify_pydcm(module=module)

    assert not result.qualified
    assert result.reasons == ("behavioral WSI source and ANN/SEG fixtures were not supplied",)


def test_pydcm_qualification_uses_documented_submodule_writer_apis(tmp_path: Path) -> None:
    source = tmp_path / "source.dcm"
    ann_path = tmp_path / "input-ann.dcm"
    seg_path = tmp_path / "input-seg.dcm"
    ann_value: dict[str, object] = {
        "coordinate_type": "2D",
        "groups": [
            {
                "number": 1,
                "uid": "2.25.1",
                "label": "Tumor",
                "generation_type": "AUTOMATIC",
                "graphic_type": "POINT",
                "num_annotations": 1,
                "dimensionality": 2,
                "property_category": {
                    "value": "MORPH",
                    "scheme": "99TEST",
                    "meaning": "Morphology",
                },
                "property_type": {
                    "value": "TUMOR",
                    "scheme": "99TEST",
                    "meaning": "Tumor",
                },
                "annotations": [np.array([[1.0, 2.0]])],
                "measurements": [],
            }
        ],
    }
    labelmap = np.array([[[0, 1], [0, 0]]], dtype=np.uint16)
    seg_metadata: dict[str, object] = {
        "segments": [
            {
                "number": 1,
                "label": "Tumor",
                "rgb": [255, 0, 0],
                "category": {
                    "value": "MORPH",
                    "scheme": "99TEST",
                    "meaning": "Morphology",
                },
                "type": {
                    "value": "TUMOR",
                    "scheme": "99TEST",
                    "meaning": "Tumor",
                },
                "anatomic": {
                    "value": "TISSUE",
                    "scheme": "99TEST",
                    "meaning": "Tissue",
                },
            }
        ]
    }
    written_ann: dict[str, dict[str, object]] = {}
    written_seg: dict[str, tuple[np.ndarray, dict[str, object]]] = {}

    def read_ann(path: Path) -> dict[str, object]:
        return written_ann.get(str(path), ann_value)

    def write_ann(
        reference: Path,
        groups: list[dict[str, object]],
        *,
        coordinate_type: str,
        output: Path,
    ) -> None:
        assert reference == source
        assert groups[0]["property_category"] == ("MORPH", "99TEST", "Morphology")
        assert coordinate_type == "2D"
        Path(output).write_bytes(b"ann")
        written_ann[str(output)] = ann_value

    def read_seg(path: Path) -> tuple[np.ndarray, dict[str, object]]:
        return written_seg.get(str(path), (labelmap, seg_metadata))

    def write_seg(
        reference: Path,
        pixels: np.ndarray,
        segments: list[dict[str, object]],
        output: Path,
    ) -> None:
        assert reference == source
        assert np.array_equal(pixels, labelmap)
        assert segments[0]["labelID"] == 1
        Path(output).write_bytes(b"seg")
        written_seg[str(output)] = (labelmap, seg_metadata)

    module = SimpleNamespace(
        __version__="0.4.5",
        ann=SimpleNamespace(read_ann=read_ann, write_ann=write_ann),
        seg=SimpleNamespace(read_seg=read_seg, write_seg=write_seg),
    )

    result = qualify_pydcm(module=module, source=source, ann=ann_path, seg=seg_path)

    assert result.qualified
    assert result.capabilities == {
        "ann_read": True,
        "ann_write": True,
        "seg_read": True,
        "seg_write": True,
    }
    assert result.reasons == ()
