from __future__ import annotations

from dataclasses import dataclass
from pathlib import Path

import highdicom as hd
import numpy as np
import pydicom
from pydicom.dataset import Dataset, FileMetaDataset
from pydicom.sr.coding import Code
from pydicom.uid import UID, ExplicitVRLittleEndian, VLWholeSlideMicroscopyImageStorage

from .derived_fixtures import (
    parametric_map_dataset,
    segmentation_report_dataset,
    structured_report_dataset,
)
from .fixture_dataset import (
    FIXED_DATE,
    FIXED_TIME,
    fix_dimension_uid,
    fix_runtime_metadata,
    highdicom_source,
    save_dataset,
)

_STUDY_UID = "2.25.100000000000000000000000000000001"
_SOURCE_SERIES_UID = "2.25.100000000000000000000000000000002"
_SOURCE_SOP_UID = "2.25.100000000000000000000000000000003"
_FRAME_OF_REFERENCE_UID = "2.25.100000000000000000000000000000004"
_IMPLEMENTATION_UID = "2.25.100000000000000000000000000000005"
_PYRAMID_SOURCE_SERIES_UID = "2.25.100000000000000000000000000000009"
_PYRAMID_SOURCE_SOP_UID = "2.25.100000000000000000000000000000010"
_GRAPHIC_TYPES = ("POINT", "POLYLINE", "POLYGON", "ELLIPSE", "RECTANGLE")


@dataclass(frozen=True)
class FixtureSet:
    source: Path
    pyramid_source: Path
    pyramid_ann: Path
    reordered_seg: Path
    pm: Path
    sr: Path
    sr_seg: Path
    ann: dict[str, Path]
    seg: dict[str, Path]


def generate_core_fixtures(directory: str | Path) -> FixtureSet:
    """Generate deterministic, non-PHI WSI/ANN/SEG fixtures with highdicom."""
    root = Path(directory)
    root.mkdir(parents=True, exist_ok=True)
    source_dataset = _source_dataset(level=0)
    source_path = save_dataset(source_dataset, root / "source-wsi.dcm")
    pyramid_source_dataset = _source_dataset(level=1)
    pyramid_source_path = save_dataset(pyramid_source_dataset, root / "source-wsi-level1.dcm")
    pyramid_ann_path = save_dataset(
        _annotation_dataset(pyramid_source_dataset, "2D_VOLUME", 5),
        root / "ann-2d-volume-level1.dcm",
    )

    ann: dict[str, Path] = {}
    for index, form in enumerate(("2D_VOLUME", "2D_FRAME", "3D_COMMON_Z", "3D_XYZ"), 1):
        dataset = _annotation_dataset(source_dataset, form, index)
        ann[form] = save_dataset(dataset, root / f"ann-{form.lower().replace('_', '-')}.dcm")

    seg: dict[str, Path] = {}
    for index, kind in enumerate(("BINARY", "LABELMAP", "FRACTIONAL"), 1):
        dataset = _segmentation_dataset(source_dataset, kind, index)
        seg[kind] = save_dataset(dataset, root / f"seg-{kind.lower()}.dcm")
    reordered = _segmentation_dataset(
        source_dataset,
        "BINARY",
        4,
        dimension_organization_type="TILED_SPARSE",
    )
    _reverse_segmentation_frames(reordered)
    reordered_path = save_dataset(reordered, root / "seg-binary-reordered.dcm")
    pm_path = save_dataset(parametric_map_dataset(source_dataset), root / "pm-float32.dcm")
    sr_path = save_dataset(structured_report_dataset(source_dataset), root / "sr-tid1500.dcm")
    sr_seg_path = save_dataset(
        segmentation_report_dataset(source_dataset, pydicom.dcmread(seg["BINARY"])),
        root / "sr-tid1500-seg-reference.dcm",
    )
    return FixtureSet(
        source=source_path,
        pyramid_source=pyramid_source_path,
        pyramid_ann=pyramid_ann_path,
        reordered_seg=reordered_path,
        pm=pm_path,
        sr=sr_path,
        sr_seg=sr_seg_path,
        ann=ann,
        seg=seg,
    )


def build_scale_ann(
    source: str | Path,
    output: str | Path,
    *,
    coordinate_values: int,
) -> Path:
    """Create one deterministic polyline with an exact coordinate-value count."""
    if coordinate_values < 4 or coordinate_values % 2:
        raise ValueError("coordinate_values must be an even integer of at least 4")
    source_dataset = pydicom.dcmread(source)
    coordinates = np.arange(coordinate_values, dtype=np.float32).reshape((-1, 2))
    coordinates %= np.float32(15.0)
    group = hd.ann.AnnotationGroup(
        number=1,
        uid=f"2.25.7000000000000000000{coordinate_values}",
        label="SCALE",
        description=f"{coordinate_values} coordinate values",
        annotated_property_category=Code("MORPH", "99WSI", "Morphology"),
        annotated_property_type=Code("FEATURE", "99WSI", "Synthetic feature"),
        graphic_type="POLYLINE",
        graphic_data=[coordinates],
        algorithm_type="MANUAL",
    )
    dataset = hd.ann.MicroscopyBulkSimpleAnnotations(
        source_images=[highdicom_source(source_dataset)],
        annotation_coordinate_type="2D",
        pixel_origin_interpretation="VOLUME",
        annotation_groups=[group],
        series_instance_uid=f"2.25.7100000000000000000{coordinate_values}",
        series_number=90,
        sop_instance_uid=f"2.25.7200000000000000000{coordinate_values}",
        instance_number=1,
        manufacturer="WSI Interop Harness",
        manufacturer_model_name="highdicom scale fixture",
        software_versions=hd.__version__,
        device_serial_number="SYNTHETIC",
        content_label="SCALE",
        content_description="Deterministic scale workload",
    )
    fix_runtime_metadata(dataset)
    return save_dataset(dataset, Path(output))


def _source_dataset(*, level: int) -> Dataset:
    if level not in {0, 1}:
        raise ValueError("synthetic WSI level must be 0 or 1")
    source_sop_uid = _SOURCE_SOP_UID if level == 0 else _PYRAMID_SOURCE_SOP_UID
    source_series_uid = _SOURCE_SERIES_UID if level == 0 else _PYRAMID_SOURCE_SERIES_UID
    total_size = 16 if level == 0 else 8
    spacing = 0.001 if level == 0 else 0.002
    image_type = (
        ["ORIGINAL", "PRIMARY", "VOLUME", "NONE"]
        if level == 0
        else ["DERIVED", "PRIMARY", "VOLUME", "RESAMPLED"]
    )
    file_meta = FileMetaDataset()
    file_meta.FileMetaInformationVersion = b"\x00\x01"
    file_meta.MediaStorageSOPClassUID = VLWholeSlideMicroscopyImageStorage
    file_meta.MediaStorageSOPInstanceUID = UID(source_sop_uid)
    file_meta.TransferSyntaxUID = ExplicitVRLittleEndian
    file_meta.ImplementationClassUID = UID(_IMPLEMENTATION_UID)
    file_meta.ImplementationVersionName = "WSIINTEROP_1"

    dataset = Dataset()
    dataset.file_meta = file_meta
    dataset.SOPClassUID = VLWholeSlideMicroscopyImageStorage
    dataset.SOPInstanceUID = source_sop_uid
    dataset.StudyInstanceUID = _STUDY_UID
    dataset.SeriesInstanceUID = source_series_uid
    dataset.FrameOfReferenceUID = _FRAME_OF_REFERENCE_UID
    dataset.PositionReferenceIndicator = "SLIDE_CORNER"
    dataset.PatientID = "SYNTHETIC-RESEARCH"
    dataset.PatientName = ""
    dataset.PatientIdentityRemoved = "YES"
    dataset.DeidentificationMethod = "Synthetic non-PHI fixture"
    dataset.PatientBirthDate = ""
    dataset.PatientSex = ""
    dataset.PatientOrientation = ""
    dataset.StudyDate = FIXED_DATE
    dataset.StudyTime = FIXED_TIME
    dataset.AccessionNumber = ""
    dataset.ReferringPhysicianName = ""
    dataset.StudyID = "SYNTHETIC"
    dataset.SeriesNumber = 1
    dataset.InstanceNumber = level + 1
    dataset.Modality = "SM"
    dataset.ImageType = image_type
    dataset.Manufacturer = "WSI Interop Harness"
    dataset.ManufacturerModelName = "Synthetic WSI"
    dataset.DeviceSerialNumber = "SYNTHETIC"
    dataset.SoftwareVersions = "1"
    dataset.ContentDate = FIXED_DATE
    dataset.ContentTime = FIXED_TIME
    dataset.AcquisitionDateTime = f"{FIXED_DATE}{FIXED_TIME}"
    dataset.BurnedInAnnotation = "NO"
    dataset.RecognizableVisualFeatures = "NO"
    dataset.LossyImageCompression = "00"
    dataset.VolumetricProperties = "VOLUME"
    dataset.SpecimenLabelInImage = "NO"
    dataset.FocusMethod = "AUTO"
    dataset.ExtendedDepthOfField = "NO"
    dataset.PresentationLUTShape = "IDENTITY"
    dataset.ContainerIdentifier = "SYNTHETIC-SLIDE"
    dataset.IssuerOfTheContainerIdentifierSequence = []
    dataset.ContainerTypeCodeSequence = [hd.sr.CodedConcept("433466003", "SCT", "Microscope slide")]
    specimen = Dataset()
    specimen.SpecimenIdentifier = "SYNTHETIC-SLIDE"
    specimen.IssuerOfTheSpecimenIdentifierSequence = []
    specimen.SpecimenUID = "2.25.100000000000000000000000000000006"
    specimen.SpecimenPreparationSequence = []
    dataset.SpecimenDescriptionSequence = [specimen]
    dataset.AcquisitionContextSequence = []
    dataset.DimensionOrganizationType = "TILED_FULL"
    dimension = Dataset()
    dimension.DimensionOrganizationUID = f"2.25.100000000000000000000000000000{11 + level:03d}"
    dataset.DimensionOrganizationSequence = [dimension]
    dataset.PyramidUID = "2.25.100000000000000000000000000000008"
    dataset.NumberOfOpticalPaths = 1
    dataset.TotalPixelMatrixFocalPlanes = 1
    dataset.Rows = 4
    dataset.Columns = 4
    dataset.TotalPixelMatrixRows = total_size
    dataset.TotalPixelMatrixColumns = total_size
    dataset.NumberOfFrames = (total_size // dataset.Rows) * (total_size // dataset.Columns)
    dataset.SamplesPerPixel = 1
    dataset.PhotometricInterpretation = "MONOCHROME2"
    dataset.RescaleIntercept = "0"
    dataset.RescaleSlope = "1"
    dataset.BitsAllocated = 8
    dataset.BitsStored = 8
    dataset.HighBit = 7
    dataset.PixelRepresentation = 0
    dataset.ImageOrientationSlide = [1, 0, 0, 0, 1, 0]
    dataset.ImagedVolumeWidth = 0.016
    dataset.ImagedVolumeHeight = 0.016
    dataset.ImagedVolumeDepth = 1
    origin = Dataset()
    origin.XOffsetInSlideCoordinateSystem = 0
    origin.YOffsetInSlideCoordinateSystem = 0
    origin.ZOffsetInSlideCoordinateSystem = 0
    dataset.TotalPixelMatrixOriginSequence = [origin]

    pixel_measures = Dataset()
    pixel_measures.PixelSpacing = [spacing, spacing]
    pixel_measures.SliceThickness = 0.001
    shared = Dataset()
    shared.PixelMeasuresSequence = [pixel_measures]
    frame_type = Dataset()
    frame_type.FrameType = image_type
    shared.WholeSlideMicroscopyImageFrameTypeSequence = [frame_type]
    dataset.SharedFunctionalGroupsSequence = [shared]

    optical_path = Dataset()
    optical_path.OpticalPathIdentifier = "1"
    optical_path.OpticalPathDescription = "Synthetic monochrome optical path"
    optical_path.IlluminationWaveLength = 550
    optical_path.IlluminationTypeCodeSequence = [
        hd.sr.CodedConcept("111741", "DCM", "Transmission illumination")
    ]
    dataset.OpticalPathSequence = [optical_path]
    dataset.PixelData = bytes(dataset.NumberOfFrames * dataset.Rows * dataset.Columns)
    return dataset


def _annotation_dataset(source: Dataset, form: str, index: int) -> Dataset:
    dimensions = 3 if form.startswith("3D") else 2
    common_z = form == "3D_COMMON_Z"
    groups = [
        _annotation_group(number, graphic_type, dimensions, common_z)
        for number, graphic_type in enumerate(_GRAPHIC_TYPES, 1)
    ]
    dataset = hd.ann.MicroscopyBulkSimpleAnnotations(
        source_images=[highdicom_source(source)],
        annotation_coordinate_type="3D" if dimensions == 3 else "2D",
        pixel_origin_interpretation="FRAME" if form == "2D_FRAME" else "VOLUME",
        annotation_groups=groups,
        series_instance_uid=f"2.25.2000000000000000000000000000000{index}",
        series_number=10 + index,
        sop_instance_uid=f"2.25.2100000000000000000000000000000{index}",
        instance_number=1,
        manufacturer="WSI Interop Harness",
        manufacturer_model_name="highdicom core fixture",
        software_versions=hd.__version__,
        device_serial_number="SYNTHETIC",
        content_label=f"ANN{index}",
        content_description=f"Deterministic {form} annotation fixture",
    )
    if form == "2D_FRAME":
        dataset.ReferencedImageSequence[0].ReferencedFrameNumber = 1
    fix_runtime_metadata(dataset)
    return dataset


def _annotation_group(
    number: int,
    graphic_type: str,
    dimensions: int,
    common_z: bool,
) -> hd.ann.AnnotationGroup:
    data = _graphic_data(graphic_type, dimensions, common_z)
    algorithm = hd.AlgorithmIdentificationSequence(
        name="Synthetic annotation algorithm",
        family=Code("AI", "99WSI", "Artificial intelligence"),
        version="1.0.0",
        source="wsi-annotation-interop",
        parameters={"threshold": "0.5"},
    )
    measurement = hd.ann.Measurements(
        name=Code("AREA", "99WSI", "Synthetic area"),
        values=np.asarray([1.0, np.nan], dtype=np.float32),
        unit=Code("mm2", "UCUM", "square millimeter"),
    )
    group = hd.ann.AnnotationGroup(
        number=number,
        uid=f"2.25.3000000000000000000000000000000{number}",
        label=f"{graphic_type} GROUP",
        description=f"Two deterministic {graphic_type} annotations",
        annotated_property_category=Code(
            "MORPH",
            "99WSI",
            "Morphologically abnormal structure",
            "1",
        ),
        annotated_property_type=Code("TUMOR", "99WSI", "Tumor"),
        graphic_type=graphic_type,
        graphic_data=data,
        algorithm_type="AUTOMATIC",
        algorithm_identification=algorithm,
        measurements=[measurement],
        anatomic_regions=[Code("LUNG", "99WSI", "Lung")],
        primary_anatomic_structures=[Code("BRONCHUS", "99WSI", "Bronchus")],
        display_color=hd.color.CIELabColor(60.0, 20.0, 10.0),
    )
    group.AnnotationPropertyTypeModifierCodeSequence = [
        hd.sr.CodedConcept("urn:wsi-interop:viable", "99WSI", "Viable")
    ]
    group.AnnotationAppliesToAllOpticalPaths = "NO"
    group.ReferencedOpticalPathIdentifier = "1"
    return group


def _graphic_data(
    graphic_type: str,
    dimensions: int,
    common_z: bool,
) -> list[np.ndarray]:
    coordinates = {
        "POINT": [
            [[0.5, 0.5]],
            [[2.5, 2.5]],
        ],
        "POLYLINE": [
            [[0.5, 0.5], [1.5, 0.75], [2.5, 1.0]],
            [[0.5, 2.5], [1.5, 2.0], [2.5, 2.5]],
        ],
        "POLYGON": [
            [[0.5, 0.5], [1.5, 0.5], [1.5, 1.5], [0.5, 1.5]],
            [[2.0, 2.0], [3.0, 2.0], [3.0, 3.0], [2.0, 3.0]],
        ],
        "ELLIPSE": [
            [[0.5, 1.0], [2.5, 1.0], [1.5, 0.5], [1.5, 1.5]],
            [[1.0, 2.5], [3.0, 2.5], [2.0, 2.0], [2.0, 3.0]],
        ],
        "RECTANGLE": [
            [[0.5, 0.5], [1.5, 0.5], [1.5, 1.5], [0.5, 1.5]],
            [[2.0, 2.0], [3.0, 2.0], [3.0, 3.0], [2.0, 3.0]],
        ],
    }[graphic_type]
    arrays = [np.asarray(annotation, dtype=np.float64) for annotation in coordinates]
    if dimensions == 2:
        return arrays
    next_z = 0
    result = []
    for array in arrays:
        xy_mm = array * 0.001
        if common_z:
            z = np.full((len(array), 1), 0.01, dtype=np.float64)
        else:
            z = np.arange(next_z, next_z + len(array), dtype=np.float64).reshape((-1, 1))
            z = 0.01 + z * 0.0001
            next_z += len(array)
        result.append(np.concatenate((xy_mm, z), axis=1))
    return result


def _segmentation_dataset(
    source: Dataset,
    kind: str,
    index: int,
    *,
    dimension_organization_type: str = "TILED_FULL",
) -> Dataset:
    algorithm = hd.AlgorithmIdentificationSequence(
        name="Synthetic segmentation algorithm",
        family=Code("AI", "99WSI", "Artificial intelligence"),
        version="1.0.0",
        source="wsi-annotation-interop",
    )
    descriptions = [
        hd.seg.SegmentDescription(
            segment_number=number,
            segment_label=f"Segment {number}",
            segmented_property_category=Code("MORPH", "99WSI", "Morphology"),
            segmented_property_type=Code(f"TUMOR{number}", "99WSI", f"Tumor {number}"),
            algorithm_type="AUTOMATIC",
            algorithm_identification=algorithm,
            tracking_uid=f"2.25.4000000000000000000000000000000{number}",
            tracking_id=f"lesion-{number}",
            anatomic_regions=[Code("LUNG", "99WSI", "Lung")],
            display_color=hd.color.CIELabColor(50.0 + number, 10.0, 5.0),
        )
        for number in (1, 2)
    ]

    segmentation_type = kind
    fractional_type = None
    if kind == "BINARY":
        pixel_array = _binary_masks()
    elif kind == "LABELMAP":
        pixel_array = np.zeros((16, 16), dtype=np.uint8)
        pixel_array[1:7, 1:7] = 1
        pixel_array[9:15, 9:15] = 2
    else:
        descriptions = descriptions[:1]
        pixel_array = np.zeros((16, 16), dtype=np.float32)
        pixel_array[1:7, 1:7] = 0.25
        pixel_array[9:15, 9:15] = 0.75
        fractional_type = "PROBABILITY"

    dataset = hd.seg.Segmentation(
        source_images=[highdicom_source(source)],
        pixel_array=pixel_array,
        segmentation_type=segmentation_type,
        segment_descriptions=descriptions,
        series_instance_uid=f"2.25.5000000000000000000000000000000{index}",
        series_number=20 + index,
        sop_instance_uid=f"2.25.5100000000000000000000000000000{index}",
        instance_number=1,
        manufacturer="WSI Interop Harness",
        manufacturer_model_name="highdicom core fixture",
        software_versions=hd.__version__,
        device_serial_number="SYNTHETIC",
        fractional_type=fractional_type,
        content_label=f"SEG{index}",
        content_description=f"Deterministic {kind} segmentation fixture",
        tile_pixel_array=True,
        tile_size=(4, 4),
        omit_empty_frames=False,
        dimension_organization_type=dimension_organization_type,
    )
    if kind == "LABELMAP":
        dataset.PatientOrientation = ""
    fix_runtime_metadata(dataset)
    fix_dimension_uid(dataset, index)
    return dataset


def _reverse_segmentation_frames(dataset: Dataset) -> None:
    frame_count = int(dataset.NumberOfFrames)
    frame_bytes = (int(dataset.Rows) * int(dataset.Columns) * int(dataset.BitsAllocated) + 7) // 8
    expected_bytes = frame_count * frame_bytes
    if len(dataset.PixelData) not in {expected_bytes, expected_bytes + 1}:
        raise ValueError("synthetic SEG pixel payload has an unexpected frame layout")
    payload = bytes(dataset.PixelData[:expected_bytes])
    frames = [
        payload[offset : offset + frame_bytes] for offset in range(0, expected_bytes, frame_bytes)
    ]
    dataset.PixelData = b"".join(reversed(frames))
    dataset.PerFrameFunctionalGroupsSequence = list(
        reversed(dataset.PerFrameFunctionalGroupsSequence)
    )


def _binary_masks() -> np.ndarray:
    masks = np.zeros((1, 16, 16, 2), dtype=bool)
    masks[0, 1:9, 1:9, 0] = True
    masks[0, 4:6, 4:6, 0] = False
    masks[0, 12:14, 1:3, 0] = True
    masks[0, 6:14, 6:14, 1] = True
    masks[0, 9:11, 9:11, 1] = False
    return masks
