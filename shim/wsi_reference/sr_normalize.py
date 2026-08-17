from __future__ import annotations

from pathlib import Path
from typing import Any

import highdicom as hd
import pydicom
from pydicom.dataset import Dataset

from .highdicom_adapter import read_dataset
from .normalize import _code, _integers, _optional_string

_ALGORITHM_CONCEPTS = {"111000", "111001", "111002", "111003", "122405"}
_NON_QUALITATIVE_CODES = {
    "276214006",
    "121071",
    "363698007",
    *_ALGORITHM_CONCEPTS,
}


def normalize_sr(path: str | Path) -> dict[str, Any]:
    """Normalize TID 1500 SR content independently through highdicom and pydicom."""
    return normalize_sr_dataset(pydicom.dcmread(path))


def normalize_sr_dataset(dataset: Dataset) -> dict[str, Any]:
    """Normalize an in-memory Comprehensive 3D SR retrieved through DICOMweb."""
    report = read_dataset(dataset)
    hd.sr.MeasurementReport.from_sequence([report], is_root=True, copy=False)
    root_content = list(getattr(report, "ContentSequence", []))
    imaging = _required_item(root_content, "126010", "Imaging Measurements")
    groups = [
        _measurement_group(item)
        for item in getattr(imaging, "ContentSequence", [])
        if _concept_value(item) == "125007"
    ]
    groups.sort(key=lambda item: item["tracking"]["uid"])
    return {
        "sop_instance_uid": str(report.SOPInstanceUID),
        "series_instance_uid": str(report.SeriesInstanceUID),
        "study_instance_uid": str(report.StudyInstanceUID),
        "frame_of_reference_uid": _optional_string(report, "FrameOfReferenceUID"),
        "template_id": _template_identifier(report),
        "title": _code(report.ConceptNameCodeSequence[0]),
        "status": {
            "completion": str(report.CompletionFlag),
            "verification": str(report.VerificationFlag),
            "preliminary": str(report.PreliminaryFlag),
        },
        "procedures_reported": [
            _code(item.ConceptCodeSequence[0])
            for item in root_content
            if _concept_value(item) == "121058"
        ],
        "device_observer_uids": [
            str(item.UID)
            for item in root_content
            if _concept_value(item) == "121012" and hasattr(item, "UID")
        ],
        "evidence_sop_instance_uids": _evidence_uids(report),
        "groups": groups,
    }


def _measurement_group(group: Dataset) -> dict[str, Any]:
    content = list(getattr(group, "ContentSequence", []))
    tracking_id = _required_value(content, "112039", "TextValue")
    tracking_uid = _required_value(content, "112040", "UID")
    measurements = [
        _measurement(item) for item in content if str(getattr(item, "ValueType", "")) == "NUM"
    ]
    qualitative = [
        {
            "concept": _code(item.ConceptNameCodeSequence[0]),
            "value": _code(item.ConceptCodeSequence[0]),
        }
        for item in content
        if str(getattr(item, "ValueType", "")) == "CODE"
        and _concept_value(item) not in _NON_QUALITATIVE_CODES
    ]
    return {
        "template_id": _template_identifier(group),
        "tracking": {"id": tracking_id, "uid": tracking_uid},
        "finding_category": _coded_value(content, "276214006"),
        "finding_type": _coded_value(content, "121071"),
        "finding_sites": [
            _code(item.ConceptCodeSequence[0])
            for item in content
            if _concept_value(item) == "363698007"
        ],
        "algorithm_identification": [
            _algorithm_item(item) for item in content if _concept_value(item) in _ALGORITHM_CONCEPTS
        ],
        "reference": _region_reference(content),
        "measurements": measurements,
        "qualitative_evaluations": qualitative,
    }


def _measurement(item: Dataset) -> dict[str, Any]:
    measured_values = list(getattr(item, "MeasuredValueSequence", []))
    if len(measured_values) != 1:
        raise ValueError("SR NUM content item must have exactly one Measured Value item")
    measured = measured_values[0]
    return {
        "concept": _code(item.ConceptNameCodeSequence[0]),
        "value": float(measured.NumericValue),
        "unit": _code(measured.MeasurementUnitsCodeSequence[0]),
        "coordinates": [
            _coordinates(child)
            for child in getattr(item, "ContentSequence", [])
            if str(getattr(child, "ValueType", "")) == "SCOORD3D"
        ],
    }


def _region_reference(content: list[Dataset]) -> dict[str, Any]:
    for item in content:
        if _concept_value(item) == "111030" and str(getattr(item, "ValueType", "")) == "SCOORD3D":
            return {"kind": "coordinates", **_coordinates(item)}
    for item in content:
        if _concept_value(item) == "121214" and str(getattr(item, "ValueType", "")) == "IMAGE":
            references = list(getattr(item, "ReferencedSOPSequence", []))
            if len(references) != 1:
                raise ValueError("SR segmentation reference must contain exactly one SOP reference")
            reference = references[0]
            return {
                "kind": "segmentation",
                "sop_class_uid": str(reference.ReferencedSOPClassUID),
                "sop_instance_uid": str(reference.ReferencedSOPInstanceUID),
                "frame_numbers": _integers(reference.get("ReferencedFrameNumber"), default=[]),
                "segment_numbers": _integers(reference.get("ReferencedSegmentNumber"), default=[]),
            }
    if any(
        str(getattr(child, "ValueType", "")) == "SCOORD3D"
        for item in content
        if str(getattr(item, "ValueType", "")) == "NUM"
        for child in getattr(item, "ContentSequence", [])
    ):
        return {"kind": "measurement_coordinates"}
    raise ValueError("SR measurement group has no lossless region reference")


def _coordinates(item: Dataset) -> dict[str, Any]:
    values = [float(value) for value in item.GraphicData]
    if not values or len(values) % 3:
        raise ValueError("SCOORD3D Graphic Data must contain coordinate triplets")
    return {
        "graphic_type": str(item.GraphicType),
        "graphic_data": [values[index : index + 3] for index in range(0, len(values), 3)],
        "frame_of_reference_uid": str(item.ReferencedFrameOfReferenceUID),
    }


def _algorithm_item(item: Dataset) -> dict[str, Any]:
    value_type = str(item.ValueType)
    if value_type == "CODE":
        value: Any = _code(item.ConceptCodeSequence[0])
    elif value_type == "TEXT":
        value = str(item.TextValue)
    else:
        raise ValueError(f"unexpected algorithm content value type {value_type}")
    return {
        "concept": _code(item.ConceptNameCodeSequence[0]),
        "value_type": value_type,
        "value": value,
    }


def _coded_value(content: list[Dataset], concept_value: str) -> dict[str, Any]:
    return _code(_required_item(content, concept_value, concept_value).ConceptCodeSequence[0])


def _required_value(content: list[Dataset], concept_value: str, keyword: str) -> str:
    item = _required_item(content, concept_value, concept_value)
    if not hasattr(item, keyword):
        raise ValueError(f"SR content item {concept_value} has no {keyword}")
    return str(getattr(item, keyword))


def _required_item(content: list[Dataset], concept_value: str, label: str) -> Dataset:
    for item in content:
        if _concept_value(item) == concept_value:
            return item
    raise ValueError(f"SR content has no {label} item")


def _concept_value(item: Dataset) -> str | None:
    sequence = getattr(item, "ConceptNameCodeSequence", [])
    return None if not sequence else str(_code(sequence[0])["value"])


def _template_identifier(item: Dataset) -> str | None:
    sequence = getattr(item, "ContentTemplateSequence", [])
    return None if not sequence else _optional_string(sequence[0], "TemplateIdentifier")


def _evidence_uids(report: Dataset) -> list[str]:
    values = []
    for study in getattr(report, "CurrentRequestedProcedureEvidenceSequence", []):
        for series in getattr(study, "ReferencedSeriesSequence", []):
            for item in getattr(series, "ReferencedSOPSequence", []):
                values.append(str(item.ReferencedSOPInstanceUID))
    return list(dict.fromkeys(values))
