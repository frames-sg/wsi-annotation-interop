from __future__ import annotations

from typing import Any

import highdicom
from pydicom.dataset import Dataset
from pydicom.uid import (
    Comprehensive3DSRStorage,
    MicroscopyBulkSimpleAnnotationsStorage,
    ParametricMapStorage,
)


def read_dataset(dataset: Dataset) -> Any:
    """Wrap an in-memory DICOMweb result with the matching highdicom type."""
    sop_class_uid = str(dataset.SOPClassUID)
    if sop_class_uid == str(MicroscopyBulkSimpleAnnotationsStorage):
        return highdicom.ann.MicroscopyBulkSimpleAnnotations.from_dataset(dataset, copy=False)
    if sop_class_uid in highdicom.seg.SOP_CLASS_UIDS:
        return highdicom.seg.Segmentation.from_dataset(dataset, copy=False)
    if sop_class_uid == str(ParametricMapStorage):
        return highdicom.pm.ParametricMap.from_dataset(dataset, copy=False)
    if sop_class_uid == str(Comprehensive3DSRStorage):
        return highdicom.sr.Comprehensive3DSR.from_dataset(dataset, copy=False)
    raise ValueError(f"unsupported SOP Class UID {sop_class_uid}")
