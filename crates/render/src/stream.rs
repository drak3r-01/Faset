//! Streamed mesh slots (lot 8.1): meshes rewritten while rendering, without
//! waiting for the GPU.
//!
//! [`Renderer::reserve_mesh_slots`](crate::Renderer::reserve_mesh_slots)
//! carves a fixed number of fixed-capacity ranges out of the static arena
//! once; [`Renderer::write_mesh_slot`](crate::Renderer::write_mesh_slot)
//! queues new contents for one of them. The next frame copies the queued
//! bytes into its own host-visible staging buffer (the frame's fence was
//! waited, so the buffer is free), records the buffer copies at the start of
//! its command buffer behind a barrier that waits for the previous frames'
//! vertex reads, and draws the new contents. GPU memory is bounded by the
//! slot count, upload traffic by [`STREAM_BYTES_PER_FRAME`].

use crate::mesh::{Aabb, MeshId, MeshRange, Vertex};
use crate::opaque::MeshArena;
use crate::RenderError;

/// Most bytes (vertices plus indices) queued for one frame: 1 MiB.
pub const STREAM_BYTES_PER_FRAME: usize = 1 << 20;

/// One queued slot write.
pub(crate) struct Pending {
    pub mesh: MeshId,
    pub vertices: Vec<Vertex>,
    pub indices: Vec<u32>,
}

impl Pending {
    pub(crate) fn bytes(&self) -> usize {
        self.vertices.len() * std::mem::size_of::<Vertex>() + self.indices.len() * 4
    }
}

/// The reserved slots and the writes waiting for the next submitted frame.
#[derive(Default)]
pub(crate) struct StreamQueue {
    /// Slot meshes, in reservation order.
    slots: Vec<MeshId>,
    max_vertices: u32,
    max_indices: u32,
    /// Writes not yet submitted (kept when a frame is abandoned).
    pub pending: Vec<Pending>,
    bytes: usize,
}

impl StreamQueue {
    /// Reserves `count` slots in `arena`, reusing the slots of an earlier call
    /// with the same capacities (their contents are cleared). Different
    /// capacities start a new pool; the old ranges stay unused in the arena.
    pub(crate) fn reserve(
        &mut self,
        arena: &mut MeshArena,
        count: u32,
        max_vertices: u32,
        max_indices: u32,
    ) -> Result<Vec<MeshId>, RenderError> {
        if max_vertices == 0 || max_indices == 0 || !max_indices.is_multiple_of(3) {
            return Err(RenderError::InvalidMesh);
        }
        if (max_vertices, max_indices) != (self.max_vertices, self.max_indices) {
            self.slots.clear();
            self.max_vertices = max_vertices;
            self.max_indices = max_indices;
        }
        self.pending.clear();
        self.bytes = 0;
        while self.slots.len() < count as usize {
            let (first_vertex, first_index) = arena
                .layout
                .reserve(max_vertices as usize, max_indices as usize)
                .ok_or(RenderError::ArenaFull)?;
            let id = MeshId(u32::try_from(arena.meshes.len()).map_err(|_| RenderError::ArenaFull)?);
            arena.meshes.push(MeshRange {
                first_index,
                index_count: 0,
                base_vertex: i32::try_from(first_vertex).map_err(|_| RenderError::ArenaFull)?,
                bounds: Aabb::from_vertices(&[]),
            });
            self.slots.push(id);
        }
        for id in &self.slots {
            arena.meshes[id.0 as usize].index_count = 0;
        }
        Ok(self.slots[..count as usize].to_vec())
    }

    /// Bytes that can still be queued before the next frame.
    pub(crate) fn bytes_left(&self) -> usize {
        STREAM_BYTES_PER_FRAME.saturating_sub(self.bytes)
    }

    /// Queues a write; checked against the slot, its capacity and the budget.
    pub(crate) fn queue(
        &mut self,
        mesh: MeshId,
        vertices: &[Vertex],
        indices: &[u32],
    ) -> Result<(), RenderError> {
        if !self.slots.contains(&mesh) {
            return Err(RenderError::NotASlot(mesh.0));
        }
        if vertices.len() > self.max_vertices as usize
            || indices.len() > self.max_indices as usize
            || !indices.len().is_multiple_of(3)
            || indices.iter().any(|&i| i as usize >= vertices.len())
        {
            return Err(RenderError::InvalidMesh);
        }
        let write = Pending {
            mesh,
            vertices: vertices.to_vec(),
            indices: indices.to_vec(),
        };
        let bytes = write.bytes();
        if bytes > self.bytes_left() {
            return Err(RenderError::StreamBudget {
                bytes,
                left: self.bytes_left(),
            });
        }
        self.bytes += bytes;
        self.pending.push(write);
        Ok(())
    }

    /// Points the slot ranges at their queued contents (index count and
    /// bounds). Idempotent; done before culling every frame that records the
    /// copies.
    pub(crate) fn apply_ranges(&self, arena: &mut MeshArena) {
        for w in &self.pending {
            let range = &mut arena.meshes[w.mesh.0 as usize];
            range.index_count = w.indices.len() as u32;
            range.bounds = Aabb::from_vertices(&w.vertices);
        }
    }

    /// Byte offsets of each queued write's vertices and indices inside the
    /// staging buffer, and the total size. Vertices first, then indices.
    pub(crate) fn staging_layout(&self) -> (Vec<(usize, usize)>, usize) {
        let mut at = 0;
        let mut out = Vec::with_capacity(self.pending.len());
        for w in &self.pending {
            let v = w.vertices.len() * std::mem::size_of::<Vertex>();
            out.push((at, at + v));
            at += w.bytes();
        }
        (out, at)
    }

    /// The writes were submitted: forget them and refill the budget.
    pub(crate) fn submitted(&mut self) {
        self.pending.clear();
        self.bytes = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use faset_core::glam::Vec3;

    fn queue_with_slots() -> StreamQueue {
        StreamQueue {
            slots: vec![MeshId(3), MeshId(4)],
            max_vertices: 8,
            max_indices: 12,
            ..StreamQueue::default()
        }
    }

    fn tri() -> (Vec<Vertex>, Vec<u32>) {
        let v = Vertex::new(Vec3::ZERO, Vec3::Y, [255; 3], 0);
        (vec![v; 3], vec![0, 1, 2])
    }

    #[test]
    fn writes_are_checked_and_budgeted() {
        let mut q = queue_with_slots();
        let (v, i) = tri();
        assert!(matches!(
            q.queue(MeshId(9), &v, &i),
            Err(RenderError::NotASlot(9))
        ));
        assert!(matches!(
            q.queue(MeshId(3), &v, &[0, 1, 3]),
            Err(RenderError::InvalidMesh)
        ));
        assert!(matches!(
            q.queue(MeshId(3), &[v[0]; 9], &i),
            Err(RenderError::InvalidMesh)
        ));
        q.queue(MeshId(3), &v, &i).unwrap();
        q.queue(MeshId(4), &v, &i).unwrap();
        let one = 3 * 20 + 3 * 4;
        assert_eq!(q.bytes_left(), STREAM_BYTES_PER_FRAME - 2 * one);
        let (layout, total) = q.staging_layout();
        assert_eq!(layout, vec![(0, 60), (one, one + 60)]);
        assert_eq!(total, 2 * one);
        q.bytes = STREAM_BYTES_PER_FRAME - 10;
        assert!(matches!(
            q.queue(MeshId(3), &v, &i),
            Err(RenderError::StreamBudget {
                bytes: 72,
                left: 10
            })
        ));
        q.submitted();
        assert!(q.pending.is_empty());
        assert_eq!(q.bytes_left(), STREAM_BYTES_PER_FRAME);
    }
}
