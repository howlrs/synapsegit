use synapse_canonical::{ObjectKind, Value, parse_oid};

use crate::error::{ProjectionError, Result};
use crate::rebuild::{
    AnalysisLinkRow, AnalysisRow, BuildPlan, DependencyRow, EdgeRow, RecordRow, SeriesLinkRow,
    SubjectLinkRow, TimelineRow,
};
use crate::store::{
    AdapterDeterminism, AnalysisMaskRole, ObservationDependencyKind, TimelineRecordKind,
    TimelineTimeBasis,
};

pub(crate) struct AnalysisQueryRow {
    pub(crate) entity_id: String,
    pub(crate) recorded_at: String,
    pub(crate) asserted_by: String,
    pub(crate) analysis_kind: String,
    pub(crate) comparison_kind: String,
    pub(crate) status: String,
    pub(crate) comparability: String,
    pub(crate) adapter_id: String,
    pub(crate) adapter_version: String,
    pub(crate) implementation_oid: String,
    pub(crate) configuration_oid: String,
    pub(crate) determinism: String,
    pub(crate) seed: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) enum AnalysisLinkCategory {
    Input,
    Transform,
    DerivedBlob,
    Mask,
}

impl AnalysisLinkCategory {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Input => "input",
            Self::Transform => "transform",
            Self::DerivedBlob => "derived_blob",
            Self::Mask => "mask",
        }
    }

    pub(crate) fn parse(value: &str) -> Result<Self> {
        match value {
            "input" => Ok(Self::Input),
            "transform" => Ok(Self::Transform),
            "derived_blob" => Ok(Self::DerivedBlob),
            "mask" => Ok(Self::Mask),
            _ => Err(ProjectionError::CorruptProjection(format!(
                "unknown Analysis link category {value:?}"
            ))),
        }
    }
}

impl BuildPlan {
    pub(crate) fn map_record(&mut self, oid: &str, value: &Value) -> Result<()> {
        let record_type = required_string(value, "record_type", oid)?;
        let entity_id = required_string(value, "entity_id", oid)?;
        let recorded_at = required_string(value, "recorded_at", oid)?;
        let asserted_by = required_string(value, "asserted_by", oid)?;
        let row = self.objects.get_mut(oid).ok_or_else(|| {
            ProjectionError::InvalidSource(format!("Record {oid} is absent from object plan"))
        })?;
        row.record_type = Some(record_type.to_owned());
        row.entity_id = Some(entity_id.to_owned());
        row.recorded_at = Some(recorded_at.to_owned());
        row.asserted_by = Some(asserted_by.to_owned());
        self.records.insert(RecordRow {
            oid: oid.to_owned(),
            record_type: record_type.to_owned(),
            entity_id: entity_id.to_owned(),
            recorded_at: recorded_at.to_owned(),
            asserted_by: asserted_by.to_owned(),
        });

        match record_type {
            "observation" => self.map_observation(oid, value),
            "activity" => self.map_activity(oid, value),
            "analysis_result" => self.map_analysis(oid, value),
            _ => Ok(()),
        }
    }

    fn map_analysis(&mut self, oid: &str, value: &Value) -> Result<()> {
        let payload = required_object(value, "payload", oid)?;
        let adapter = required_object(payload, "adapter", oid)?;
        let implementation_oid = required_string(adapter, "implementation_digest", oid)?;
        let configuration_oid = required_string(adapter, "configuration_digest", oid)?;
        self.validate_analysis_target(
            oid,
            "adapter.implementation_digest",
            implementation_oid,
            None,
        )?;
        self.validate_analysis_target(
            oid,
            "adapter.configuration_digest",
            configuration_oid,
            None,
        )?;
        let determinism = required_string(adapter, "determinism", oid)?;
        let determinism = match determinism {
            "deterministic" => AdapterDeterminism::Deterministic,
            "seeded" => AdapterDeterminism::Seeded,
            "probabilistic" => AdapterDeterminism::Probabilistic,
            value => {
                return Err(ProjectionError::InvalidSource(format!(
                    "AnalysisResult {oid} has unsupported adapter determinism {value:?}"
                )));
            }
        };
        self.analyses.insert(AnalysisRow {
            analysis_oid: oid.to_owned(),
            analysis_kind: required_string(payload, "analysis_kind", oid)?.to_owned(),
            comparison_kind: required_string(payload, "comparison_kind", oid)?.to_owned(),
            status: required_string(payload, "status", oid)?.to_owned(),
            comparability: required_string(payload, "comparability", oid)?.to_owned(),
            adapter_id: required_string(adapter, "id", oid)?.to_owned(),
            adapter_version: required_string(adapter, "version", oid)?.to_owned(),
            implementation_oid: implementation_oid.to_owned(),
            configuration_oid: configuration_oid.to_owned(),
            determinism: determinism.as_str().to_owned(),
            seed: adapter
                .get("seed")
                .and_then(Value::as_str)
                .map(str::to_owned),
        });

        for (ordinal, input) in required_array(payload, "inputs", oid)?.iter().enumerate() {
            let target_oid = required_string(input, "ref", oid)?;
            self.validate_analysis_target(oid, "inputs[].ref", target_oid, None)?;
            self.analysis_links.insert(AnalysisLinkRow {
                analysis_oid: oid.to_owned(),
                category: AnalysisLinkCategory::Input,
                ordinal,
                role: Some(required_string(input, "role", oid)?.to_owned()),
                target_oid: target_oid.to_owned(),
            });
        }

        if let Some(transforms) = payload.get("transform_refs").and_then(Value::as_array) {
            for (ordinal, target) in transforms.iter().enumerate() {
                let target_oid = target.as_str().ok_or_else(|| {
                    ProjectionError::InvalidSource(format!(
                        "AnalysisResult {oid} transform_refs contains a non-string"
                    ))
                })?;
                self.validate_analysis_target(
                    oid,
                    "transform_refs[]",
                    target_oid,
                    Some(ObjectKind::Record),
                )?;
                self.analysis_links.insert(AnalysisLinkRow {
                    analysis_oid: oid.to_owned(),
                    category: AnalysisLinkCategory::Transform,
                    ordinal,
                    role: None,
                    target_oid: target_oid.to_owned(),
                });
            }
        }

        for (ordinal, target) in required_array(payload, "derived_blob_refs", oid)?
            .iter()
            .enumerate()
        {
            let target_oid = target.as_str().ok_or_else(|| {
                ProjectionError::InvalidSource(format!(
                    "AnalysisResult {oid} derived_blob_refs contains a non-string"
                ))
            })?;
            self.validate_analysis_target(
                oid,
                "derived_blob_refs[]",
                target_oid,
                Some(ObjectKind::Blob),
            )?;
            self.analysis_links.insert(AnalysisLinkRow {
                analysis_oid: oid.to_owned(),
                category: AnalysisLinkCategory::DerivedBlob,
                ordinal,
                role: None,
                target_oid: target_oid.to_owned(),
            });
        }

        if let Some(mask_refs) = payload.get("mask_refs") {
            mask_refs.as_object().ok_or_else(|| {
                ProjectionError::InvalidSource(format!(
                    "AnalysisResult {oid} field mask_refs is not an object"
                ))
            })?;
            for (ordinal, role) in [
                AnalysisMaskRole::Changed,
                AnalysisMaskRole::Unchanged,
                AnalysisMaskRole::Ambiguous,
                AnalysisMaskRole::Unobservable,
                AnalysisMaskRole::Validity,
            ]
            .into_iter()
            .enumerate()
            {
                let Some(target_oid) = mask_refs.get(role.as_str()).and_then(Value::as_str) else {
                    continue;
                };
                self.validate_analysis_target(
                    oid,
                    "mask_refs",
                    target_oid,
                    Some(ObjectKind::Blob),
                )?;
                self.analysis_links.insert(AnalysisLinkRow {
                    analysis_oid: oid.to_owned(),
                    category: AnalysisLinkCategory::Mask,
                    ordinal,
                    role: Some(role.as_str().to_owned()),
                    target_oid: target_oid.to_owned(),
                });
            }
        }
        Ok(())
    }

    pub(crate) fn validate_analysis_target(
        &self,
        analysis_oid: &str,
        field: &str,
        target_oid: &str,
        expected_kind: Option<ObjectKind>,
    ) -> Result<()> {
        let actual_kind = parse_oid(target_oid).map_err(|error| {
            ProjectionError::InvalidSource(format!(
                "AnalysisResult {analysis_oid} {field} has invalid OID: {error}"
            ))
        })?;
        if let Some(expected_kind) = expected_kind
            && actual_kind != expected_kind
        {
            return Err(ProjectionError::InvalidSource(format!(
                "AnalysisResult {analysis_oid} {field} requires {} OID, found {}",
                expected_kind.prefix(),
                actual_kind.prefix()
            )));
        }
        if !self.objects.contains_key(target_oid) {
            return Err(ProjectionError::InvalidSource(format!(
                "AnalysisResult {analysis_oid} {field} target {target_oid} is absent from its verified closure"
            )));
        }
        let edge_start = EdgeRow {
            source_oid: analysis_oid.to_owned(),
            target_oid: target_oid.to_owned(),
            role: String::new(),
            expected_kind: String::new(),
        };
        let directly_linked = self.edges.range(edge_start..).next().is_some_and(|edge| {
            edge.source_oid == analysis_oid
                && edge.target_oid == target_oid
                && edge.expected_kind == actual_kind.prefix()
        });
        if !directly_linked {
            return Err(ProjectionError::InvalidSource(format!(
                "AnalysisResult {analysis_oid} {field} target {target_oid} has no matching verified graph edge"
            )));
        }
        Ok(())
    }

    fn map_observation(&mut self, oid: &str, value: &Value) -> Result<()> {
        let payload = required_object(value, "payload", oid)?;
        let subject_id = required_string(payload, "subject_ref", oid)?;
        let series_id = required_string(payload, "series_ref", oid)?;
        self.subject_links.insert(SubjectLinkRow {
            record_oid: oid.to_owned(),
            subject_id: subject_id.to_owned(),
        });
        self.series_links.insert(SeriesLinkRow {
            record_oid: oid.to_owned(),
            series_id: series_id.to_owned(),
        });
        let record = self
            .records
            .iter()
            .find(|row| row.oid == oid)
            .ok_or_else(|| {
                ProjectionError::InvalidSource(format!("Observation {oid} lacks common Record row"))
            })?;
        let time = project_valid_time(
            required_object(payload, "capture_time", oid)?,
            &record.recorded_at,
            true,
            oid,
        )?;
        self.timelines.insert(TimelineRow {
            record_oid: oid.to_owned(),
            record_kind: TimelineRecordKind::Observation.as_str().to_owned(),
            entity_id: record.entity_id.clone(),
            ordering_time: time.ordering_time,
            time_basis: time.basis.as_str().to_owned(),
            event_time_start: time.start,
            event_time_end: time.end,
            recorded_at: record.recorded_at.clone(),
            asserted_by: record.asserted_by.clone(),
        });

        self.map_optional_dependency(
            oid,
            payload,
            "capture_profile_ref",
            ObservationDependencyKind::CaptureProfile,
            None,
        )?;
        self.map_optional_dependency(
            oid,
            payload,
            "station_ref",
            ObservationDependencyKind::Station,
            Some("entity"),
        )?;
        self.map_optional_dependency(
            oid,
            payload,
            "station_deployment_ref",
            ObservationDependencyKind::StationDeployment,
            None,
        )?;
        self.map_oid_array(
            oid,
            payload,
            "calibration_refs",
            ObservationDependencyKind::Calibration,
        )?;
        self.map_oid_array(
            oid,
            payload,
            "environment_refs",
            ObservationDependencyKind::Environment,
        )?;
        for (ordinal, media) in required_array(payload, "media_refs", oid)?
            .iter()
            .enumerate()
        {
            let target = required_string(media, "oid", oid)?;
            let role = required_string(media, "role", oid)?;
            self.dependencies.insert(DependencyRow {
                observation_oid: oid.to_owned(),
                dependency_kind: ObservationDependencyKind::Media.as_str().to_owned(),
                target_ref: target.to_owned(),
                target_kind: kind_name(parse_oid(target).map_err(|error| {
                    ProjectionError::InvalidSource(format!(
                        "Observation {oid} media OID is invalid: {error}"
                    ))
                })?)
                .to_owned(),
                role: Some(role.to_owned()),
                ordinal,
            });
        }
        Ok(())
    }

    fn map_activity(&mut self, oid: &str, value: &Value) -> Result<()> {
        let payload = required_object(value, "payload", oid)?;
        for subject in required_array(payload, "subject_refs", oid)? {
            let subject = subject.as_str().ok_or_else(|| {
                ProjectionError::InvalidSource(format!(
                    "Activity {oid} contains non-string subject_ref"
                ))
            })?;
            self.subject_links.insert(SubjectLinkRow {
                record_oid: oid.to_owned(),
                subject_id: subject.to_owned(),
            });
        }
        let record = self
            .records
            .iter()
            .find(|row| row.oid == oid)
            .ok_or_else(|| {
                ProjectionError::InvalidSource(format!("Activity {oid} lacks common Record row"))
            })?;
        let time = project_valid_time(
            required_object(value, "valid_time", oid)?,
            &record.recorded_at,
            false,
            oid,
        )?;
        self.timelines.insert(TimelineRow {
            record_oid: oid.to_owned(),
            record_kind: TimelineRecordKind::Activity.as_str().to_owned(),
            entity_id: record.entity_id.clone(),
            ordering_time: time.ordering_time,
            time_basis: time.basis.as_str().to_owned(),
            event_time_start: time.start,
            event_time_end: time.end,
            recorded_at: record.recorded_at.clone(),
            asserted_by: record.asserted_by.clone(),
        });
        Ok(())
    }

    fn map_optional_dependency(
        &mut self,
        observation_oid: &str,
        payload: &Value,
        field: &str,
        kind: ObservationDependencyKind,
        forced_target_kind: Option<&str>,
    ) -> Result<()> {
        let Some(target) = payload.get(field).and_then(Value::as_str) else {
            return Ok(());
        };
        let target_kind = match forced_target_kind {
            Some(kind) => kind.to_owned(),
            None => kind_name(parse_oid(target).map_err(|error| {
                ProjectionError::InvalidSource(format!(
                    "Observation {observation_oid} {field} is invalid: {error}"
                ))
            })?)
            .to_owned(),
        };
        self.dependencies.insert(DependencyRow {
            observation_oid: observation_oid.to_owned(),
            dependency_kind: kind.as_str().to_owned(),
            target_ref: target.to_owned(),
            target_kind,
            role: None,
            ordinal: 0,
        });
        Ok(())
    }

    fn map_oid_array(
        &mut self,
        observation_oid: &str,
        payload: &Value,
        field: &str,
        kind: ObservationDependencyKind,
    ) -> Result<()> {
        let Some(values) = payload.get(field).and_then(Value::as_array) else {
            return Ok(());
        };
        for (ordinal, value) in values.iter().enumerate() {
            let target = value.as_str().ok_or_else(|| {
                ProjectionError::InvalidSource(format!(
                    "Observation {observation_oid} {field} contains a non-string"
                ))
            })?;
            self.dependencies.insert(DependencyRow {
                observation_oid: observation_oid.to_owned(),
                dependency_kind: kind.as_str().to_owned(),
                target_ref: target.to_owned(),
                target_kind: kind_name(parse_oid(target).map_err(|error| {
                    ProjectionError::InvalidSource(format!(
                        "Observation {observation_oid} {field} OID is invalid: {error}"
                    ))
                })?)
                .to_owned(),
                role: None,
                ordinal,
            });
        }
        Ok(())
    }
}

struct ProjectedTime {
    ordering_time: String,
    basis: TimelineTimeBasis,
    start: Option<String>,
    end: Option<String>,
}

fn project_valid_time(
    valid_time: &Value,
    recorded_at: &str,
    observation: bool,
    oid: &str,
) -> Result<ProjectedTime> {
    match valid_time.get("kind").and_then(Value::as_str) {
        Some("instant") => {
            let at = required_string(valid_time, "at", oid)?.to_owned();
            Ok(ProjectedTime {
                ordering_time: at.clone(),
                basis: if observation {
                    TimelineTimeBasis::ObservationCaptureInstant
                } else {
                    TimelineTimeBasis::ActivityValidInstant
                },
                start: Some(at),
                end: None,
            })
        }
        Some("interval") => {
            let start = valid_time
                .get("from")
                .and_then(Value::as_str)
                .map(str::to_owned);
            let end = valid_time
                .get("to")
                .and_then(Value::as_str)
                .map(str::to_owned);
            let ordering_time = start
                .as_ref()
                .or(end.as_ref())
                .ok_or_else(|| {
                    ProjectionError::InvalidSource(format!(
                        "Record {oid} has an interval without from or to"
                    ))
                })?
                .clone();
            Ok(ProjectedTime {
                ordering_time,
                basis: if observation {
                    TimelineTimeBasis::ObservationCaptureInterval
                } else {
                    TimelineTimeBasis::ActivityValidInterval
                },
                start,
                end,
            })
        }
        Some("unknown") => Ok(ProjectedTime {
            ordering_time: recorded_at.to_owned(),
            basis: if observation {
                TimelineTimeBasis::ObservationRecordedAtFallback
            } else {
                TimelineTimeBasis::ActivityRecordedAtFallback
            },
            start: None,
            end: None,
        }),
        kind => Err(ProjectionError::InvalidSource(format!(
            "Record {oid} has unsupported ValidTime kind {kind:?}"
        ))),
    }
}

pub(crate) fn required_string<'value>(
    value: &'value Value,
    key: &str,
    oid: &str,
) -> Result<&'value str> {
    value.get(key).and_then(Value::as_str).ok_or_else(|| {
        ProjectionError::InvalidSource(format!("object {oid} requires string field {key}"))
    })
}

fn required_object<'value>(value: &'value Value, key: &str, oid: &str) -> Result<&'value Value> {
    let child = value.get(key).ok_or_else(|| {
        ProjectionError::InvalidSource(format!("object {oid} requires object field {key}"))
    })?;
    child.as_object().ok_or_else(|| {
        ProjectionError::InvalidSource(format!("object {oid} field {key} is not an object"))
    })?;
    Ok(child)
}

fn required_array<'value>(value: &'value Value, key: &str, oid: &str) -> Result<&'value [Value]> {
    value.get(key).and_then(Value::as_array).ok_or_else(|| {
        ProjectionError::InvalidSource(format!("object {oid} requires array field {key}"))
    })
}

pub(crate) const fn kind_name(kind: ObjectKind) -> &'static str {
    kind.prefix()
}
