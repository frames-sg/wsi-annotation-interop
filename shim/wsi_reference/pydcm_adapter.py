from __future__ import annotations

import importlib
import tempfile
from dataclasses import dataclass
from pathlib import Path
from typing import Any

_ENTRY_POINTS = {
    "ann_read": ("ann", "read_ann"),
    "ann_write": ("ann", "write_ann"),
    "seg_read": ("seg", "read_seg"),
    "seg_write": ("seg", "write_seg"),
}


@dataclass(frozen=True)
class QualificationResult:
    implementation: str
    version: str | None
    qualified: bool
    primary_failure: bool
    capabilities: dict[str, bool]
    reasons: tuple[str, ...]


def qualify_pydcm(
    module: Any | None = None,
    *,
    source: Path | None = None,
    ann: Path | None = None,
    seg: Path | None = None,
) -> QualificationResult:
    """Conservatively qualify pydcm; lack of support never fails a primary study arm."""
    if module is None:
        try:
            module = importlib.import_module("pydcm")
        except ImportError:
            return QualificationResult(
                implementation="pydcm",
                version=None,
                qualified=False,
                primary_failure=False,
                capabilities={},
                reasons=("pydcm is not installed in the selected environment",),
            )

    entry_points = _resolve_entry_points(module)
    capabilities: dict[str, bool] = {
        capability: capability in entry_points for capability in _ENTRY_POINTS
    }
    reasons = tuple(
        f"required {capability.replace('_', ' ')} probe entry point is unavailable"
        for capability, available in capabilities.items()
        if not available
    )
    if not reasons:
        if source is None or ann is None or seg is None:
            reasons = ("behavioral WSI source and ANN/SEG fixtures were not supplied",)
        else:
            capabilities, reasons = _run_behavioral_probes(entry_points, source, ann, seg)
    return QualificationResult(
        implementation="pydcm",
        version=getattr(module, "__version__", None),
        qualified=not reasons,
        primary_failure=False,
        capabilities=capabilities,
        reasons=reasons,
    )


def _resolve_entry_points(module: Any) -> dict[str, Any]:
    namespaces: dict[str, Any] = {}
    module_name = getattr(module, "__name__", None)
    for namespace, _ in _ENTRY_POINTS.values():
        owner = getattr(module, namespace, None)
        if owner is None and module_name:
            try:
                owner = importlib.import_module(f"{module_name}.{namespace}")
            except ImportError:
                owner = None
        namespaces[namespace] = owner

    resolved = {}
    for capability, (namespace, name) in _ENTRY_POINTS.items():
        function = getattr(namespaces[namespace], name, None)
        if not callable(function):
            function = getattr(module, name, None)
        if callable(function):
            resolved[capability] = function
    return resolved


def _run_behavioral_probes(
    entry_points: dict[str, Any],
    source: Path,
    ann: Path,
    seg: Path,
) -> tuple[dict[str, bool], tuple[str, ...]]:
    capabilities = dict.fromkeys(_ENTRY_POINTS, False)
    reasons: list[str] = []
    with tempfile.TemporaryDirectory(prefix="pydcm-qualification-") as directory:
        output_directory = Path(directory)
        try:
            _probe_ann(entry_points, source, ann, output_directory, capabilities)
        except Exception as error:  # qualification records third-party failures
            reasons.append(f"ANN behavioral probe failed: {error}")
        try:
            _probe_seg(entry_points, source, seg, output_directory, capabilities)
        except Exception as error:  # qualification records third-party failures
            reasons.append(f"SEG behavioral probe failed: {error}")
    return capabilities, tuple(reasons)


def _probe_ann(
    entry_points: dict[str, Any],
    source: Path,
    ann: Path,
    output_directory: Path,
    capabilities: dict[str, bool],
) -> None:
    value = entry_points["ann_read"](ann)
    expected = _ann_signature(value)
    capabilities["ann_read"] = True
    output = output_directory / "ann.dcm"
    entry_points["ann_write"](
        source,
        _writable_ann_groups(value["groups"]),
        coordinate_type=value["coordinate_type"],
        output=output,
    )
    _require_output(output)
    if _ann_signature(entry_points["ann_read"](output)) != expected:
        raise RuntimeError("ANN read/write/read changed annotation semantics")
    capabilities["ann_write"] = True


def _probe_seg(
    entry_points: dict[str, Any],
    source: Path,
    seg: Path,
    output_directory: Path,
    capabilities: dict[str, bool],
) -> None:
    import numpy as np

    value = entry_points["seg_read"](seg)
    if not isinstance(value, tuple) or len(value) != 2:
        raise RuntimeError("reader did not return a labelmap and metadata")
    labelmap, metadata = value
    labelmap = np.asarray(labelmap)
    if labelmap.size == 0 or not np.any(labelmap):
        raise RuntimeError("reader returned no foreground labels for a nonempty WSI SEG fixture")
    capabilities["seg_read"] = True
    output = output_directory / "seg.dcm"
    entry_points["seg_write"](source, labelmap, _writable_seg_segments(metadata), output)
    _require_output(output)
    rewritten, _ = entry_points["seg_read"](output)
    if not np.array_equal(np.asarray(rewritten), labelmap):
        raise RuntimeError("SEG read/write/read changed the labelmap")
    capabilities["seg_write"] = True


def _require_output(path: Path) -> None:
    if not path.is_file():
        raise RuntimeError("writer did not create its requested output")


def _code_tuple(code: Any) -> tuple[str, str, str]:
    if not isinstance(code, dict):
        raise RuntimeError("reader returned a malformed coded concept")
    try:
        return str(code["value"]), str(code["scheme"]), str(code["meaning"])
    except KeyError as error:
        raise RuntimeError("reader returned an incomplete coded concept") from error


def _writable_ann_groups(groups: Any) -> list[dict[str, Any]]:
    if not isinstance(groups, list) or not groups:
        raise RuntimeError("reader returned no annotation groups")
    writable = []
    for group in groups:
        converted = dict(group)
        converted.pop("dimensionality", None)
        converted.pop("num_annotations", None)
        for key in ("property_category", "property_type"):
            converted[key] = _code_tuple(converted[key])
        converted["measurements"] = [
            {
                **measurement,
                "name": _code_tuple(measurement["name"]),
                "unit": _code_tuple(measurement["unit"]),
            }
            for measurement in converted.get("measurements", [])
        ]
        writable.append(converted)
    return writable


def _writable_seg_segments(metadata: Any) -> list[dict[str, Any]]:
    if not isinstance(metadata, dict) or not metadata.get("segments"):
        raise RuntimeError("reader returned no segment descriptions")
    return [
        {
            "label": segment["label"],
            "labelID": int(segment["number"]),
            "rgb": tuple(segment["rgb"]),
            "category": _code_tuple(segment["category"]),
            "type": _code_tuple(segment["type"]),
            "anatomic": _code_tuple(segment["anatomic"]),
            "algorithm_type": "AUTOMATIC",
            "algorithm_name": "pydcm qualification probe",
        }
        for segment in metadata["segments"]
    ]


def _ann_signature(value: Any) -> tuple[Any, ...]:
    if not isinstance(value, dict) or not value.get("groups"):
        raise RuntimeError("reader returned no annotation groups")
    return (
        value.get("coordinate_type"),
        tuple(
            (
                group.get("number"),
                group.get("uid"),
                group.get("label"),
                group.get("generation_type"),
                group.get("graphic_type"),
                _code_tuple(group.get("property_category")),
                _code_tuple(group.get("property_type")),
                tuple(_array_values(annotation) for annotation in group["annotations"]),
                tuple(_measurement_signature(item) for item in group.get("measurements", [])),
            )
            for group in value["groups"]
        ),
    )


def _measurement_signature(measurement: dict[str, Any]) -> tuple[Any, ...]:
    return (
        _code_tuple(measurement["name"]),
        _code_tuple(measurement["unit"]),
        _array_values(measurement["values"]),
        tuple(measurement.get("annotation_index") or ()),
    )


def _array_values(value: Any) -> tuple[Any, ...]:
    import numpy as np

    return tuple(np.asarray(value).reshape(-1).tolist())
