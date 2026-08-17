from __future__ import annotations

from copy import deepcopy
from pathlib import Path

from pydicom.dataset import Dataset

FIXED_DATE = "20260813"
FIXED_TIME = "120000"


def highdicom_source(source: Dataset) -> Dataset:
    value = deepcopy(source)
    value.PatientName = ""
    return value


def fix_runtime_metadata(dataset: Dataset) -> None:
    dataset.ContentDate = FIXED_DATE
    dataset.ContentTime = FIXED_TIME
    dataset.InstanceCreationDate = FIXED_DATE
    dataset.InstanceCreationTime = FIXED_TIME
    if hasattr(dataset, "SeriesDate"):
        dataset.SeriesDate = FIXED_DATE
    if hasattr(dataset, "SeriesTime"):
        dataset.SeriesTime = FIXED_TIME
    for item in getattr(dataset, "ContributingEquipmentSequence", []):
        item.ContributionDateTime = f"{FIXED_DATE}{FIXED_TIME}"


def fix_dimension_uid(dataset: Dataset, index: int) -> None:
    uid = f"2.25.6000000000000000000000000000000{index}"
    for item in getattr(dataset, "DimensionOrganizationSequence", []):
        item.DimensionOrganizationUID = uid
    for item in getattr(dataset, "DimensionIndexSequence", []):
        item.DimensionOrganizationUID = uid


def save_dataset(dataset: Dataset, path: Path) -> Path:
    path.parent.mkdir(parents=True, exist_ok=True)
    dataset.save_as(path, enforce_file_format=True)
    return path
