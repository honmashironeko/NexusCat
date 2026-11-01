use crate::ids::{DeviceId, DisplayNodeId};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Size {
    pub width: u32,
    pub height: u32,
}

impl Size {
    pub const fn new(width: u32, height: u32) -> Self {
        Self { width, height }
    }

    pub const fn area(self) -> u64 {
        self.width as u64 * self.height as u64
    }

    pub const fn is_empty(self) -> bool {
        self.width == 0 || self.height == 0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

impl Point {
    pub const fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl Rect {
    pub const fn new(x: f64, y: f64, width: f64, height: f64) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    pub fn right(&self) -> f64 {
        self.x + self.width
    }

    pub fn bottom(&self) -> f64 {
        self.y + self.height
    }

    pub fn center(&self) -> Point {
        Point::new(self.x + self.width / 2.0, self.y + self.height / 2.0)
    }

    pub fn contains(&self, point: Point) -> bool {
        point.x >= self.x
            && point.x <= self.right()
            && point.y >= self.y
            && point.y <= self.bottom()
    }

    pub fn is_empty(&self) -> bool {
        self.width <= 0.0 || self.height <= 0.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct PhysicalSize {
    pub width_mm: f32,
    pub height_mm: f32,
}

impl PhysicalSize {
    pub const fn new(width_mm: f32, height_mm: f32) -> Self {
        Self {
            width_mm,
            height_mm,
        }
    }

    pub fn from_diagonal_inches(size: Size, diagonal_inches: f32) -> Self {
        if size.is_empty() || diagonal_inches <= 0.0 {
            return Self::default();
        }
        let diagonal_px = (size.width as f32 * size.width as f32
            + size.height as f32 * size.height as f32)
            .sqrt();
        let mm_per_px = diagonal_inches * 25.4 / diagonal_px;
        Self::new(
            size.width as f32 * mm_per_px,
            size.height as f32 * mm_per_px,
        )
    }

    pub fn pixels_per_mm_x(&self, size: Size) -> Option<f64> {
        (self.width_mm > 0.0).then(|| size.width as f64 / self.width_mm as f64)
    }

    pub fn pixels_per_mm_y(&self, size: Size) -> Option<f64> {
        (self.height_mm > 0.0).then(|| size.height as f64 / self.height_mm as f64)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ScreenOrientation {
    #[default]
    Landscape,
    Portrait,
}

impl ScreenOrientation {
    pub fn from_size(size: Size) -> Self {
        if size.height > size.width {
            Self::Portrait
        } else {
            Self::Landscape
        }
    }

    pub fn apply(self, size: Size) -> Size {
        let matches = match self {
            Self::Landscape => size.width >= size.height,
            Self::Portrait => size.height > size.width,
        };
        if matches {
            size
        } else {
            Size::new(size.height, size.width)
        }
    }

    pub fn apply_physical(self, physical: PhysicalSize) -> PhysicalSize {
        let matches = match self {
            Self::Landscape => physical.width_mm >= physical.height_mm,
            Self::Portrait => physical.height_mm > physical.width_mm,
        };
        if matches {
            physical
        } else {
            PhysicalSize::new(physical.height_mm, physical.width_mm)
        }
    }

    pub fn toggled(self) -> Self {
        match self {
            Self::Landscape => Self::Portrait,
            Self::Portrait => Self::Landscape,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DisplayNode {
    pub id: DisplayNodeId,
    pub device_id: DeviceId,
    pub device_name: String,
    #[serde(default)]
    pub display_name: String,
    pub monitor_index: u32,
    pub resolution: Size,
    pub dpi: f32,
    pub physical: PhysicalSize,
    pub origin: Point,
    pub is_local: bool,
    pub is_primary: bool,
}

impl DisplayNode {
    pub fn bounds(&self) -> Rect {
        Rect::new(
            self.origin.x,
            self.origin.y,
            self.resolution.width as f64,
            self.resolution.height as f64,
        )
    }

    pub fn center(&self) -> Point {
        self.bounds().center()
    }

    pub fn resize_centered(&mut self, resolution: Size) -> bool {
        if self.resolution == resolution {
            return false;
        }
        let center = self.center();
        self.resolution = resolution;
        self.origin = Point::new(
            center.x - resolution.width as f64 / 2.0,
            center.y - resolution.height as f64 / 2.0,
        );
        true
    }

    pub fn contains(&self, point: Point) -> bool {
        self.bounds().contains(point)
    }

    pub fn aspect_ratio(&self) -> f64 {
        if self.resolution.height == 0 {
            1.0
        } else {
            self.resolution.width as f64 / self.resolution.height as f64
        }
    }
}
