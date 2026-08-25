from __future__ import annotations

import hashlib
from collections.abc import Iterable, Sequence
from pathlib import Path
from typing import Any

import highdicom as hd
import numpy as np
import pydicom
from pydicom.dataset import Dataset

from .highdicom_adapter import read_dataset


def normalize_ann(
    path: str | Path,
    source_path: str | Path,
    *,
    canonical_source_path: str | Path | None = None,
) -> dict[str, Any]:
    """Normalize ANN semantics independently through highdicom and pydicom."""
    annotation = pydicom.dcmread(path)
    source = pydicom.dcmread(source_path, stop_before_pixels=True)
    canonical_source = (
        source
        if canonical_source_path is None
        else pydicom.dcmread(canonical_source_path, stop_before_pixels=True)
    )
    return normalize_ann_dataset(annotation, source, canonical_source=canonical_source)


def normalize_ann_dataset(
    annotation_dataset: Dataset,
    source: Dataset,
    *,
    canonical_source: Dataset | None = None,
) -> dict[str, Any]:
    """Normalize an in-memory ANN result retrieved through DICOMweb."""
    annotation = read_dataset(annotation_dataset)
    canonical = source if canonical_source is None else canonical_source
    groups = [
        _annotation_group(
            group,
            annotation.AnnotationCoordinateType,
            annotation,
            source,
            canonical,
        )
        for group in annotation.get_annotation_groups()
    ]
    groups.sort(key=lambda group: group["uid"])
    referenced = annotation.ReferencedImageSequence[0]
    return {
        "sop_instance_uid": str(annotation.SOPInstanceUID),
        "series_instance_uid": str(annotation.SeriesInstanceUID),
        "coordinate_type": str(annotation.AnnotationCoordinateType),
        "pixel_origin_interpretation": _optional_string(annotation, "PixelOriginInterpretation"),
        "referenced_frame_number": _optional_int(referenced, "ReferencedFrameNumber"),
        "content": _content(annotation),
        "source": _source(source, canonical),
        "groups": groups,
    }


def normalize_seg(path: str | Path, source_path: str | Path) -> dict[str, Any]:
    """Normalize SEG semantics and row runs independently through highdicom."""
    segmentation = pydicom.dcmread(path)
    source = pydicom.dcmread(source_path, stop_before_pixels=True)
    return normalize_seg_dataset(segmentation, source)


def normalize_seg_dataset(
    segmentation_dataset: Dataset,
    source: Dataset,
) -> dict[str, Any]:
    """Normalize an in-memory SEG result retrieved through DICOMweb."""
    segmentation = read_dataset(segmentation_dataset)
    kind = str(segmentation.SegmentationType).lower()
    segments = [_segment(item) for item in segmentation.SegmentSequence]
    segments.sort(key=lambda segment: segment["number"])
    if kind == "fractional":
        masks = _fractional_masks(segmentation, source)
    else:
        masks = _binary_masks(segmentation, source, kind)
    return {
        "sop_instance_uid": str(segmentation.SOPInstanceUID),
        "series_instance_uid": str(segmentation.SeriesInstanceUID),
        "segmentation_kind": kind,
        "content": _content(segmentation),
        "source": _source(source, source),
        "segments": segments,
        "masks": masks,
    }


def normalize_wsi_source(dataset: Dataset) -> dict[str, Any]:
    """Normalize source identity and geometry used to interpret ANN/SEG objects."""
    pixel_data = bytes(getattr(dataset, "PixelData", b""))
    return {
        "source": _source(dataset, dataset),
        "number_of_frames": int(dataset.NumberOfFrames),
        "dimension_organization_type": _optional_string(dataset, "DimensionOrganizationType"),
        "optical_path_identifiers": [
            str(item.OpticalPathIdentifier) for item in getattr(dataset, "OpticalPathSequence", [])
        ],
        "pixel_data_sha256": hashlib.sha256(pixel_data).hexdigest(),
    }


def _annotation_group(
    group: hd.ann.AnnotationGroup,
    coordinate_type: str,
    annotation: hd.ann.MicroscopyBulkSimpleAnnotations,
    source: Dataset,
    canonical_source: Dataset,
) -> dict[str, Any]:
    graphic_data = group.get_graphic_data(coordinate_type)
    common_z = _numbers(group.get("CommonZCoordinateValue"))
    native_dimensions = 2 if coordinate_type == "2D" or common_z else 3
    native_arrays = [array[:, :native_dimensions] for array in graphic_data]
    native_coordinates = np.concatenate(native_arrays).reshape(-1).astype(float).tolist()
    canonical_coordinates = []
    for array in graphic_data:
        for point in array:
            if coordinate_type == "2D":
                x, y = _canonical_2d(
                    float(point[0]),
                    float(point[1]),
                    annotation,
                    source,
                    canonical_source,
                )
                canonical_coordinates.extend((x, y))
            else:
                x, y = _slide_to_pixel(
                    float(point[0]),
                    float(point[1]),
                    float(point[2]),
                    canonical_source,
                )
                canonical_coordinates.extend((x, y, float(point[2])))

    return {
        "uid": str(group.AnnotationGroupUID),
        "label": str(group.AnnotationGroupLabel),
        "description": _optional_string(group, "AnnotationGroupDescription") or "",
        "generation_type": str(group.AnnotationGroupGenerationType),
        "algorithms": _algorithms(group, "AnnotationGroupAlgorithmIdentificationSequence"),
        "category": _code(group.AnnotationPropertyCategoryCodeSequence[0]),
        "property_type": _code(group.AnnotationPropertyTypeCodeSequence[0]),
        "property_type_modifiers": _codes(group, "AnnotationPropertyTypeModifierCodeSequence"),
        "anatomic_regions": _codes(group, "AnatomicRegionSequence"),
        "primary_anatomic_structures": _codes(group, "PrimaryAnatomicStructureSequence"),
        "applies_to_all_optical_paths": (
            _optional_string(group, "AnnotationAppliesToAllOpticalPaths") != "NO"
        ),
        "referenced_optical_paths": _strings(group.get("ReferencedOpticalPathIdentifier")),
        "applies_to_all_z_planes": (
            _optional_string(group, "AnnotationAppliesToAllZPlanes") != "NO"
        ),
        "common_z_coordinates_mm": common_z,
        "recommended_display_cielab": _integers(
            group.get("RecommendedDisplayCIELabValue"), default=[0, 0, 0]
        ),
        "graphic_type": str(group.GraphicType),
        "annotation_count": int(group.NumberOfAnnotations),
        "measurements": _measurements(group),
        "geometry": {
            "mode": "Full",
            "native_dimensions": native_dimensions,
            "canonical_dimensions": 2 if coordinate_type == "2D" else 3,
            "native_coordinates": native_coordinates,
            "canonical_level0_coordinates": canonical_coordinates,
            "primitive_point_indices": _primitive_indices(group),
        },
    }


def _segment(item: Dataset) -> dict[str, Any]:
    number = int(item.SegmentNumber)
    return {
        "number": number,
        "label": str(item.SegmentLabel),
        "description": _optional_string(item, "SegmentDescription") or "",
        "generation_type": _optional_string(item, "SegmentAlgorithmType") or "MANUAL",
        "algorithms": (
            [] if number == 0 else _algorithms(item, "SegmentationAlgorithmIdentificationSequence")
        ),
        "category": _code(item.SegmentedPropertyCategoryCodeSequence[0]),
        "property_type": _code(item.SegmentedPropertyTypeCodeSequence[0]),
        "property_type_modifiers": _codes(item, "SegmentedPropertyTypeModifierCodeSequence"),
        "tracking_id": _optional_string(item, "TrackingID"),
        "tracking_uid": _optional_string(item, "TrackingUID"),
        "anatomic_regions": _codes(item, "AnatomicRegionSequence"),
        "primary_anatomic_structures": _codes(item, "PrimaryAnatomicStructureSequence"),
        "recommended_display_cielab": _integers(
            item.get("RecommendedDisplayCIELabValue"), default=[0, 0, 0]
        ),
    }


def _binary_masks(segmentation: hd.seg.Segmentation, source: Dataset, kind: str) -> dict[str, Any]:
    segment_numbers = segmentation.get_segment_numbers()
    matrix = segmentation.get_total_pixel_matrix(
        segment_numbers=segment_numbers,
        rescale_fractional=False,
    )
    runs = []
    for channel, segment_number in enumerate(segment_numbers):
        runs.extend(_binary_runs(matrix[:, :, channel] != 0, segment_number))
    digest = _mask_digest(source, kind, runs, fractional=False)
    return {"mode": "FullBinary", "sha256": digest, "runs": runs}


def _fractional_masks(segmentation: hd.seg.Segmentation, source: Dataset) -> dict[str, Any]:
    segment_numbers = segmentation.get_segment_numbers()
    matrix = segmentation.get_total_pixel_matrix(
        segment_numbers=segment_numbers,
        rescale_fractional=False,
    )
    maximum = int(segmentation.MaximumFractionalValue)
    runs = []
    for channel, segment_number in enumerate(segment_numbers):
        runs.extend(_fractional_runs(matrix[:, :, channel], segment_number, maximum))
    digest = _mask_digest(source, "fractional", runs, fractional=True)
    return {"mode": "FullFractional", "sha256": digest, "runs": runs}


def _binary_runs(mask: np.ndarray, segment_number: int) -> list[dict[str, int]]:
    runs = []
    for row, values in enumerate(mask):
        column = 0
        while column < len(values):
            if not values[column]:
                column += 1
                continue
            start = column
            while column < len(values) and values[column]:
                column += 1
            runs.append(
                {
                    "segment_number": int(segment_number),
                    "row": row,
                    "column_start": start,
                    "length": column - start,
                }
            )
    return runs


def _fractional_runs(mask: np.ndarray, segment_number: int, maximum: int) -> list[dict[str, Any]]:
    runs = []
    for row, values in enumerate(mask):
        column = 0
        while column < len(values):
            if values[column] == 0:
                column += 1
                continue
            start = column
            while column < len(values) and values[column] != 0:
                column += 1
            runs.append(
                {
                    "segment_number": int(segment_number),
                    "row": row,
                    "column_start": start,
                    "maximum_fractional_value": maximum,
                    "values": [int(value) for value in values[start:column]],
                }
            )
    return runs


def _mask_digest(
    source: Dataset,
    kind: str,
    runs: Sequence[dict[str, Any]],
    *,
    fractional: bool,
) -> str:
    digest = hashlib.sha256()
    digest.update(b"dicom-viewer-seg-runs-v1\0")
    digest.update(int(source.TotalPixelMatrixColumns).to_bytes(4, "little"))
    digest.update(int(source.TotalPixelMatrixRows).to_bytes(4, "little"))
    digest.update(bytes(({"binary": 0, "labelmap": 1, "fractional": 2}[kind],)))
    for run in runs:
        digest.update(int(run["segment_number"]).to_bytes(2, "little"))
        digest.update(int(run["row"]).to_bytes(4, "little"))
        digest.update(int(run["column_start"]).to_bytes(4, "little"))
        if fractional:
            values = run["values"]
            digest.update(int(run["maximum_fractional_value"]).to_bytes(2, "little"))
            digest.update(len(values).to_bytes(4, "little"))
            for value in values:
                digest.update(int(value).to_bytes(2, "little"))
        else:
            digest.update(int(run["length"]).to_bytes(4, "little"))
    return digest.hexdigest()


def _canonical_2d(
    x: float,
    y: float,
    annotation: hd.ann.MicroscopyBulkSimpleAnnotations,
    source: Dataset,
    canonical_source: Dataset,
) -> tuple[float, float]:
    if getattr(annotation, "PixelOriginInterpretation", None) == "FRAME":
        frame = int(annotation.ReferencedImageSequence[0].ReferencedFrameNumber)
        total_columns = int(source.TotalPixelMatrixColumns)
        tile_columns = int(source.Columns)
        tiles_across = (total_columns + tile_columns - 1) // tile_columns
        frame_index = frame - 1
        x += (frame_index % tiles_across) * tile_columns
        y += (frame_index // tiles_across) * int(source.Rows)
    slide = _pixel_to_slide(x, y, source)
    return _slide_to_pixel(*slide, canonical_source)


def _pixel_to_slide(x: float, y: float, source: Dataset) -> tuple[float, float, float]:
    origin_item = source.TotalPixelMatrixOriginSequence[0]
    origin = np.asarray(
        [
            float(origin_item.XOffsetInSlideCoordinateSystem),
            float(origin_item.YOffsetInSlideCoordinateSystem),
            float(getattr(origin_item, "ZOffsetInSlideCoordinateSystem", 0.0)),
        ]
    )
    orientation = np.asarray(source.ImageOrientationSlide, dtype=float)
    spacing = _pixel_spacing(source)
    slide = origin + orientation[:3] * x * spacing[1] + orientation[3:] * y * spacing[0]
    return float(slide[0]), float(slide[1]), float(slide[2])


def _slide_to_pixel(x: float, y: float, z: float, source: Dataset) -> tuple[float, float]:
    origin_item = source.TotalPixelMatrixOriginSequence[0]
    origin = np.asarray(
        [
            float(origin_item.XOffsetInSlideCoordinateSystem),
            float(origin_item.YOffsetInSlideCoordinateSystem),
            float(getattr(origin_item, "ZOffsetInSlideCoordinateSystem", 0.0)),
        ]
    )
    orientation = np.asarray(source.ImageOrientationSlide, dtype=float)
    column = orientation[:3]
    row = orientation[3:]
    basis = np.column_stack((column, row))
    distances = np.linalg.lstsq(basis, np.asarray([x, y, z]) - origin, rcond=None)[0]
    spacing = _pixel_spacing(source)
    return float(distances[0] / spacing[1]), float(distances[1] / spacing[0])


def _source(source: Dataset, canonical_source: Dataset) -> dict[str, Any]:
    spacing = _pixel_spacing(source)
    canonical_spacing = _pixel_spacing(canonical_source)
    return {
        "sop_class_uid": str(source.SOPClassUID),
        "sop_instance_uid": str(source.SOPInstanceUID),
        "series_instance_uid": str(source.SeriesInstanceUID),
        "study_instance_uid": str(source.StudyInstanceUID),
        "frame_of_reference_uid": _optional_string(source, "FrameOfReferenceUID"),
        "total_pixel_matrix_columns": int(source.TotalPixelMatrixColumns),
        "total_pixel_matrix_rows": int(source.TotalPixelMatrixRows),
        "tile_columns": int(source.Columns),
        "tile_rows": int(source.Rows),
        "pixel_spacing": spacing,
        "canonical_total_pixel_matrix_columns": int(canonical_source.TotalPixelMatrixColumns),
        "canonical_total_pixel_matrix_rows": int(canonical_source.TotalPixelMatrixRows),
        "canonical_pixel_spacing": canonical_spacing,
    }


def _pixel_spacing(source: Dataset) -> list[float]:
    if "PixelSpacing" in source:
        return [float(value) for value in source.PixelSpacing]
    measures = source.SharedFunctionalGroupsSequence[0].PixelMeasuresSequence[0]
    return [float(value) for value in measures.PixelSpacing]


def _content(dataset: Dataset) -> dict[str, str | None]:
    return {
        "label": str(dataset.ContentLabel),
        "description": _optional_string(dataset, "ContentDescription") or "",
        "creator_name": _optional_string(dataset, "ContentCreatorName"),
    }


def _code(item: Dataset) -> dict[str, Any]:
    if "CodeValue" in item:
        value = str(item.CodeValue)
        value_kind = "short"
    elif "LongCodeValue" in item:
        value = str(item.LongCodeValue)
        value_kind = "long"
    else:
        value = str(item.URNCodeValue)
        value_kind = "urn"
    extension = _optional_string(item, "ContextGroupExtensionFlag")
    return {
        "value": value,
        "value_kind": value_kind,
        "scheme": str(item.CodingSchemeDesignator),
        "coding_scheme_version": _optional_string(item, "CodingSchemeVersion"),
        "meaning": str(item.CodeMeaning),
        "context_identifier": _optional_string(item, "ContextIdentifier"),
        "context_uid": _optional_string(item, "ContextUID"),
        "mapping_resource": _optional_string(item, "MappingResource"),
        "mapping_resource_uid": _optional_string(item, "MappingResourceUID"),
        "context_group_version": _optional_string(item, "ContextGroupVersion"),
        "context_group_local_version": _optional_string(item, "ContextGroupLocalVersion"),
        "context_group_extension": None if extension is None else extension == "YES",
        "context_group_extension_creator_uid": _optional_string(
            item, "ContextGroupExtensionCreatorUID"
        ),
    }


def _codes(dataset: Dataset, keyword: str) -> list[dict[str, Any]]:
    return [_code(item) for item in getattr(dataset, keyword, [])]


def _algorithms(dataset: Dataset, keyword: str) -> list[dict[str, Any]]:
    algorithms = []
    for item in getattr(dataset, keyword, []):
        family = getattr(item, "AlgorithmFamilyCodeSequence", [])
        if (
            not family
            or not hasattr(item, "AlgorithmName")
            or not hasattr(item, "AlgorithmVersion")
        ):
            continue
        name_code = getattr(item, "AlgorithmNameCodeSequence", [])
        algorithms.append(
            {
                "family": _code(family[0]),
                "name_code": _code(name_code[0]) if name_code else None,
                "name": str(item.AlgorithmName),
                "version": str(item.AlgorithmVersion),
                "parameters": _optional_string(item, "AlgorithmParameters"),
                "source": _optional_string(item, "AlgorithmSource"),
            }
        )
    return algorithms


def _measurements(group: Dataset) -> list[dict[str, Any]]:
    measurements = []
    for measurement in getattr(group, "MeasurementsSequence", []):
        values_item = measurement.MeasurementValuesSequence[0]
        values = np.frombuffer(values_item.FloatingPointValues, dtype="<f4").astype(float).tolist()
        indices = None
        if hasattr(values_item, "AnnotationIndexList"):
            indices = (
                np.frombuffer(values_item.AnnotationIndexList, dtype="<i4").astype(int).tolist()
            )
        measurements.append(
            {
                "concept": _code(measurement.ConceptNameCodeSequence[0]),
                "units": _code(measurement.MeasurementUnitsCodeSequence[0]),
                "values": values,
                "annotation_indices": indices,
            }
        )
    return measurements


def _primitive_indices(group: Dataset) -> list[int]:
    value = group.get("LongPrimitivePointIndexList")
    if value is None:
        return []
    return np.frombuffer(value, dtype="<i4").astype(int).tolist()


def _optional_string(dataset: Dataset, keyword: str) -> str | None:
    value = dataset.get(keyword)
    if value is None or str(value) == "":
        return None
    return str(value)


def _optional_int(dataset: Dataset, keyword: str) -> int | None:
    value = dataset.get(keyword)
    return None if value is None else int(value)


def _numbers(value: Any) -> list[float]:
    if value is None:
        return []
    if isinstance(value, Iterable) and not isinstance(value, (str, bytes)):
        return [float(item) for item in value]
    return [float(value)]


def _integers(value: Any, *, default: list[int]) -> list[int]:
    if value is None:
        return default
    if isinstance(value, Iterable) and not isinstance(value, (str, bytes)):
        return [int(item) for item in value]
    return [int(value)]


def _strings(value: Any) -> list[str]:
    if value is None:
        return []
    if isinstance(value, Iterable) and not isinstance(value, (str, bytes)):
        return [str(item) for item in value]
    return [str(value)]
