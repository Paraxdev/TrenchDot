//! Sorting brush faces into floors, ceilings and walls that face into a room or out of the building.

use td_core::{DQuat, Ray};

use crate::brush::Brush;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FaceClass {
    Floor,
    Ceiling,
    InsideWall,
    OutsideWall,
}

/// Faces steeper than this, as the cosine of their slope, count as walls.
const LEVEL: f64 = 0.7;

/// What face `face` of `brush` is among `world`. A wall faces into a room when rays from it straight ahead and 45
/// degrees to either side all hit a brush of `world`, so a wall facing one lone building across the street stays an
/// outside wall.
pub fn classify(brush: &Brush, face: usize, world: &[&Brush]) -> FaceClass {
    let normal = brush.faces[face].plane.normal;
    if normal.y >= LEVEL {
        return FaceClass::Floor;
    }

    if normal.y <= -LEVEL {
        return FaceClass::Ceiling;
    }

    let origin = brush.face_center(face) + normal * 0.5;
    let enclosed = [-45.0_f64, 0.0, 45.0].into_iter().all(|deg| {
        let dir = DQuat::from_rotation_y(deg.to_radians()) * normal;
        let ray = Ray::new(origin, dir);
        world.iter().any(|b| b.ray_cast(&ray).is_some_and(|hit| hit.distance > 0.0))
    });
    if enclosed { FaceClass::InsideWall } else { FaceClass::OutsideWall }
}

#[cfg(test)]
mod tests {
    use td_core::{Aabb, DVec3};

    use super::*;

    fn block(min: [f64; 3], max: [f64; 3]) -> Brush {
        Brush::from_aabb(&Aabb::new(DVec3::from(min), DVec3::from(max)), "m").unwrap()
    }

    fn class_of(brush: &Brush, normal: DVec3, world: &[&Brush]) -> FaceClass {
        let face = brush.faces.iter().position(|f| f.plane.normal.dot(normal) > 0.99).unwrap();
        classify(brush, face, world)
    }

    #[test]
    fn a_room_has_inside_walls_facing_in_and_outside_walls_facing_out() {
        let floor = block([0.0, -16.0, 0.0], [256.0, 0.0, 256.0]);
        let ceiling = block([0.0, 128.0, 0.0], [256.0, 144.0, 256.0]);
        let west = block([0.0, 0.0, 0.0], [16.0, 128.0, 256.0]);
        let east = block([240.0, 0.0, 0.0], [256.0, 128.0, 256.0]);
        let north = block([16.0, 0.0, 0.0], [240.0, 128.0, 16.0]);
        let south = block([16.0, 0.0, 240.0], [240.0, 128.0, 256.0]);
        let world = [&floor, &ceiling, &west, &east, &north, &south];

        assert_eq!(class_of(&floor, DVec3::Y, &world), FaceClass::Floor);
        assert_eq!(class_of(&ceiling, -DVec3::Y, &world), FaceClass::Ceiling);
        assert_eq!(class_of(&west, DVec3::X, &world), FaceClass::InsideWall);
        assert_eq!(class_of(&west, -DVec3::X, &world), FaceClass::OutsideWall);
        assert_eq!(class_of(&north, DVec3::Z, &world), FaceClass::InsideWall);
        assert_eq!(class_of(&south, DVec3::Z, &world), FaceClass::OutsideWall);

        let shed = block([-96.0, 0.0, 112.0], [-64.0, 96.0, 144.0]);
        let with_shed = [&floor, &ceiling, &west, &east, &north, &south, &shed];
        assert_eq!(class_of(&west, -DVec3::X, &with_shed), FaceClass::OutsideWall, "one shed across the way is no room");
    }
}
