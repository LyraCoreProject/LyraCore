//! M2 collision geometry, independent of rendering and animation records.

use anyhow::{bail, Context, Result};
use std::io::Cursor;

pub(crate) fn triangles(bytes: &[u8]) -> Result<Vec<[[f32; 3]; 3]>> {
    let header = wow_m2::header::M2Header::parse(&mut Cursor::new(bytes))
        .context("reading M2 collision header")?;
    if header.bounding_triangles.count % 3 != 0 {
        bail!("M2 collision index count is not a multiple of three");
    }
    let vertex_bytes = section(
        bytes,
        header.bounding_vertices.offset,
        header.bounding_vertices.count,
        12,
        "vertices",
    )?;
    let index_bytes = section(
        bytes,
        header.bounding_triangles.offset,
        header.bounding_triangles.count,
        2,
        "indices",
    )?;
    let mut vertices = Vec::with_capacity(vertex_bytes.len() / 12);
    for row in vertex_bytes.chunks_exact(12) {
        let vertex = [
            f32::from_le_bytes(row[0..4].try_into().unwrap()),
            f32::from_le_bytes(row[4..8].try_into().unwrap()),
            f32::from_le_bytes(row[8..12].try_into().unwrap()),
        ];
        if !vertex.iter().all(|value| value.is_finite()) {
            bail!("M2 collision vertex is not finite");
        }
        vertices.push(vertex);
    }
    let mut triangles = Vec::with_capacity(index_bytes.len() / 6);
    for row in index_bytes.chunks_exact(6) {
        let a = u16::from_le_bytes(row[0..2].try_into().unwrap()) as usize;
        let b = u16::from_le_bytes(row[2..4].try_into().unwrap()) as usize;
        let c = u16::from_le_bytes(row[4..6].try_into().unwrap()) as usize;
        if [a, b, c].iter().any(|index| *index >= vertices.len()) {
            bail!("M2 collision index exceeds the vertex count");
        }
        triangles.push([vertices[a], vertices[b], vertices[c]]);
    }
    Ok(triangles)
}

fn section<'a>(
    bytes: &'a [u8],
    offset: u32,
    count: u32,
    width: usize,
    name: &str,
) -> Result<&'a [u8]> {
    if count == 0 {
        return Ok(&[]);
    }
    let start = offset as usize;
    let end = (count as usize)
        .checked_mul(width)
        .and_then(|length| start.checked_add(length))
        .with_context(|| format!("M2 collision {name} range overflow"))?;
    bytes
        .get(start..end)
        .with_context(|| format!("M2 collision {name} range exceeds the file"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use wow_m2::common::M2Array;
    use wow_m2::header::M2Header;

    fn collision_with_unreadable_particle_records() -> Vec<u8> {
        let mut blank = vec![0; 512];
        blank[..4].copy_from_slice(b"MD20");
        blank[4..8].copy_from_slice(&256u32.to_le_bytes());
        let mut header = M2Header::parse(&mut Cursor::new(blank)).unwrap();
        header.bounding_vertices = M2Array::new(3, 512);
        header.bounding_triangles = M2Array::new(3, 548);
        header.particle_emitters = M2Array::new(1, 4096);
        let mut bytes = Vec::new();
        header.write(&mut bytes).unwrap();
        bytes.resize(512, 0);
        for vertex in [[0.0f32, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]] {
            for value in vertex {
                bytes.extend(value.to_le_bytes());
            }
        }
        for index in [0u16, 1, 2] {
            bytes.extend(index.to_le_bytes());
        }
        bytes
    }

    #[test]
    fn valid_collision_survives_unreadable_rendering_records() {
        let bytes = collision_with_unreadable_particle_records();
        assert!(wow_m2::parse_m2(&mut Cursor::new(&bytes)).is_err());
        assert_eq!(
            triangles(&bytes).unwrap(),
            vec![[[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]]]
        );
    }

    fn replace_header(bytes: &mut [u8], edit: impl FnOnce(&mut M2Header)) {
        let mut header = M2Header::parse(&mut Cursor::new(&*bytes)).unwrap();
        edit(&mut header);
        header.write(&mut Cursor::new(bytes)).unwrap();
    }

    #[test]
    fn a_model_without_collision_remains_empty() {
        let mut bytes = collision_with_unreadable_particle_records();
        replace_header(&mut bytes, |header| {
            header.bounding_vertices = M2Array::new(0, 0);
            header.bounding_triangles = M2Array::new(0, 0);
        });
        assert!(triangles(&bytes).unwrap().is_empty());
    }

    #[test]
    fn malformed_collision_cannot_be_reported_as_empty_or_partial() {
        let valid = collision_with_unreadable_particle_records();
        let mut truncated = valid.clone();
        truncated.pop();
        assert!(triangles(&truncated).is_err());
        let mut outside = valid.clone();
        outside[552..554].copy_from_slice(&3u16.to_le_bytes());
        assert!(triangles(&outside).is_err());
        let mut nonfinite = valid.clone();
        nonfinite[512..516].copy_from_slice(&f32::NAN.to_le_bytes());
        assert!(triangles(&nonfinite).is_err());
        let mut incomplete = valid.clone();
        replace_header(&mut incomplete, |header| {
            header.bounding_triangles.count = 2
        });
        assert!(triangles(&incomplete).is_err());
        let mut overflow = valid;
        replace_header(&mut overflow, |header| {
            header.bounding_vertices.offset = u32::MAX;
            header.bounding_vertices.count = u32::MAX;
        });
        assert!(triangles(&overflow).is_err());
    }
}
