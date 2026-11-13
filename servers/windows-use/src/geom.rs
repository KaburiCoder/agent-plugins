use std::fmt;

/// A rectangle in physical screen pixels (the process is per-monitor DPI aware).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ScreenRect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl ScreenRect {
    pub fn new(x: i32, y: i32, width: i32, height: i32) -> Self {
        Self { left: x, top: y, right: x + width, bottom: y + height }
    }
    pub fn width(&self) -> i32 {
        self.right - self.left
    }
    pub fn height(&self) -> i32 {
        self.bottom - self.top
    }
    pub fn is_empty(&self) -> bool {
        self.width() <= 0 || self.height() <= 0
    }
    pub fn center(&self) -> (i32, i32) {
        ((self.left + self.right) / 2, (self.top + self.bottom) / 2)
    }
    pub fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.left && x < self.right && y >= self.top && y < self.bottom
    }
    pub fn intersect(&self, o: &Self) -> Self {
        Self {
            left: self.left.max(o.left),
            top: self.top.max(o.top),
            right: self.right.min(o.right),
            bottom: self.bottom.min(o.bottom),
        }
    }
    pub fn union(&self, o: &Self) -> Self {
        Self {
            left: self.left.min(o.left),
            top: self.top.min(o.top),
            right: self.right.max(o.right),
            bottom: self.bottom.max(o.bottom),
        }
    }
}

/// `[x,y wxh]`
impl fmt::Display for ScreenRect {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[{},{} {}x{}]", self.left, self.top, self.width(), self.height())
    }
}

impl From<uiautomation::types::Rect> for ScreenRect {
    fn from(r: uiautomation::types::Rect) -> Self {
        Self { left: r.get_left(), top: r.get_top(), right: r.get_right(), bottom: r.get_bottom() }
    }
}
