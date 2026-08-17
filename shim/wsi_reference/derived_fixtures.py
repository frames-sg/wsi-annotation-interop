from __future__ import annotations

import highdicom as hd
import numpy as np
from pydicom.dataset import Dataset
from pydicom.sr.coding import Code

from .fixture_dataset import fix_dimension_uid, fix_runtime_metadata, highdicom_source

_PM_SERIES_UID = "2.25.100000000000000000000000000000013"
_PM_SOP_UID = "2.25.100000000000000000000000000000014"
_SR_SERIES_UID = "2.25.100000000000000000000000000000015"
_SR_SOP_UID = "2.25.100000000000000000000000000000016"
_SR_DEVICE_UID = "2.25.800000000000000000000000000000001"
_SR_TRACKING_UID = "2.25.800000000000000000000000000000002"
_SR_SEG_SERIES_UID = "2.25.100000000000000000000000000000017"
_SR_SEG_SOP_UID = "2.25.100000000000000000000000000000018"


def parametric_map_dataset(source: Dataset) -> Dataset:
    mapping = hd.pm.RealWorldValueMapping(
        lut_label="TUMOR",
        lut_explanation="Tumor probability",
        unit=Code("1", "UCUM", "no units"),
        value_range=(0.0, 1.0),
        slope=1.0,
        intercept=0.0,
        quantity_definition=Code("TUMOR", "99WSI", "Tumor probability"),
    )
    pixels = np.linspace(0.0, 1.0, 256, dtype=np.float32).reshape(16, 16)
    dataset = hd.pm.ParametricMap(
        source_images=[highdicom_source(source)],
        pixel_array=pixels,
        series_instance_uid=_PM_SERIES_UID,
        series_number=40,
        sop_instance_uid=_PM_SOP_UID,
        instance_number=1,
        manufacturer="WSI Interop Harness",
        manufacturer_model_name="highdicom PM oracle",
        software_versions=hd.__version__,
        device_serial_number="SYNTHETIC",
        contains_recognizable_visual_features=False,
        real_world_value_mappings=[mapping],
        voi_lut_transformations=[hd.VOILUTTransformation(window_center=0.5, window_width=1.0)],
        content_label="PMORACLE",
        content_description="Independent highdicom PM oracle",
        tile_pixel_array=True,
        dimension_organization_type="TILED_FULL",
    )
    fix_runtime_metadata(dataset)
    fix_dimension_uid(dataset, 13)
    return dataset


def structured_report_dataset(source: Dataset) -> Dataset:
    region = hd.sr.ImageRegion3D(
        graphic_type="POLYGON",
        graphic_data=np.asarray(
            [
                [0.001, 0.001, 0.0],
                [0.006, 0.001, 0.0],
                [0.006, 0.006, 0.0],
                [0.001, 0.006, 0.0],
                [0.001, 0.001, 0.0],
            ],
            dtype=float,
        ),
        frame_of_reference_uid=str(source.FrameOfReferenceUID),
    )
    group = _sr_measurement_group(_SR_TRACKING_UID, "2.25.71", referenced_region=region)
    return _sr_document(source, [source], group, _SR_SERIES_UID, _SR_SOP_UID, 50)


def segmentation_report_dataset(source: Dataset, segmentation: Dataset) -> Dataset:
    # TILED_FULL omits per-frame functional groups. Its implicit frame order
    # groups the complete tile grid by segment, so segment 1 occupies the first block.
    frames_per_segment = int(segmentation.NumberOfFrames) // len(segmentation.SegmentSequence)
    frame_numbers = list(range(1, frames_per_segment + 1))
    reference = hd.sr.ReferencedSegmentationFrame(
        sop_class_uid=str(segmentation.SOPClassUID),
        sop_instance_uid=str(segmentation.SOPInstanceUID),
        frame_number=frame_numbers,
        segment_number=1,
        source_image=hd.sr.SourceImageForSegmentation(
            referenced_sop_class_uid=str(source.SOPClassUID),
            referenced_sop_instance_uid=str(source.SOPInstanceUID),
        ),
    )
    segment = segmentation.SegmentSequence[0]
    group = _sr_measurement_group(
        str(segment.TrackingUID),
        str(segment.TrackingID),
        referenced_segment=reference,
    )
    return _sr_document(
        source, [source, segmentation], group, _SR_SEG_SERIES_UID, _SR_SEG_SOP_UID, 51
    )


def _sr_measurement_group(
    tracking_uid: str,
    tracking_id: str,
    *,
    referenced_region: hd.sr.ImageRegion3D | None = None,
    referenced_segment: hd.sr.ReferencedSegmentationFrame | None = None,
) -> hd.sr.PlanarROIMeasurementsAndQualitativeEvaluations:
    return hd.sr.PlanarROIMeasurementsAndQualitativeEvaluations(
        tracking_identifier=hd.sr.TrackingIdentifier(uid=tracking_uid, identifier=tracking_id),
        referenced_region=referenced_region,
        referenced_segment=referenced_segment,
        finding_category=Code("M-01000", "SRT", "Morphologically Altered Structure"),
        finding_type=Code("108369006", "SCT", "Neoplasm"),
        algorithm_id=hd.sr.AlgorithmIdentification(
            name="Reference model",
            version="1.0",
            family=Code("123110", "DCM", "Artificial Intelligence"),
        ),
        measurements=[
            hd.sr.Measurement(
                name=Code("AREA", "99WSI", "Area"),
                value=25.0,
                unit=Code("mm2", "UCUM", "square millimeter"),
            )
        ],
        qualitative_evaluations=[
            hd.sr.QualitativeEvaluation(
                name=Code("GRADE", "99WSI", "Grade"),
                value=Code("HIGH", "99WSI", "High"),
            )
        ],
    )


def _sr_document(
    source: Dataset,
    evidence: list[Dataset],
    group: hd.sr.PlanarROIMeasurementsAndQualitativeEvaluations,
    series_uid: str,
    sop_uid: str,
    series_number: int,
) -> Dataset:
    procedure = Code("P5-09051", "SRT", "Histopathology procedure")
    observer = hd.sr.ObserverContext(
        observer_type=hd.sr.CodedConcept("121007", "DCM", "Device"),
        observer_identifying_attributes=hd.sr.DeviceObserverIdentifyingAttributes(
            uid=_SR_DEVICE_UID,
            name="Reference oracle",
            manufacturer_name="WSI Interop Harness",
            model_name="highdicom SR oracle",
            serial_number="SYNTHETIC",
        ),
    )
    report = hd.sr.MeasurementReport(
        observation_context=hd.sr.ObservationContext(observer_device_context=observer),
        procedure_reported=procedure,
        imaging_measurements=[group],
        title=Code("126000", "DCM", "Imaging Measurement Report"),
    )
    dataset = hd.sr.Comprehensive3DSR(
        evidence=[highdicom_source(item) for item in evidence],
        content=report,
        series_instance_uid=series_uid,
        series_number=series_number,
        sop_instance_uid=sop_uid,
        instance_number=1,
        manufacturer="WSI Interop Harness",
        is_complete=True,
        is_final=False,
        is_verified=False,
        performed_procedure_codes=[procedure],
    )
    fix_runtime_metadata(dataset)
    return dataset
