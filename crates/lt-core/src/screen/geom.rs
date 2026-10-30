use super::Rect;
use serde::{ser::SerializeStruct, Deserialize, Deserializer, Serialize, Serializer};

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct PointF {
    pub x: f64,
    pub y: f64,
}

impl PointF {
    pub const fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct RectF {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl RectF {
    pub const EMPTY: Self = Self {
        x: f64::INFINITY,
        y: f64::INFINITY,
        width: f64::NEG_INFINITY,
        height: f64::NEG_INFINITY,
    };

    pub fn new(x: f64, y: f64, width: f64, height: f64) -> Self {
        assert!(!(width < 0.0 || height < 0.0));
        Self {
            x,
            y,
            width,
            height,
        }
    }

    pub fn is_empty(self) -> bool {
        self.width < 0.0
    }

    pub fn right(self) -> f64 {
        if self.is_empty() {
            f64::NEG_INFINITY
        } else {
            self.x + self.width
        }
    }

    pub fn bottom(self) -> f64 {
        if self.is_empty() {
            f64::NEG_INFINITY
        } else {
            self.y + self.height
        }
    }

    pub fn contains(self, point: PointF) -> bool {
        !self.is_empty()
            && point.x >= self.x
            && point.x - self.width <= self.x
            && point.y >= self.y
            && point.y - self.height <= self.y
    }

    pub fn contains_rect(self, other: Self) -> bool {
        !self.is_empty()
            && !other.is_empty()
            && self.x <= other.x
            && self.y <= other.y
            && self.right() >= other.right()
            && self.bottom() >= other.bottom()
    }

    pub fn intersects_with(self, other: Self) -> bool {
        !self.is_empty()
            && !other.is_empty()
            && other.x <= self.right()
            && other.right() >= self.x
            && other.y <= self.bottom()
            && other.bottom() >= self.y
    }

    pub fn intersect(self, other: Self) -> Self {
        if !self.intersects_with(other) {
            return Self::EMPTY;
        }
        let left = max(self.x, other.x);
        let top = max(self.y, other.y);
        Self::new(
            left,
            top,
            max(min(self.right(), other.right()) - left, 0.0),
            max(min(self.bottom(), other.bottom()) - top, 0.0),
        )
    }

    pub fn union(self, other: Self) -> Self {
        if self.is_empty() {
            return other;
        }
        if other.is_empty() {
            return self;
        }
        let left = min(self.x, other.x);
        let top = min(self.y, other.y);
        let width = if self.width == f64::INFINITY || other.width == f64::INFINITY {
            f64::INFINITY
        } else {
            max(max(self.right(), other.right()) - left, 0.0)
        };
        let height = if self.height == f64::INFINITY || other.height == f64::INFINITY {
            f64::INFINITY
        } else {
            max(max(self.bottom(), other.bottom()) - top, 0.0)
        };
        Self::new(left, top, width, height)
    }

    pub fn offset(self, x: f64, y: f64) -> Self {
        assert!(!self.is_empty());
        Self::new(self.x + x, self.y + y, self.width, self.height)
    }

    pub fn inflate(self, x: f64, y: f64) -> Self {
        assert!(!self.is_empty());
        let width = (self.width + x) + x;
        let height = (self.height + y) + y;
        if width >= 0.0 && height >= 0.0 {
            Self::new(self.x - x, self.y - y, width, height)
        } else {
            Self::EMPTY
        }
    }

    pub fn enclosing(points: &[PointF]) -> Self {
        points.iter().fold(Self::EMPTY, |bounds, point| {
            bounds.union(Self::new(point.x, point.y, 0.0, 0.0))
        })
    }

    pub fn corners(self) -> [PointF; 4] {
        [
            PointF::new(self.x, self.y),
            PointF::new(self.right(), self.y),
            PointF::new(self.right(), self.bottom()),
            PointF::new(self.x, self.bottom()),
        ]
    }

    pub fn outward(self) -> Rect {
        if self.is_empty() {
            return Rect::default();
        }
        let x = self.x.floor() as i32;
        let y = self.y.floor() as i32;
        Rect {
            x,
            y,
            w: (self.right().ceil() - f64::from(x)).max(0.0) as u32,
            h: (self.bottom().ceil() - f64::from(y)).max(0.0) as u32,
        }
    }
}

impl From<Rect> for RectF {
    fn from(rect: Rect) -> Self {
        Self::new(
            f64::from(rect.x),
            f64::from(rect.y),
            f64::from(rect.w),
            f64::from(rect.h),
        )
    }
}

impl Serialize for RectF {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut fields =
            serializer.serialize_struct("RectF", if self.is_empty() { 1 } else { 5 })?;
        fields.serialize_field("empty", &self.is_empty())?;
        if !self.is_empty() {
            fields.serialize_field("x", &self.x)?;
            fields.serialize_field("y", &self.y)?;
            fields.serialize_field("width", &self.width)?;
            fields.serialize_field("height", &self.height)?;
        }
        fields.end()
    }
}

impl<'de> Deserialize<'de> for RectF {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Default, Deserialize)]
        #[serde(default)]
        struct Fields {
            empty: bool,
            x: f64,
            y: f64,
            width: f64,
            height: f64,
        }
        let fields = Fields::deserialize(deserializer)?;
        if fields.empty {
            return Ok(Self::EMPTY);
        }
        if fields.width < 0.0 || fields.height < 0.0 {
            return Err(serde::de::Error::custom(
                "rectangle dimensions are negative",
            ));
        }
        Ok(Self::new(fields.x, fields.y, fields.width, fields.height))
    }
}

pub(crate) fn min(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() {
        f64::NAN
    } else {
        a.min(b)
    }
}

pub(crate) fn max(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() {
        f64::NAN
    } else {
        a.max(b)
    }
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
pub struct LineGeometry {
    pub length: f64,
    pub thickness: f64,
    #[serde(alias = "angleDegrees")]
    pub angle_degrees: f64,
}

impl LineGeometry {
    pub const TILTED_FROM_DEGREES: f64 = 15.0;
    pub const TILTED_TO_DEGREES: f64 = 45.0;
    pub const MIN_LENGTH_TO_THICKNESS: f64 = 2.0;

    pub fn is_tilted(self) -> bool {
        self.length >= self.thickness * Self::MIN_LENGTH_TO_THICKNESS
            && (Self::TILTED_FROM_DEGREES..=Self::TILTED_TO_DEGREES)
                .contains(&self.angle_degrees.abs())
    }

    pub fn from_quad(points: &[PointF]) -> Option<Self> {
        if points.len() != 4 {
            return None;
        }
        let distance =
            |a: PointF, b: PointF| ((a.x - b.x) * (a.x - b.x) + (a.y - b.y) * (a.y - b.y)).sqrt();
        let first = distance(points[0], points[1]);
        let second = distance(points[1], points[2]);
        if first <= 0.0 || second <= 0.0 {
            return None;
        }
        let (from, to) = if first >= second {
            (points[0], points[1])
        } else {
            (points[1], points[2])
        };
        let angle = (to.y - from.y).atan2(to.x - from.x) * 180.0 / std::f64::consts::PI;
        Some(Self {
            length: max(first, second),
            thickness: min(first, second),
            angle_degrees: folded_angle(angle),
        })
    }
}

fn folded_angle(angle: f64) -> f64 {
    if angle > 90.0 {
        angle - 180.0
    } else if angle <= -90.0 {
        angle + 180.0
    } else {
        angle
    }
}
