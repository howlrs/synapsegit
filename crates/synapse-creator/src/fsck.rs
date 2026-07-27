use crate::session::CreatorFsckReserve;
use crate::{CreatorError, Result};
use synapse_core::{FsckLimits, Repository, TombstoneScanLimits};
use synapse_sqlite::{RefRecord, RefSnapshot};

pub(crate) fn reserve_fsck_capacity(
    limits: FsckLimits,
    reserve: CreatorFsckReserve,
    operation: &str,
) -> Result<FsckLimits> {
    fn subtract_usize(
        available: usize,
        reserved: usize,
        resource: &str,
        operation: &str,
    ) -> Result<usize> {
        let remaining = available.checked_sub(reserved).ok_or_else(|| {
            CreatorError::ResourceLimit(format!(
                "creator {operation} cannot reserve {reserved} {resource} from limit {available}"
            ))
        })?;
        if remaining == 0 {
            return Err(CreatorError::ResourceLimit(format!(
                "creator {operation} reservation leaves no {resource} capacity"
            )));
        }
        Ok(remaining)
    }

    fn subtract_u64(available: u64, reserved: u64, resource: &str, operation: &str) -> Result<u64> {
        available
            .checked_sub(reserved)
            .filter(|remaining| *remaining > 0)
            .ok_or_else(|| {
                CreatorError::ResourceLimit(format!(
                    "creator {operation} cannot reserve {reserved} {resource} from limit {available}"
                ))
            })
    }

    Ok(FsckLimits {
        max_ref_roots: subtract_usize(
            limits.max_ref_roots,
            reserve.ref_roots,
            "Ref roots",
            operation,
        )?,
        max_objects: subtract_usize(limits.max_objects, reserve.objects, "objects", operation)?,
        max_object_bytes: subtract_u64(
            limits.max_object_bytes,
            reserve.object_bytes,
            "object bytes",
            operation,
        )?,
        max_closure_nodes: subtract_usize(
            limits.max_closure_nodes,
            reserve.closure_nodes,
            "closure nodes",
            operation,
        )?,
        max_closure_edges: subtract_usize(
            limits.max_closure_edges,
            reserve.closure_edges,
            "closure edges",
            operation,
        )?,
        tombstone_scan: TombstoneScanLimits {
            max_record_objects: subtract_usize(
                limits.tombstone_scan.max_record_objects,
                reserve.tombstone_records,
                "Tombstone-scan Records",
                operation,
            )?,
            max_record_bytes: subtract_u64(
                limits.tombstone_scan.max_record_bytes,
                reserve.tombstone_bytes,
                "Tombstone-scan Record bytes",
                operation,
            )?,
        },
    })
}

pub(crate) fn prospective_fsck(
    repository: &Repository,
    mut snapshot: RefSnapshot,
    updates: &[(&str, &str)],
    limits: FsckLimits,
    operation: &str,
) -> Result<()> {
    for (ref_name, head) in updates {
        if let Some(record) = snapshot
            .refs
            .iter_mut()
            .find(|record| record.name == *ref_name)
        {
            record.head = (*head).to_owned();
        } else {
            snapshot.refs.push(RefRecord {
                name: (*ref_name).to_owned(),
                head: (*head).to_owned(),
                updated_event_id: 0,
            });
        }
    }
    snapshot
        .refs
        .sort_by(|left, right| left.name.cmp(&right.name));
    let report = repository.fsck_snapshot_with_limits(&snapshot, limits)?;
    if !report.is_clean() {
        return Err(CreatorError::Integrity(format!(
            "creator {operation} prospective state has {} fsck issue(s)",
            report.issues.len()
        )));
    }
    Ok(())
}
