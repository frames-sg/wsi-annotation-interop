from __future__ import annotations

import argparse
import importlib.metadata
import json
import platform
import sys
from dataclasses import asdict
from pathlib import Path
from typing import Any

import pydicom
from wsi_reference.fixtures import build_scale_ann, generate_core_fixtures
from wsi_reference.normalize import normalize_ann, normalize_seg, normalize_wsi_source
from wsi_reference.pm_normalize import normalize_pm
from wsi_reference.pydcm_adapter import qualify_pydcm
from wsi_reference.sr_normalize import normalize_sr


def main() -> int:
    arguments = _parser().parse_args()
    try:
        report = _run(arguments)
    except Exception as error:  # Rust records the isolated reference failure.
        print(json.dumps({"error": str(error)}, sort_keys=True))
        return 1
    print(json.dumps(report, sort_keys=True, separators=(",", ":"), allow_nan=False))
    return 0


def _run(arguments: argparse.Namespace) -> dict[str, Any]:
    if arguments.command == "generate-core":
        fixtures = generate_core_fixtures(arguments.output)
        return {
            "source": str(fixtures.source),
            "pyramid_source": str(fixtures.pyramid_source),
            "pyramid_ann": str(fixtures.pyramid_ann),
            "reordered_seg": str(fixtures.reordered_seg),
            "pm": str(fixtures.pm),
            "sr": str(fixtures.sr),
            "sr_seg": str(fixtures.sr_seg),
            "ann": {key: str(value) for key, value in fixtures.ann.items()},
            "seg": {key: str(value) for key, value in fixtures.seg.items()},
        }
    if arguments.command == "normalize-ann":
        return normalize_ann(
            arguments.annotation,
            arguments.source,
            canonical_source_path=arguments.canonical_source,
        )
    if arguments.command == "normalize-seg":
        return normalize_seg(arguments.annotation, arguments.source)
    if arguments.command == "normalize-pm":
        return normalize_pm(arguments.dicom)
    if arguments.command == "normalize-sr":
        return normalize_sr(arguments.dicom)
    if arguments.command == "normalize-wsi":
        return normalize_wsi_source(pydicom.dcmread(arguments.source))
    if arguments.command == "metadata":
        return _metadata(arguments.dicom)
    if arguments.command == "build-scale":
        path = build_scale_ann(
            arguments.source,
            arguments.output,
            coordinate_values=arguments.coordinate_values,
        )
        return {"output": str(path), "bytes": path.stat().st_size}
    if arguments.command == "qualify-pydcm":
        return asdict(
            qualify_pydcm(
                source=arguments.source,
                ann=arguments.ann,
                seg=arguments.seg,
            )
        )
    if arguments.command == "environment":
        packages = {}
        for package in ("highdicom", "pydicom", "numpy", "pydcm"):
            try:
                packages[package] = importlib.metadata.version(package)
            except importlib.metadata.PackageNotFoundError:
                packages[package] = None
        return {"python": sys.version, "platform": platform.platform(), "packages": packages}
    raise ValueError(f"unsupported reference-shim command {arguments.command}")


def _metadata(path: Path) -> dict[str, Any]:
    dataset = pydicom.dcmread(path, stop_before_pixels=True)
    passthrough = (
        "SeriesNumber",
        "SeriesDescription",
        "Manufacturer",
        "ManufacturerModelName",
        "DeviceSerialNumber",
        "SoftwareVersions",
        "PositionReferenceIndicator",
    )
    return {
        "sop_instance_uid": str(dataset.SOPInstanceUID),
        "study_instance_uid": str(dataset.StudyInstanceUID),
        "series_instance_uid": str(dataset.SeriesInstanceUID),
        "frame_of_reference_uid": _optional_string(dataset, "FrameOfReferenceUID"),
        "preserved": {keyword: _optional_string(dataset, keyword) for keyword in passthrough},
    }


def _optional_string(dataset: pydicom.Dataset, keyword: str) -> str | None:
    value = getattr(dataset, keyword, None)
    return None if value is None else str(value)


def _parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(prog="wsi-reference-shim")
    subparsers = parser.add_subparsers(dest="command", required=True)

    generate = subparsers.add_parser("generate-core")
    generate.add_argument("--output", required=True, type=Path)

    normalize_ann_parser = subparsers.add_parser("normalize-ann")
    _add_normalize_arguments(normalize_ann_parser)
    normalize_ann_parser.add_argument("--canonical-source", type=Path)

    normalize_seg_parser = subparsers.add_parser("normalize-seg")
    _add_normalize_arguments(normalize_seg_parser)

    for command in ("normalize-pm", "normalize-sr"):
        normalize_derived = subparsers.add_parser(command)
        normalize_derived.add_argument("--dicom", required=True, type=Path)

    normalize_wsi_parser = subparsers.add_parser("normalize-wsi")
    normalize_wsi_parser.add_argument("--source", required=True, type=Path)

    metadata = subparsers.add_parser("metadata")
    metadata.add_argument("--dicom", required=True, type=Path)

    scale = subparsers.add_parser("build-scale")
    scale.add_argument("--source", required=True, type=Path)
    scale.add_argument("--output", required=True, type=Path)
    scale.add_argument("--coordinate-values", required=True, type=int)

    qualify = subparsers.add_parser("qualify-pydcm")
    qualify.add_argument("--source", required=True, type=Path)
    qualify.add_argument("--ann", required=True, type=Path)
    qualify.add_argument("--seg", required=True, type=Path)

    subparsers.add_parser("environment")

    return parser


def _add_normalize_arguments(parser: argparse.ArgumentParser) -> None:
    parser.add_argument("--source", required=True, type=Path)
    parser.add_argument("--annotation", required=True, type=Path)


if __name__ == "__main__":
    raise SystemExit(main())
