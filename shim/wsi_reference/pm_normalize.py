from __future__ import annotations

import hashlib
import json
import math
from pathlib import Path
from typing import Any

import numpy as np
import pydicom
from pydicom.dataset import Dataset

from .highdicom_adapter import read_dataset
from .normalize import _code, _content, _integers, _optional_string


def normalize_pm(path: str | Path) -> dict[str, Any]:
    """Normalize Parametric Map identity, mapping, frames, and pixel values."""
    return normalize_pm_dataset(pydicom.dcmread(path))


def normalize_pm_dataset(dataset: Dataset) -> dict[str, Any]:
    """Normalize an in-memory Parametric Map retrieved through DICOMweb."""
    parametric_map = read_dataset(dataset)
    values = np.asarray(parametric_map.pixel_array)
    precision = _pixel_precision(parametric_map)
    canonical = np.asarray(values, dtype="<f4" if precision == "float32" else "<f8").copy()
    missing = np.isnan(canonical)
    canonical[missing] = np.nan
    finite = canonical[np.isfinite(canonical)]
    digest = hashlib.sha256()
    digest.update(b"wsi-annotation-interop-pm-pixels-v1\0")
    digest.update(precision.encode("ascii"))
    digest.update(canonical.tobytes(order="C"))
    return {
        "sop_instance_uid": str(parametric_map.SOPInstanceUID),
        "series_instance_uid": str(parametric_map.SeriesInstanceUID),
        "study_instance_uid": str(parametric_map.StudyInstanceUID),
        "frame_of_reference_uid": _optional_string(parametric_map, "FrameOfReferenceUID"),
        "content": _content(parametric_map),
        "dimension_organization_type": _optional_string(
            parametric_map, "DimensionOrganizationType"
        ),
        "matrix": {
            "columns": int(parametric_map.Columns),
            "frames": int(getattr(parametric_map, "NumberOfFrames", 1)),
            "rows": int(parametric_map.Rows),
            "total_columns": int(parametric_map.TotalPixelMatrixColumns),
            "total_rows": int(parametric_map.TotalPixelMatrixRows),
        },
        "concatenation": {
            "uid": _optional_string(parametric_map, "ConcatenationUID"),
            "source_sop_instance_uid": _optional_string(
                parametric_map, "SOPInstanceUIDOfConcatenationSource"
            ),
            "number": _optional_integer(parametric_map, "InConcatenationNumber"),
            "total": _optional_integer(parametric_map, "InConcatenationTotalNumber"),
            "frame_offset": _optional_integer(parametric_map, "ConcatenationFrameOffsetNumber"),
        },
        "pixel": {
            "precision": precision,
            "sha256": digest.hexdigest(),
            "finite_count": int(finite.size),
            "missing_count": int(missing.sum()),
            "finite_minimum": None if finite.size == 0 else float(finite.min()),
            "finite_maximum": None if finite.size == 0 else float(finite.max()),
            "padding_value": _padding_value(parametric_map, precision),
        },
        "mappings": _real_world_value_mappings(parametric_map),
        "frames": _frames(parametric_map),
        "source_sop_instance_uids": _source_sop_instance_uids(parametric_map),
    }


def _pixel_precision(dataset: Dataset) -> str:
    if "FloatPixelData" in dataset:
        return "float32"
    if "DoubleFloatPixelData" in dataset:
        return "float64"
    raise ValueError("Parametric Map has neither Float nor Double Float Pixel Data")


def _padding_value(dataset: Dataset, precision: str) -> float | str | None:
    keyword = "FloatPixelPaddingValue" if precision == "float32" else "DoubleFloatPixelPaddingValue"
    value = dataset.get(keyword)
    if value is None:
        return None
    number = float(value)
    if math.isnan(number):
        return "NaN"
    if not math.isfinite(number):
        raise ValueError("Parametric Map pixel padding value must not be infinite")
    return number


def _real_world_value_mappings(dataset: Dataset) -> list[dict[str, Any]]:
    mappings: list[dict[str, Any]] = []
    seen: set[str] = set()
    for functional_group in _functional_groups(dataset):
        for item in getattr(functional_group, "RealWorldValueMappingSequence", []):
            normalized = _mapping(item)
            key = json.dumps(normalized, sort_keys=True, separators=(",", ":"))
            if key not in seen:
                seen.add(key)
                mappings.append(normalized)
    return mappings


def _mapping(item: Dataset) -> dict[str, Any]:
    quantity = None
    algorithm: dict[str, Any] = {}
    for definition in getattr(item, "QuantityDefinitionSequence", []):
        name_sequence = getattr(definition, "ConceptNameCodeSequence", [])
        if not name_sequence:
            continue
        name_value = _code(name_sequence[0])["value"]
        if name_value == "246205007" and getattr(definition, "ConceptCodeSequence", []):
            quantity = _code(definition.ConceptCodeSequence[0])
        elif name_value == "111000" and getattr(definition, "ConceptCodeSequence", []):
            algorithm["family"] = _code(definition.ConceptCodeSequence[0])
        elif name_value == "111001" and hasattr(definition, "TextValue"):
            algorithm["name"] = str(definition.TextValue)
        elif name_value == "111003" and hasattr(definition, "TextValue"):
            algorithm["version"] = str(definition.TextValue)
    units = getattr(item, "MeasurementUnitsCodeSequence", [])
    return {
        "label": _optional_string(item, "LUTLabel"),
        "explanation": _optional_string(item, "LUTExplanation"),
        "first_value": _optional_float(item, "DoubleFloatRealWorldValueFirstValueMapped"),
        "last_value": _optional_float(item, "DoubleFloatRealWorldValueLastValueMapped"),
        "slope": _optional_float(item, "RealWorldValueSlope"),
        "intercept": _optional_float(item, "RealWorldValueIntercept"),
        "quantity": quantity,
        "unit": None if not units else _code(units[0]),
        "algorithm": algorithm,
    }


def _functional_groups(dataset: Dataset) -> list[Dataset]:
    groups = list(getattr(dataset, "SharedFunctionalGroupsSequence", []))
    groups.extend(getattr(dataset, "PerFrameFunctionalGroupsSequence", []))
    return groups


def _frames(dataset: Dataset) -> list[dict[str, Any]]:
    frames = []
    for index, group in enumerate(getattr(dataset, "PerFrameFunctionalGroupsSequence", []), 1):
        content = getattr(group, "FrameContentSequence", [])
        position = getattr(group, "PlanePositionSlideSequence", [])
        frames.append(
            {
                "number": index,
                "dimension_index_values": (
                    []
                    if not content
                    else _integers(content[0].get("DimensionIndexValues"), default=[])
                ),
                "column_position": (
                    None
                    if not position
                    else _optional_integer(position[0], "ColumnPositionInTotalImagePixelMatrix")
                ),
                "row_position": (
                    None
                    if not position
                    else _optional_integer(position[0], "RowPositionInTotalImagePixelMatrix")
                ),
                "slide_position": (
                    None
                    if not position
                    else [
                        _optional_float(position[0], "XOffsetInSlideCoordinateSystem"),
                        _optional_float(position[0], "YOffsetInSlideCoordinateSystem"),
                        _optional_float(position[0], "ZOffsetInSlideCoordinateSystem"),
                    ]
                ),
            }
        )
    return frames


def _source_sop_instance_uids(dataset: Dataset) -> list[str]:
    values: list[str] = []
    for item in getattr(dataset, "SourceImageSequence", []):
        values.append(str(item.ReferencedSOPInstanceUID))
    for series in getattr(dataset, "ReferencedSeriesSequence", []):
        for item in getattr(series, "ReferencedInstanceSequence", []):
            values.append(str(item.ReferencedSOPInstanceUID))
    for group in _functional_groups(dataset):
        for derivation in getattr(group, "DerivationImageSequence", []):
            for item in getattr(derivation, "SourceImageSequence", []):
                values.append(str(item.ReferencedSOPInstanceUID))
    return list(dict.fromkeys(values))


def _optional_integer(dataset: Dataset, keyword: str) -> int | None:
    value = dataset.get(keyword)
    return None if value is None else int(value)


def _optional_float(dataset: Dataset, keyword: str) -> float | None:
    value = dataset.get(keyword)
    return None if value is None else float(value)
