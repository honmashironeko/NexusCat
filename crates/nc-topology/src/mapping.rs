use nc_core::{Direction, DisplayNode, Point};

const MILLIMETERS_PER_INCH: f64 = 25.4;

pub fn map_point(from: &DisplayNode, to: &DisplayNode, point: Point) -> Point {
    let fraction_x = fraction(point.x - from.origin.x, from.resolution.width as f64);
    let fraction_y = fraction(point.y - from.origin.y, from.resolution.height as f64);
    Point::new(
        to.origin.x + fraction_x * to.resolution.width as f64,
        to.origin.y + fraction_y * to.resolution.height as f64,
    )
}

pub fn map_edge_crossing(
    from: &DisplayNode,
    to: &DisplayNode,
    direction: Direction,
    exit_point: Point,
) -> Point {
    match direction {
        Direction::Left | Direction::Right => {
            let source_center_y = from.center().y;
            let target_center_y = to.center().y;
            let offset_mm = (exit_point.y - source_center_y) / pixels_per_mm_y(from);
            let target_offset = offset_mm * pixels_per_mm_y(to);
            let x = if direction == Direction::Right {
                to.origin.x
            } else {
                to.bounds().right()
            };
            let y = (target_center_y + target_offset).clamp(to.origin.y, to.bounds().bottom());
            Point::new(x, y)
        }
        Direction::Up | Direction::Down => {
            let source_center_x = from.center().x;
            let target_center_x = to.center().x;
            let offset_mm = (exit_point.x - source_center_x) / pixels_per_mm_x(from);
            let target_offset = offset_mm * pixels_per_mm_x(to);
            let y = if direction == Direction::Down {
                to.origin.y
            } else {
                to.bounds().bottom()
            };
            let x = (target_center_x + target_offset).clamp(to.origin.x, to.bounds().right());
            Point::new(x, y)
        }
    }
}

fn fraction(offset: f64, extent: f64) -> f64 {
    if extent <= 0.0 || !extent.is_finite() {
        return 0.0;
    }
    let value = offset / extent;
    if value.is_finite() {
        value.clamp(0.0, 1.0)
    } else {
        0.0
    }
}

fn pixels_per_mm_x(node: &DisplayNode) -> f64 {
    pixels_per_mm(
        node.resolution.width as f64,
        node.physical.width_mm as f64,
        node.dpi as f64,
    )
}

fn pixels_per_mm_y(node: &DisplayNode) -> f64 {
    pixels_per_mm(
        node.resolution.height as f64,
        node.physical.height_mm as f64,
        node.dpi as f64,
    )
}

fn pixels_per_mm(pixels: f64, physical_mm: f64, dpi: f64) -> f64 {
    if physical_mm.is_finite() && physical_mm > 0.0 {
        let density = pixels / physical_mm;
        if density.is_finite() && density > 0.0 {
            return density;
        }
    }
    if dpi.is_finite() && dpi > 0.0 {
        return dpi / MILLIMETERS_PER_INCH;
    }
    1.0
}
